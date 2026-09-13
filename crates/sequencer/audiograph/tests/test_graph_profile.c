#include "../graph_api.h"
#include "../graph_edit.h"
#include "../graph_nodes.h"
#include "../graph_profile.h"
#include <assert.h>

// A delayed test kernel guarantees overlapping jobs; this is an ordering and
// ownership regression, not a measurement of real DSP throughput.
static void source(float *const *in, float *const *out, int frames,
                   void *state, void *buffers) {
  (void)in; (void)state; (void)buffers;
  usleep(200);
  for (int i = 0; i < frames; i++) out[0][i] = 1.0f;
}

int main(void) {
  enum { SOURCES = 128, FRAMES = 64 };
  engine_start_workers(4);
  LiveGraph *g = create_live_graph(512, FRAMES, "profile", 1);
  assert(g);
  int tail = live_add_gain(g, 1, "joined \"output\"");
  int ids[SOURCES];
  NodeVTable vt = {.process = source};
  for (int i = 0; i < SOURCES; i++) {
    ids[i] = apply_add_node(g, vt, 0, (uint64_t)i + 1000, "parallel source", 0, 1, NULL);
    assert(ids[i] >= 0);
    assert(apply_connect_internal(g, ids[i], 0, tail, 0));
  }
  assert(apply_connect_internal(g, tail, 0, g->dac_node_id, 0));
  update_orphaned_status(g);
  GraphProfile *p = calloc(1, sizeof(*p));
  assert(p);
  for (int repeat = 0; repeat < 8; repeat++) {
    float output[FRAMES];
    assert(graph_profile_request());
    assert(!graph_profile_request());
    assert(!graph_profile_take(p));
    process_next_block(g, output, FRAMES);
    assert(graph_profile_take(p));
    assert(!graph_profile_take(p));
    assert(!p->overflow && p->frames == FRAMES && p->workers == 4);
    uint64_t sum = 0;
    bool saw_worker = false;
    for (int i = 0; i < SOURCES; i++) {
      GraphProfileNode *n = &p->nodes[ids[i]];
      assert(n->logical_id == (uint64_t)i + 1000);
      assert(n->start_ns >= p->start_ns && n->end_ns >= n->start_ns);
      assert(n->end_ns <= p->end_ns);
      assert(n->worker >= 0 && n->worker <= 4);
      saw_worker |= n->worker != 0;
      sum += n->end_ns - n->start_ns;
    }
    assert(saw_worker);
    assert(sum > p->end_ns - p->start_ns);
    for (int i = 0; i < p->edge_count; i++) {
      GraphProfileNode *a = &p->nodes[p->edges[i].source];
      GraphProfileNode *b = &p->nodes[p->edges[i].destination];
      if (a->end_ns && b->start_ns) assert(a->end_ns <= b->start_ns);
    }
    for (int i = 0; i < FRAMES; i++) assert(output[i] == SOURCES);
  }
  assert(graph_profile_request());
  LiveGraph oversized = {.node_count = GRAPH_PROFILE_NODES + 1};
  graph_profile_begin(&oversized, FRAMES);
  graph_profile_end(&oversized);
  assert(graph_profile_take(p) && p->overflow);
  free(p);
  destroy_live_graph(g);
  engine_stop_workers();
  puts("graph profile: parallel timelines, dependencies, ownership and bounds pass");
  return 0;
}
