#include "graph_profile.h"

#ifdef AUDIOGRAPH_EXPERIMENTS
#include <errno.h>

enum { PROFILE_IDLE, PROFILE_REQUESTED, PROFILE_CAPTURING, PROFILE_READY, PROFILE_READING };
static _Atomic int profile_state;
static GraphProfile profile;
// Written only by the callback outside an open worker session. Session
// publication/acquisition and close_work_session synchronize worker reads.
static bool profile_active;
static pthread_t recorder;
static bool recorder_started;
static _Atomic bool recorder_stop;

bool graph_profile_request(void) {
  int expected = PROFILE_IDLE;
  return atomic_compare_exchange_strong_explicit(&profile_state, &expected,
      PROFILE_REQUESTED, memory_order_acq_rel, memory_order_acquire);
}

bool graph_profile_take(GraphProfile *output) {
  if (!output) return false;
  int expected = PROFILE_READY;
  if (!atomic_compare_exchange_strong_explicit(&profile_state, &expected,
      PROFILE_READING, memory_order_acq_rel, memory_order_acquire)) return false;
  memcpy(output, &profile, sizeof(*output));
  atomic_store_explicit(&profile_state, PROFILE_IDLE, memory_order_release);
  return true;
}

void graph_profile_begin(LiveGraph *graph, int frames) {
  int expected = PROFILE_REQUESTED;
  if (!atomic_compare_exchange_strong_explicit(&profile_state, &expected,
      PROFILE_CAPTURING, memory_order_acq_rel, memory_order_acquire)) return;
  profile.frames = frames;
  profile.node_count = graph->node_count;
  profile.edge_count = 0;
  profile.overflow = graph->node_count > GRAPH_PROFILE_NODES;
  if (!profile.overflow)
    memset(profile.nodes, 0, sizeof(*profile.nodes) * (size_t)graph->node_count);
  profile_active = !profile.overflow;
  profile.start_ns = nsec_now();
}

uint64_t graph_profile_node_begin(void) {
  return profile_active ? nsec_now() : 0;
}

void graph_profile_node_end(int node, int worker, uint64_t start) {
  if (!start) return;
  // A DAG job owns its node's slot exactly once per render slice.
  profile.nodes[node].end_ns = nsec_now();
  profile.nodes[node].start_ns = start;
  profile.nodes[node].worker = worker;
}

void graph_profile_end(LiveGraph *graph) {
  if (atomic_load_explicit(&profile_state, memory_order_acquire) != PROFILE_CAPTURING) return;
  profile.end_ns = nsec_now();
  profile.workers = atomic_load_explicit(&g_engine.activeWorkerLimit, memory_order_relaxed);
  profile_active = false;
  // Metadata is copied after the timed interval, while the graph is still
  // callback-owned. No graph pointers escape to the recorder thread.
  if (!profile.overflow) {
    for (int i = 0; i < graph->node_count; i++) {
      RTNode *node = &graph->nodes[i];
      profile.nodes[i].logical_id = node->logical_id;
      const char *name = node->debug_name ? node->debug_name : "";
      size_t length = strnlen(name, sizeof(profile.nodes[i].name) - 1);
      // Keep a truncated UTF-8 name on a character boundary.
      if (length == sizeof(profile.nodes[i].name) - 1)
        while (length && ((unsigned char)name[length] & 0xc0) == 0x80) length--;
      memcpy(profile.nodes[i].name, name, length);
      profile.nodes[i].name[length] = 0;
      for (int j = 0; j < node->succCount; j++) {
        if (profile.edge_count == GRAPH_PROFILE_EDGES) {
          profile.overflow = 1;
          break;
        }
        profile.edges[profile.edge_count++] = (GraphProfileEdge){i, node->succ[j]};
      }
    }
  }
  atomic_store_explicit(&profile_state, PROFILE_READY, memory_order_release);
}

static void json_string(FILE *file, const char *s) {
  fputc('"', file);
  for (const unsigned char *p = (const unsigned char *)s; *p; p++) {
    if (*p == '"' || *p == '\\') { fputc('\\', file); fputc(*p, file); }
    else if (*p < 32) fprintf(file, "\\u%04x", *p);
    else fputc(*p, file);
  }
  fputc('"', file);
}

