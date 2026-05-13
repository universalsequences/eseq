#include "graph_engine.h"
#include "graph_nodes.h"
#include "graph_edit.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include <string.h>

// Minimal 2-output process: copies input to both outputs
void dual_out_process(float *const *in, float *const *out, int n, void *memory,
                      void *buffers) {
  (void)memory;
  (void)buffers;
  const float *a = in[0];
  float *y0 = out[0];
  float *y1 = out[1];
  for (int i = 0; i < n; i++) {
    y0[i] = a[i];       // L = input
    y1[i] = a[i] * 2.0f; // R = input * 2
  }
}

static NodeVTable DUAL_OUT_VTABLE = {
  .process = dual_out_process,
  .init = NULL,
  .reset = NULL,
  .migrate = NULL,
};

// Test: queued add_node with 2 outputs + queued graph_connect
int main() {
  printf("=== Testing Multi-Output Node (Queued Operations) ===\n\n");

  initialize_engine(128, 48000);

  LiveGraph *lg = create_live_graph(64, 128, "multi_out_test", 2);
  assert(lg != NULL);
  int dac_id = lg->dac_node_id;

  // Create bus_L and bus_R immediately (like our Rust code)
  int bus_l = live_add_gain(lg, 1.0f, "bus_L");
  int bus_r = live_add_gain(lg, 1.0f, "bus_R");
  printf("bus_L=%d, bus_R=%d\n", bus_l, bus_r);

  // Queue bus connections to DAC (like our Rust code)
  bool ok;
  ok = graph_connect(lg, bus_l, 0, dac_id, 0);
  printf("graph_connect(bus_L -> DAC[0]): %s\n", ok ? "queued" : "FAILED");
  ok = graph_connect(lg, bus_r, 0, dac_id, 1);
  printf("graph_connect(bus_R -> DAC[1]): %s\n", ok ? "queued" : "FAILED");

  // Queue a source node (number = 10.0)
  int src = live_add_number(lg, 10.0f, "source");
  printf("source=%d\n", src);

  // Queue a 2-output node (1 in, 2 out) — like our delay node
  int dual = add_node(lg, DUAL_OUT_VTABLE, 0, "dual_out", 1, 2, NULL, 0);
  printf("dual_out=%d (nOutputs=2)\n", dual);

  // Queue connections: src -> dual, dual[0] -> bus_L, dual[1] -> bus_R
  ok = graph_connect(lg, src, 0, dual, 0);
  printf("graph_connect(src -> dual): %s\n", ok ? "queued" : "FAILED");
  ok = graph_connect(lg, dual, 0, bus_l, 0);
  printf("graph_connect(dual[0] -> bus_L): %s\n", ok ? "queued" : "FAILED");
  ok = graph_connect(lg, dual, 1, bus_r, 0);
  printf("graph_connect(dual[1] -> bus_R): %s\n", ok ? "queued" : "FAILED");

  // Process — this will drain the queue and run the graph
  int nframes = 128;
  float output[nframes * 2];
  memset(output, 0, sizeof(output));

  process_next_block(lg, output, nframes);

  printf("\nAfter process_next_block:\n");

  // Check dual_out node state
  RTNode *dual_node = &lg->nodes[dual];
  printf("  dual_out nOutputs=%d\n", dual_node->nOutputs);
  printf("  dual_out outEdgeId[0]=%d\n",
         dual_node->outEdgeId ? dual_node->outEdgeId[0] : -999);
  printf("  dual_out outEdgeId[1]=%d\n",
         dual_node->outEdgeId ? dual_node->outEdgeId[1] : -999);
  printf("  dual_out is_orphaned=%d\n", lg->sched.is_orphaned[dual]);
  printf("  dual_out cached_outPtrs[0]=%p\n",
         dual_node->cached_outPtrs ? (void *)dual_node->cached_outPtrs[0] : NULL);
  printf("  dual_out cached_outPtrs[1]=%p\n",
         dual_node->cached_outPtrs ? (void *)dual_node->cached_outPtrs[1] : NULL);
  printf("  scratch_null=%p\n", (void *)lg->scratch_null);

  // Check bus_R state
  RTNode *bus_r_node = &lg->nodes[bus_r];
  printf("  bus_R inEdgeId[0]=%d\n",
         bus_r_node->inEdgeId ? bus_r_node->inEdgeId[0] : -999);
  printf("  bus_R is_orphaned=%d\n", lg->sched.is_orphaned[bus_r]);

  // Check output
  printf("\n  Output samples:\n");
  for (int i = 0; i < 4; i++) {
    printf("    Frame %d: L=%.1f (expect 10.0), R=%.1f (expect 20.0)\n",
           i, output[i * 2], output[i * 2 + 1]);
  }

  float left = output[0];
  float right = output[1];

  if (fabsf(left - 10.0f) < 0.001f && fabsf(right - 20.0f) < 0.001f) {
    printf("\n=== PASS: Multi-output node works with queued operations! ===\n");
  } else {
    printf("\n=== FAIL: L=%.1f (expected 10.0), R=%.1f (expected 20.0) ===\n",
           left, right);
  }

  destroy_live_graph(lg);
  return 0;
}
