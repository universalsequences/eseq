#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

// Test topology mirrors the provided patch diagram:
//
//   Source(0→A, 1→B)
//      ├─(0)→ Pass(1x1) ─────────────┐
//      └─(1)→ Dup(2x2: in0→out0,
//                 in1→out1) ──┬─> DAC[0]
//                              └─> DAC[1]
//
// Connections (before disconnect):
//  - source:0 → pass:0 → DAC:0
//  - source:0 → dup:0 and source:1 → dup:1
//  - dup:0 → DAC:0  (auto-SUM with pass path)
//  - dup:1 → DAC:1
//
// Then we disconnect pass→DAC:0 and confirm:
//  - Orphaning: pass is orphaned; source, dup, DAC remain connected
//  - Audio still valid: L= B (was A+B before), R = B

// ===================== Custom Nodes =====================

// 0-in / 2-out constant source: out0=A, out1=B
static const float SRC_VAL_A = 1.0f;
static const float SRC_VAL_B = 2.0f;

static void src2_process(float *const *in, float *const *out, int n,
                         void *memory, void *buffers) {
  (void)in;
  (void)memory;
  float *o0 = out[0];
  float *o1 = out[1];
  for (int i = 0; i < n; i++) {
    o0[i] = SRC_VAL_A;
    o1[i] = SRC_VAL_B;
  }
}

static const NodeVTable SRC2_VT = {
    .process = src2_process, .init = NULL, .reset = NULL, .migrate = NULL};

// 1-in / 1-out pass-through
static void pass_process(float *const *in, float *const *out, int n,
                         void *memory, void *buffers) {
  (void)memory;
  const float *i0 = in[0];
  float *o0 = out[0];
  if (!i0) {
    memset(o0, 0, (size_t)n * sizeof(float));
    return;
  }
  memcpy(o0, i0, (size_t)n * sizeof(float));
}

static const NodeVTable PASS_VT = {
    .process = pass_process, .init = NULL, .reset = NULL, .migrate = NULL};

// 2-in / 2-out duplicator: out0=in0, out1=in1
static void dup22_process(float *const *in, float *const *out, int n,
                          void *memory, void *buffers) {
  (void)memory;
  const float *i0 = in[0];
  const float *i1 = in[1];
  float *o0 = out[0];
  float *o1 = out[1];

  if (i0)
    memcpy(o0, i0, (size_t)n * sizeof(float));
  else
    memset(o0, 0, (size_t)n * sizeof(float));

  if (i1)
    memcpy(o1, i1, (size_t)n * sizeof(float));
  else
    memset(o1, 0, (size_t)n * sizeof(float));
}

static const NodeVTable DUP22_VT = {
    .process = dup22_process, .init = NULL, .reset = NULL, .migrate = NULL};

int main() {
  printf("=== Patch Disconnect Orphan Test ===\n");

  const int block_size = 64;
  initialize_engine(block_size, 48000);

  // Stereo graph (2 DAC inputs/outputs)
  LiveGraph *lg =
      create_live_graph(16, block_size, "patch_disconnect_orphan", 2);
  assert(lg && lg->num_channels == 2);
  int dac = lg->dac_node_id;
  assert(dac >= 0);
  assert(lg->nodes[dac].nInputs == 2 && lg->nodes[dac].nOutputs == 2);

  // Create nodes
  int src = add_node(lg, SRC2_VT, 0, "src2", 0, 2, NULL, 0);
  int pass = add_node(lg, PASS_VT, 0, "pass", 1, 1, NULL, 0);
  int dup22 = add_node(lg, DUP22_VT, 0, "dup22", 2, 2, NULL, 0);
  assert(src >= 0 && pass >= 0 && dup22 >= 0);

  // Realize node creations
  bool applied = apply_graph_edits(lg->graphEditQueue, lg);
  assert(applied);

  // Wire connections
  // Left branch: src:0 -> pass:0 -> DAC:0

  // Right branch: src:1 feeds dup22 both inputs, then dup22->DAC[0,1]
  assert(apply_connect(lg, src, 0, dup22, 0));
  assert(apply_connect(lg, src, 1, dup22, 1));
  assert(apply_connect(lg, src, 0, pass, 0));
  assert(apply_connect(lg, pass, 0, dac, 0));
  assert(apply_connect(lg, dup22, 1, dac, 1));
  assert(apply_connect(lg, dup22, 0, dac, 0)); // creates auto-SUM on DAC:0

  // Validate orphan status pre-disconnect (none orphaned)
  assert(!lg->sched.is_orphaned[dac]);
  assert(!lg->sched.is_orphaned[src]);
  assert(!lg->sched.is_orphaned[pass]);
  assert(!lg->sched.is_orphaned[dup22]);
  printf("✓ Pre-disconnect: all nodes connected (no orphans)\n");

  // Process a block and verify with current wiring:
  //  - DAC[0] sees pass(A) + dup22.out0(A) = 2A
  //  - DAC[1] sees dup22.out1(B) = B
  float buf[block_size * 2];
  memset(buf, 0, sizeof(buf));
  process_next_block(lg, buf, block_size);

  float L0 = buf[0];
  float R0 = buf[1];
  float expected_L_pre = 2.0f * SRC_VAL_A; // 2A
  float expected_R_pre = SRC_VAL_B;        // B

  if (fabsf(L0 - expected_L_pre) >= 0.001f ||
      fabsf(R0 - expected_R_pre) >= 0.001f) {
    printf("✗ Pre-disconnect output mismatch: L=%.3f (exp %.3f), R=%.3f (exp "
           "%.3f)\n",
           L0, expected_L_pre, R0, expected_R_pre);
    return 1;
  }
  printf("✓ Pre-disconnect audio OK: L=%.3f, R=%.3f\n", L0, R0);

  // Disconnect pass → DAC:0
  assert(apply_disconnect(lg, pass, 0, dac, 0));

  // After disconnect, orphaning should update automatically
  assert(!lg->sched.is_orphaned[dac]);
  assert(!lg->sched.is_orphaned[src]);
  assert(!lg->sched.is_orphaned[dup22]);
  assert(lg->sched.is_orphaned[pass]);
  printf(
      "✓ Post-disconnect orphaning OK: pass is orphaned, others connected\n");

  // Process again after disconnect:
  //  - DAC[0] sees dup22.out0(A) = A
  //  - DAC[1] sees dup22.out1(B) = B
  memset(buf, 0, sizeof(buf));
  process_next_block(lg, buf, block_size);
  float L1 = buf[0];
  float R1 = buf[1];

  float expected_L_post = SRC_VAL_A; // only dup22:0 (A) remains on DAC:0
  float expected_R_post = SRC_VAL_B; // dup22:1 (B) on DAC:1

  if (fabsf(L1 - expected_L_post) >= 0.001f ||
      fabsf(R1 - expected_R_post) >= 0.001f) {
    printf("✗ Post-disconnect output mismatch: L=%.3f (exp %.3f), R=%.3f (exp "
           "%.3f)\n",
           L1, expected_L_post, R1, expected_R_post);
    return 1;
  }
  printf("✓ Post-disconnect audio OK: L=%.3f, R=%.3f\n", L1, R1);

  destroy_live_graph(lg);
  printf("\n=== Test Passed: Graph remains valid after disconnect, orphaning "
         "correct ===\n");
  return 0;
}
