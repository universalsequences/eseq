#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

// Custom nodes for the topology
static const float A = 1.0f, B = 2.0f;

static void src2(float *const *in, float *const *out, int n, void *m, void *b) {
  (void)in;
  (void)m;
  for (int i = 0; i < n; i++) {
    out[0][i] = A;
    out[1][i] = B;
  }
}
static const NodeVTable SRC2 = {.process = src2};

static void passt(float *const *in, float *const *out, int n, void *m,
                  void *b) {
  (void)m;
  if (in[0])
    memcpy(out[0], in[0], (size_t)n * sizeof(float));
  else
    memset(out[0], 0, (size_t)n * sizeof(float));
}
static const NodeVTable PASS = {.process = passt};

static void dup22(float *const *in, float *const *out, int n, void *m,
                  void *b) {
  (void)m;
  if (in[0])
    memcpy(out[0], in[0], (size_t)n * sizeof(float));
  else
    memset(out[0], 0, (size_t)n * sizeof(float));
  if (in[1])
    memcpy(out[1], in[1], (size_t)n * sizeof(float));
  else
    memset(out[1], 0, (size_t)n * sizeof(float));
}
static const NodeVTable DUP22 = {.process = dup22};

static double now_ms() {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

int main() {
  const int block = 128;
  initialize_engine(block, 48000);
  engine_start_workers(2); // exercise the path that used to hang

  LiveGraph *lg = create_live_graph(16, block, "deadlock_disconnect", 2);
  assert(lg);
  int dac = lg->dac_node_id;

  int src = add_node(lg, SRC2, 0, "src2", 0, 2, NULL, 0);
  int pass = add_node(lg, PASS, 0, "pass", 1, 1, NULL, 0);
  int dup = add_node(lg, DUP22, 0, "dup22", 2, 2, NULL, 0);
  assert(src >= 0 && pass >= 0 && dup >= 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Wiring: src:0 -> pass -> DAC[0]; src:0 -> dup:0; src:1 -> dup:1;
  // dup:0->DAC[0], dup:1->DAC[1]
  assert(apply_connect(lg, src, 0, pass, 0));
  assert(apply_connect(lg, pass, 0, dac, 0));
  assert(apply_connect(lg, src, 0, dup, 0));
  assert(apply_connect(lg, src, 1, dup, 1));
  assert(apply_connect(lg, dup, 0, dac, 0));
  assert(apply_connect(lg, dup, 1, dac, 1));

  float out[block * 2];
  double t0 = now_ms();
  process_next_block(lg, out, block);
  double t1 = now_ms();
  printf("First block processed in %.3f ms\n", (t1 - t0));

  // Disconnect pass -> DAC[0]; previously this caused deadlock when indegree
  // drifted
  assert(apply_disconnect(lg, pass, 0, dac, 0));

  double t2 = now_ms();
  process_next_block(lg, out, block);
  double t3 = now_ms();
  printf("Second block after disconnect processed in %.3f ms\n", (t3 - t2));

  // Sanity: both calls should return quickly
  assert((t1 - t0) < 500.0);
  assert((t3 - t2) < 500.0);

  engine_stop_workers();
  destroy_live_graph(lg);
  printf("✓ No deadlock on disconnect with workers enabled\n");
  return 0;
}