static bool write_profile(FILE *file, const GraphProfile *p) {
  fprintf(file, "{\"start_ns\":%llu,\"end_ns\":%llu,\"frames\":%d,\"workers\":%d,\"overflow\":%d,\"nodes\":[",
      (unsigned long long)p->start_ns, (unsigned long long)p->end_ns,
      p->frames, p->workers, p->overflow);
  bool first = true;
  for (int i = 0; !p->overflow && i < p->node_count; i++) {
    const GraphProfileNode *n = &p->nodes[i];
    if (!n->start_ns) continue;
    fprintf(file, "%s{\"id\":%d,\"logical_id\":%llu,\"worker\":%d,\"start_ns\":%llu,\"end_ns\":%llu,\"name\":",
        first ? "" : ",", i, (unsigned long long)n->logical_id, n->worker,
        (unsigned long long)n->start_ns, (unsigned long long)n->end_ns);
    json_string(file, n->name);
    fputc('}', file);
    first = false;
  }
  fputs("],\"edges\":[", file);
  for (int i = 0; !p->overflow && i < p->edge_count; i++)
    fprintf(file, "%s[%d,%d]", i ? "," : "", p->edges[i].source, p->edges[i].destination);
  fputs("]}\n", file);
  return fflush(file) == 0 && !ferror(file);
}

typedef struct { FILE *file; char *request_path; GraphProfile *snapshot; } Recorder;

static void *record_profiles(void *argument) {
  Recorder *r = argument;
  // Creating <output>.request arms forty captures, one every half second.
  // The control thread does all file access and sleeps; DSP only fills the
  // bounded snapshot and hands ownership back through profile_state.
  int remaining = 0;
  bool waiting = false;
  uint64_t next = 0;
  while (!atomic_load_explicit(&recorder_stop, memory_order_acquire)) {
    if (!remaining && access(r->request_path, F_OK) == 0 && unlink(r->request_path) == 0)
      remaining = 40;
    if (waiting && graph_profile_take(r->snapshot)) {
      if (!write_profile(r->file, r->snapshot)) {
        fprintf(stderr, "audiograph: cannot write graph profile: %s\n", strerror(errno));
        break;
      }
      waiting = false;
      remaining--;
      next = nsec_now() + 500000000;
    }
    if (remaining && !waiting && nsec_now() >= next)
      waiting = graph_profile_request();
    usleep(10000);
  }
  fclose(r->file);
  free(r->snapshot);
  free(r->request_path);
  free(r);
  return NULL;
}

void graph_profile_start_recorder(void) {
  const char *path = getenv("TINYSEQ_AUDIOGRAPH_PROFILE");
  if (!path || !*path || recorder_started) return;
  Recorder *r = calloc(1, sizeof(*r));
  if (!r) return;
  r->snapshot = malloc(sizeof(*r->snapshot));
  r->request_path = malloc(strlen(path) + sizeof(".request"));
  r->file = fopen(path, "wx");
  if (!r->snapshot || !r->request_path || !r->file) {
    fprintf(stderr, "audiograph: cannot open profile recorder %s: %s\n", path, strerror(errno));
    if (r->file) fclose(r->file);
    free(r->snapshot); free(r->request_path); free(r);
    return;
  }
  sprintf(r->request_path, "%s.request", path);
  atomic_store_explicit(&recorder_stop, false, memory_order_release);
  int result = pthread_create(&recorder, NULL, record_profiles, r);
  if (result != 0) {
    fprintf(stderr, "audiograph: cannot start profile recorder: %s\n", strerror(result));
    fclose(r->file); free(r->snapshot); free(r->request_path); free(r);
    return;
  }
  recorder_started = true;
}

void graph_profile_stop_recorder(void) {
  if (recorder_started) {
    atomic_store_explicit(&recorder_stop, true, memory_order_release);
    pthread_join(recorder, NULL);
    recorder_started = false;
  }
  profile_active = false;
  atomic_store_explicit(&profile_state, PROFILE_IDLE, memory_order_release);
}
#endif
