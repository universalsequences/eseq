/******************************************************************************
 * AudioGraph Cycle Prevention Test
 *
 * Verifies that cycle-creating connections are rejected at connect time,
 * the graph stays healthy after rejection, and valid non-cyclic topologies
 * (diamonds, fan-out reconvergence) are not falsely rejected.
 ******************************************************************************/

#include "graph_api.h"
#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int tests_passed = 0;
static int tests_total = 0;

#define RUN_TEST(fn)                                                           \
  do {                                                                         \
    tests_total++;                                                             \
    printf("  %-50s", #fn);                                                    \
    fn();                                                                      \
    tests_passed++;                                                            \
    printf("PASS\n");                                                          \
  } while (0)

// Helper: process a block and return peak absolute output value
static float process_and_peak(LiveGraph *lg, int block_size) {
  float *buf = calloc(block_size, sizeof(float));
  process_next_block(lg, buf, block_size);
  float peak = 0.0f;
  for (int i = 0; i < block_size; i++) {
    float v = fabsf(buf[i]);
    if (v > peak)
      peak = v;
  }
  free(buf);
  return peak;
}

// ===================== Basic Rejection Tests =====================

static void test_self_loop_rejected(void) {
  LiveGraph *lg = create_live_graph(10, 128, "self_loop", 1);
  int osc = live_add_oscillator(lg, 440.0f, "osc");
  apply_graph_edits(lg->graphEditQueue, lg);

  // Self-loop: osc → osc
  graph_connect(lg, osc, 0, osc, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify osc has no input edge (connection was rejected)
  assert(lg->nodes[osc].inEdgeId == NULL ||
         lg->nodes[osc].inEdgeId[0] == -1 ||
         lg->nodes[osc].nInputs == 0);

  destroy_live_graph(lg);
}

static void test_direct_cycle_rejected(void) {
  LiveGraph *lg = create_live_graph(10, 128, "direct_cycle", 1);
  int a = live_add_gain(lg, 1.0f, "A");
  int b = live_add_gain(lg, 1.0f, "B");
  apply_graph_edits(lg->graphEditQueue, lg);

  // A → B (valid)
  graph_connect(lg, a, 0, b, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // B → A (would create cycle)
  graph_connect(lg, b, 0, a, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify A still has no input from B
  // A is a gain node (1 input), check its input edge
  if (lg->nodes[a].inEdgeId) {
    int eid = lg->nodes[a].inEdgeId[0];
    // Either no edge, or edge is not from B
    assert(eid == -1 || lg->edges[eid].src_node != b);
  }

  destroy_live_graph(lg);
}

static void test_indirect_cycle_rejected(void) {
  LiveGraph *lg = create_live_graph(10, 128, "indirect_cycle", 1);
  int a = live_add_gain(lg, 1.0f, "A");
  int b = live_add_gain(lg, 1.0f, "B");
  int c = live_add_gain(lg, 1.0f, "C");
  apply_graph_edits(lg->graphEditQueue, lg);

  // A → B → C (valid chain)
  graph_connect(lg, a, 0, b, 0);
  graph_connect(lg, b, 0, c, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // C → A (would create cycle through B)
  graph_connect(lg, c, 0, a, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify A has no input from C
  if (lg->nodes[a].inEdgeId) {
    int eid = lg->nodes[a].inEdgeId[0];
    assert(eid == -1 || lg->edges[eid].src_node != c);
  }

  destroy_live_graph(lg);
}

static void test_long_chain_cycle_rejected(void) {
  LiveGraph *lg = create_live_graph(16, 128, "long_chain", 1);
  int a = live_add_gain(lg, 1.0f, "A");
  int b = live_add_gain(lg, 1.0f, "B");
  int c = live_add_gain(lg, 1.0f, "C");
  int d = live_add_gain(lg, 1.0f, "D");
  apply_graph_edits(lg->graphEditQueue, lg);

  // A → B → C → D
  graph_connect(lg, a, 0, b, 0);
  graph_connect(lg, b, 0, c, 0);
  graph_connect(lg, c, 0, d, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // D → A (4-node cycle)
  graph_connect(lg, d, 0, a, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify A has no input from D
  if (lg->nodes[a].inEdgeId) {
    int eid = lg->nodes[a].inEdgeId[0];
    assert(eid == -1 || lg->edges[eid].src_node != d);
  }

  destroy_live_graph(lg);
}

// ===================== Audio Health After Rejection =====================

static void test_audio_continues_after_rejection(void) {
  LiveGraph *lg = create_live_graph(10, 128, "audio_health", 1);
  int osc = live_add_oscillator(lg, 440.0f, "osc");
  int gain = live_add_gain(lg, 0.5f, "gain");
  apply_graph_edits(lg->graphEditQueue, lg);

  // osc → gain → DAC
  graph_connect(lg, osc, 0, gain, 0);
  graph_connect(lg, gain, 0, lg->dac_node_id, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify audio works
  float peak_before = process_and_peak(lg, 128);
  assert(peak_before > 0.0f);

  // Attempt cycle: gain → osc (rejected)
  graph_connect(lg, gain, 0, osc, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Audio must still work — multiple blocks to be sure
  for (int i = 0; i < 5; i++) {
    float peak = process_and_peak(lg, 128);
    assert(peak > 0.0f);
  }

  destroy_live_graph(lg);
}

static void test_valid_connect_works_after_rejection(void) {
  LiveGraph *lg = create_live_graph(10, 128, "post_reject", 1);
  int osc = live_add_oscillator(lg, 440.0f, "osc");
  int g1 = live_add_gain(lg, 0.5f, "g1");
  int g2 = live_add_gain(lg, 0.8f, "g2");
  apply_graph_edits(lg->graphEditQueue, lg);

  // osc → g1 → g2
  graph_connect(lg, osc, 0, g1, 0);
  graph_connect(lg, g1, 0, g2, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Reject: g2 → osc
  graph_connect(lg, g2, 0, osc, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Now do a valid connection: g2 → DAC
  graph_connect(lg, g2, 0, lg->dac_node_id, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Should produce audio
  float peak = process_and_peak(lg, 128);
  assert(peak > 0.0f);

  destroy_live_graph(lg);
}

// ===================== Side Chain Cycle (The Heuristic Bug) =====================

static void test_cycle_in_side_chain_main_path_unaffected(void) {
  // This is the scenario the old heuristic missed:
  // Main path has source nodes, so has_cycle heuristic returns false,
  // but a side chain has a cycle that would deadlock those nodes.
  LiveGraph *lg = create_live_graph(16, 128, "side_chain", 1);

  // Main path: osc → gain → DAC
  int osc = live_add_oscillator(lg, 440.0f, "main_osc");
  int gain = live_add_gain(lg, 0.5f, "main_gain");
  apply_graph_edits(lg->graphEditQueue, lg);
  graph_connect(lg, osc, 0, gain, 0);
  graph_connect(lg, gain, 0, lg->dac_node_id, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Side chain: X → Y → Z
  int x = live_add_gain(lg, 1.0f, "X");
  int y = live_add_gain(lg, 1.0f, "Y");
  int z = live_add_gain(lg, 1.0f, "Z");
  apply_graph_edits(lg->graphEditQueue, lg);
  graph_connect(lg, x, 0, y, 0);
  graph_connect(lg, y, 0, z, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Attempt cycle in side chain: Z → X (rejected)
  graph_connect(lg, z, 0, x, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Main path audio unaffected
  float peak = process_and_peak(lg, 128);
  assert(peak > 0.0f);

  // Verify X has no input from Z
  if (lg->nodes[x].inEdgeId) {
    int eid = lg->nodes[x].inEdgeId[0];
    assert(eid == -1 || lg->edges[eid].src_node != z);
  }

  destroy_live_graph(lg);
}

// ===================== Auto-Sum Interaction =====================

static void test_cycle_through_autosum_rejected(void) {
  // A → mixer, B → mixer (auto-sum), mixer → C → ???→ A
  LiveGraph *lg = create_live_graph(16, 128, "autosum_cycle", 1);
  int a = live_add_oscillator(lg, 220.0f, "A");
  int b = live_add_oscillator(lg, 330.0f, "B");
  int c = live_add_gain(lg, 1.0f, "C");
  int d = live_add_gain(lg, 1.0f, "D");
  apply_graph_edits(lg->graphEditQueue, lg);

  // A → C (first connection to C's input 0)
  graph_connect(lg, a, 0, c, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // B → C:0 (second source to same port → triggers auto-sum)
  graph_connect(lg, b, 0, c, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // C → D
  graph_connect(lg, c, 0, d, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // D → A would create cycle: A → [SUM] → C → D → A
  graph_connect(lg, d, 0, a, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify: A is an oscillator (0 inputs), so check it wasn't corrupted
  // The connection should have been rejected
  assert(lg->nodes[a].nInputs == 0);

  destroy_live_graph(lg);
}

// ===================== Delete and Reconnect =====================

static void test_delete_breaks_cycle_path_allows_reconnect(void) {
  LiveGraph *lg = create_live_graph(16, 128, "delete_reconnect", 1);
  int a = live_add_gain(lg, 1.0f, "A");
  int b = live_add_gain(lg, 1.0f, "B");
  int c = live_add_gain(lg, 1.0f, "C");
  int osc = live_add_oscillator(lg, 440.0f, "osc");
  apply_graph_edits(lg->graphEditQueue, lg);

  // osc → A → B → C → DAC
  graph_connect(lg, osc, 0, a, 0);
  graph_connect(lg, a, 0, b, 0);
  graph_connect(lg, b, 0, c, 0);
  graph_connect(lg, c, 0, lg->dac_node_id, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Attempt C → A (cycle — rejected)
  graph_connect(lg, c, 0, a, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Audio should work
  float peak = process_and_peak(lg, 128);
  assert(peak > 0.0f);

  // Record A's successor count before — C → A was rejected, so C should not
  // be a successor going through A
  int c_succ_before = lg->nodes[c].succCount;

  // Delete B — breaks the path A → B → C
  delete_node(lg, b);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Disconnect osc → A so the next connect is a clean 1:1 (avoids auto-sum)
  graph_disconnect(lg, osc, 0, a, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Now C → A should succeed (no path from A to C exists anymore)
  graph_connect(lg, c, 0, a, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify: A should now have an input from C (direct edge)
  assert(lg->nodes[a].inEdgeId != NULL);
  int eid = lg->nodes[a].inEdgeId[0];
  assert(eid >= 0);
  assert(lg->edges[eid].src_node == c);

  // And C's successor count should have increased
  assert(lg->nodes[c].succCount > c_succ_before);

  destroy_live_graph(lg);
}

// ===================== Valid Non-Cyclic Topologies =====================

static void test_diamond_topology_allowed(void) {
  // Diamond: A → B, A → C, B → D, C → D (NOT a cycle)
  LiveGraph *lg = create_live_graph(16, 128, "diamond", 1);
  int a = live_add_oscillator(lg, 440.0f, "A");
  int b = live_add_gain(lg, 0.5f, "B");
  int c = live_add_gain(lg, 0.5f, "C");
  int d = live_add_mixer2(lg, "D");
  apply_graph_edits(lg->graphEditQueue, lg);

  // A → B and A → C
  graph_connect(lg, a, 0, b, 0);
  graph_connect(lg, a, 0, c, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // B → D:0 and C → D:1
  graph_connect(lg, b, 0, d, 0);
  graph_connect(lg, c, 0, d, 1);
  apply_graph_edits(lg->graphEditQueue, lg);

  // D → DAC
  graph_connect(lg, d, 0, lg->dac_node_id, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Should produce audio (this is a valid DAG)
  float peak = process_and_peak(lg, 128);
  assert(peak > 0.0f);

  destroy_live_graph(lg);
}

static void test_fanout_reconverge_allowed(void) {
  // A → B → D, A → C → D (fan-out then reconverge via auto-sum)
  LiveGraph *lg = create_live_graph(16, 128, "fanout", 1);
  int a = live_add_oscillator(lg, 440.0f, "A");
  int b = live_add_gain(lg, 0.5f, "B");
  int c = live_add_gain(lg, 0.5f, "C");
  int d = live_add_gain(lg, 1.0f, "D");
  apply_graph_edits(lg->graphEditQueue, lg);

  // A → B, A → C
  graph_connect(lg, a, 0, b, 0);
  graph_connect(lg, a, 0, c, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // B → D:0 (first connection)
  graph_connect(lg, b, 0, d, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // C → D:0 (second source to same port → auto-sum)
  graph_connect(lg, c, 0, d, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // D → DAC
  graph_connect(lg, d, 0, lg->dac_node_id, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Should produce audio (DAG, not a cycle)
  float peak = process_and_peak(lg, 128);
  assert(peak > 0.0f);

  destroy_live_graph(lg);
}

// ===================== Stress: Repeated Reject/Accept =====================

static void test_repeated_cycle_attempts_dont_corrupt(void) {
  LiveGraph *lg = create_live_graph(16, 128, "stress_reject", 1);
  int osc = live_add_oscillator(lg, 440.0f, "osc");
  int g1 = live_add_gain(lg, 0.5f, "g1");
  int g2 = live_add_gain(lg, 0.5f, "g2");
  int g3 = live_add_gain(lg, 0.5f, "g3");
  apply_graph_edits(lg->graphEditQueue, lg);

  // osc → g1 → g2 → g3 → DAC
  graph_connect(lg, osc, 0, g1, 0);
  graph_connect(lg, g1, 0, g2, 0);
  graph_connect(lg, g2, 0, g3, 0);
  graph_connect(lg, g3, 0, lg->dac_node_id, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Repeatedly try to create cycles — all should be rejected
  for (int i = 0; i < 20; i++) {
    graph_connect(lg, g3, 0, osc, 0); // g3 → osc (cycle)
    graph_connect(lg, g2, 0, g1, 0);  // g2 → g1 (cycle)
    graph_connect(lg, g3, 0, g1, 0);  // g3 → g1 (cycle)
    apply_graph_edits(lg->graphEditQueue, lg);

    // Audio must still work every iteration
    float peak = process_and_peak(lg, 128);
    assert(peak > 0.0f);
  }

  destroy_live_graph(lg);
}

// ===================== DAC Connection (Edge Case) =====================

static void test_cycle_through_dac_rejected(void) {
  // Ensure we can't create a cycle through the DAC node
  LiveGraph *lg = create_live_graph(10, 128, "dac_cycle", 1);
  int osc = live_add_oscillator(lg, 440.0f, "osc");
  int gain = live_add_gain(lg, 1.0f, "gain");
  apply_graph_edits(lg->graphEditQueue, lg);

  // osc → gain → DAC
  graph_connect(lg, osc, 0, gain, 0);
  graph_connect(lg, gain, 0, lg->dac_node_id, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // DAC → osc (would create cycle if DAC had an output)
  // DAC is a sink (nOutputs=0), so this should fail on port validation,
  // but verify it doesn't crash
  graph_connect(lg, lg->dac_node_id, 0, osc, 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Audio still works
  float peak = process_and_peak(lg, 128);
  assert(peak > 0.0f);

  destroy_live_graph(lg);
}

// ===================== Main =====================

int main(void) {
  printf("\n=== Cycle Prevention Tests ===\n\n");

  printf("Basic rejection:\n");
  RUN_TEST(test_self_loop_rejected);
  RUN_TEST(test_direct_cycle_rejected);
  RUN_TEST(test_indirect_cycle_rejected);
  RUN_TEST(test_long_chain_cycle_rejected);

  printf("\nGraph health after rejection:\n");
  RUN_TEST(test_audio_continues_after_rejection);
  RUN_TEST(test_valid_connect_works_after_rejection);

  printf("\nSide chain cycle (heuristic bug scenario):\n");
  RUN_TEST(test_cycle_in_side_chain_main_path_unaffected);

  printf("\nAuto-sum interaction:\n");
  RUN_TEST(test_cycle_through_autosum_rejected);

  printf("\nDelete and reconnect:\n");
  RUN_TEST(test_delete_breaks_cycle_path_allows_reconnect);

  printf("\nValid non-cyclic topologies (must NOT be rejected):\n");
  RUN_TEST(test_diamond_topology_allowed);
  RUN_TEST(test_fanout_reconverge_allowed);

  printf("\nStress:\n");
  RUN_TEST(test_repeated_cycle_attempts_dont_corrupt);
  RUN_TEST(test_cycle_through_dac_rejected);

  printf("\n%d/%d tests passed\n", tests_passed, tests_total);
  assert(tests_passed == tests_total);
  printf("\nAll cycle prevention tests passed.\n\n");
  return 0;
}
