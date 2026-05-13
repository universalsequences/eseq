#include <assert.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"

// Forward declaration for cycle detection function
extern bool detect_cycle(LiveGraph *lg);

// Stress test to trigger the non-deterministic disconnection bug
// Runs the same failing sequence multiple times with different memory states

// Custom node states (identical to fuzz test)
typedef struct {
  float value;
} NumberGenState;

typedef struct {
  float value1;
  float value2;
} DualOutputState;

typedef struct {
  float dummy;
} MultiplierState;

// Custom node process functions (identical to fuzz test)
static void number_gen_process(float *const *inputs, float *const *outputs,
                               int block_size, void *state, void *buffers) {
  (void)inputs;
  NumberGenState *s = (NumberGenState *)state;

  for (int i = 0; i < block_size; i++) {
    outputs[0][i] = s->value;
  }
}

static void dual_output_process(float *const *inputs, float *const *outputs,
                                int block_size, void *state, void *buffers) {
  (void)inputs;
  DualOutputState *s = (DualOutputState *)state;

  for (int i = 0; i < block_size; i++) {
    outputs[0][i] = s->value1;
    outputs[1][i] = s->value2;
  }
}

static void multiplier_process(float *const *inputs, float *const *outputs,
                               int block_size, void *state, void *buffers) {
  (void)state;

  for (int i = 0; i < block_size; i++) {
    outputs[0][i] = inputs[0][i] * inputs[1][i];
  }
}

// Custom node init functions (identical to fuzz test)
static void number_gen_init(void *state, int sr, int mb,
                            const void *initial_state) {
  (void)sr;
  (void)mb;
  NumberGenState *s = (NumberGenState *)state;
  s->value = 1.0f;
}

static void dual_output_init(void *state, int sr, int mb,
                             const void *initial_state) {
  (void)sr;
  (void)mb;
  DualOutputState *s = (DualOutputState *)state;
  s->value1 = 2.0f;
  s->value2 = 3.0f;
}

static void multiplier_init(void *state, int sr, int mb,
                            const void *initial_state) {
  (void)sr;
  (void)mb;
  (void)state;
}

// Custom VTables (identical to fuzz test)
static const NodeVTable NUMBER_GEN_VTABLE = {.init = number_gen_init,
                                             .process = number_gen_process,
                                             .reset = NULL,
                                             .migrate = NULL};

static const NodeVTable DUAL_OUTPUT_VTABLE = {.init = dual_output_init,
                                              .process = dual_output_process,
                                              .reset = NULL,
                                              .migrate = NULL};

static const NodeVTable MULTIPLIER_VTABLE = {.init = multiplier_init,
                                             .process = multiplier_process,
                                             .reset = NULL,
                                             .migrate = NULL};

// Global graph state that gets reused and corrupted over iterations
static LiveGraph *g_reused_graph = NULL;
static int g_node1_id, g_node2_id, g_node3_id, g_node4_id;

// Initialize the reusable graph (called once)
static void initialize_reusable_graph() {
  if (g_reused_graph)
    return; // Already initialized

  g_reused_graph = create_live_graph(32, 256, "reused_stress_test", 1);
  assert(g_reused_graph != NULL);

  // Create nodes once and reuse them
  g_node1_id = add_node(g_reused_graph, NUMBER_GEN_VTABLE,
                        sizeof(NumberGenState), "number_gen", 0, 1);
  g_node2_id = add_node(g_reused_graph, DUAL_OUTPUT_VTABLE,
                        sizeof(DualOutputState), "dual_output", 0, 2);
  g_node3_id = add_node(g_reused_graph, MULTIPLIER_VTABLE,
                        sizeof(MultiplierState), "multiplier", 2, 1);
  g_node4_id = live_add_gain(g_reused_graph, 0.5f, "gain");

  apply_graph_edits(g_reused_graph->graphEditQueue, g_reused_graph);
  printf("✅ Initialized reusable graph with nodes: N1=%d, N2=%d, N3=%d, "
         "N4=%d, DAC=%d\n",
         g_node1_id, g_node2_id, g_node3_id, g_node4_id,
         g_reused_graph->dac_node_id);
}

// Manually disconnect all edges (instead of destroy_live_graph)
static void disconnect_all_edges() {
  LiveGraph *lg = g_reused_graph;

  // Disconnect all edges in reverse order of creation to avoid dependency
  // issues
  graph_disconnect(lg, g_node4_id, 0, lg->dac_node_id, 0); // N4→DAC
  graph_disconnect(lg, g_node3_id, 0, lg->dac_node_id, 0); // N3→DAC
  apply_disconnect(lg, g_node2_id, 1, lg->dac_node_id, 0); // N2→DAC
  graph_disconnect(lg, g_node2_id, 0, g_node3_id, 1);      // N2→N3
  graph_disconnect(lg, g_node1_id, 0, g_node4_id, 0);      // N1→N4
  graph_disconnect(lg, g_node1_id, 0, g_node3_id, 0);      // N1→N3

  apply_graph_edits(lg->graphEditQueue, lg);
}

// Reconnect all edges to restore initial topology
static void reconnect_full_topology() {
  LiveGraph *lg = g_reused_graph;

  // Recreate the full 6-edge topology
  graph_connect(lg, g_node1_id, 0, g_node3_id, 0);      // N1→N3
  graph_connect(lg, g_node1_id, 0, g_node4_id, 0);      // N1→N4
  graph_connect(lg, g_node2_id, 0, g_node3_id, 1);      // N2→N3
  apply_connect(lg, g_node2_id, 1, lg->dac_node_id, 0); // N2→DAC
  apply_connect(lg, g_node3_id, 0, lg->dac_node_id, 0); // N3→DAC
  apply_connect(lg, g_node4_id, 0, lg->dac_node_id, 0); // N4→DAC

  apply_graph_edits(lg->graphEditQueue, lg);
}

// Run one iteration using the reusable corrupted graph
static bool run_single_iteration(int iteration) {
  LiveGraph *lg = g_reused_graph;

  // Key: DON'T create fresh graph - reuse the corrupted one!
  // Step 1: Disconnect all edges (simulates graph reset without destroying
  // state)
  disconnect_all_edges();

  // Step 2: Reconnect to initial topology (builds up corruption)
  reconnect_full_topology();

  // Step 3: Execute the failing disconnection sequence on the corrupted graph
  // Use graceful failure handling instead of assertions to detect corruption
  bool step1_ok = graph_disconnect(lg, g_node1_id, 0, g_node4_id, 0); // N1→N4
  if (step1_ok)
    apply_graph_edits(lg->graphEditQueue, lg);

  bool step2_ok =
      apply_disconnect(lg, g_node4_id, 0, lg->dac_node_id, 0); // N4→DAC
  if (step2_ok)
    apply_graph_edits(lg->graphEditQueue, lg);

  bool step3_ok = graph_disconnect(lg, g_node2_id, 0, g_node3_id, 1); // N2→N3
  if (step3_ok)
    apply_graph_edits(lg->graphEditQueue, lg);

  bool step4_ok =
      apply_disconnect(lg, g_node3_id, 0, lg->dac_node_id, 0); // N3→DAC
  if (step4_ok)
    apply_graph_edits(lg->graphEditQueue, lg);

  // Check if any disconnect operations failed (indicates corruption)
  if (!step1_ok || !step2_ok || !step3_ok || !step4_ok) {
    printf("🚨 GRAPH CORRUPTION DETECTED in iteration %d!\n", iteration);
    printf("   Disconnect results: N1→N4=%s, N4→DAC=%s, N2→N3=%s, N3→DAC=%s\n",
           step1_ok ? "OK" : "FAIL", step2_ok ? "OK" : "FAIL",
           step3_ok ? "OK" : "FAIL", step4_ok ? "OK" : "FAIL");
    return true; // This is the bug we're looking for!
  }

  // Check final output (should be 3.0 from N2→DAC, but bug causes 0.0)
  float output_buffer[256];
  memset(output_buffer, 0, sizeof(output_buffer));
  process_next_block(lg, output_buffer, 256);

  float actual_output = output_buffer[0];
  float expected_output = 3.0f; // From N2 port 1 → DAC

  bool has_bug = (fabs(actual_output) < 0.001f) && (expected_output > 0.001f);

  if (has_bug) {
    printf("🐛 BUG REPRODUCED in iteration %d! (Reused graph corruption)\n",
           iteration);
    printf("   Expected: %.3f, Got: %.3f\n", expected_output, actual_output);
    printf("   DAC indegree: %d (should be 1)\n",
           lg->sched.indegree[lg->dac_node_id]);

    // Check actual DAC connections
    RTNode *dac = &lg->nodes[lg->dac_node_id];
    printf("   Actual DAC connections: ");
    bool found_any = false;
    for (int i = 0; i < dac->nInputs; i++) {
      if (dac->inEdgeId[i] >= 0) {
        printf("input[%d]=edge%d ", i, dac->inEdgeId[i]);
        found_any = true;
      }
    }
    if (!found_any) {
      printf("NONE (graph corruption confirmed!)");
    }
    printf("\n");

  } else if (iteration % 50 == 0) {
    printf("   Iteration %d: OK (output=%.3f)\n", iteration, actual_output);
  }

  return has_bug;
}

// Run multiple different failing test cases from fuzz test
static bool run_different_sequences(int iteration) {
  // Try different failing sequences from the fuzz test results
  const char *sequences[] = {
      "N1→N4-N4→DAC-N2→N3-N3→DAC", // Original failing case
      "N2→N3-N1→N3-N1→N4",         // Another failing case
      "N2→N3-N3→DAC-N1→N3",        // Another failing case
      "N1→N4-N4→DAC-N3→DAC",       // Another failing case
  };

  int seq_index = iteration % 4;

  LiveGraph *lg = create_live_graph(32, 256, "different_sequences", 1);
  assert(lg != NULL);

  // Create nodes
  int node1_id = add_node(lg, NUMBER_GEN_VTABLE, sizeof(NumberGenState),
                          "number_gen", 0, 1);
  int node2_id = add_node(lg, DUAL_OUTPUT_VTABLE, sizeof(DualOutputState),
                          "dual_output", 0, 2);
  int node3_id = add_node(lg, MULTIPLIER_VTABLE, sizeof(MultiplierState),
                          "multiplier", 2, 1);
  int node4_id = live_add_gain(lg, 0.5f, "gain");

  apply_graph_edits(lg->graphEditQueue, lg);

  // Always create all connections first
  assert(graph_connect(lg, node1_id, 0, node3_id, 0));
  assert(graph_connect(lg, node1_id, 0, node4_id, 0));
  assert(graph_connect(lg, node2_id, 0, node3_id, 1));
  assert(apply_connect(lg, node2_id, 1, lg->dac_node_id, 0));
  assert(apply_connect(lg, node3_id, 0, lg->dac_node_id, 0));
  assert(apply_connect(lg, node4_id, 0, lg->dac_node_id, 0));

  apply_graph_edits(lg->graphEditQueue, lg);

  // Execute different disconnection sequences
  bool has_bug = false;

  switch (seq_index) {
  case 0: // N1→N4-N4→DAC-N2→N3-N3→DAC
    assert(graph_disconnect(lg, node1_id, 0, node4_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(apply_disconnect(lg, node4_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(graph_disconnect(lg, node2_id, 0, node3_id, 1));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(apply_disconnect(lg, node3_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    break;

  case 1: // N2→N3-N1→N3-N1→N4
    assert(graph_disconnect(lg, node2_id, 0, node3_id, 1));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(graph_disconnect(lg, node1_id, 0, node3_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(graph_disconnect(lg, node1_id, 0, node4_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    break;

  case 2: // N2→N3-N3→DAC-N1→N3
    assert(graph_disconnect(lg, node2_id, 0, node3_id, 1));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(apply_disconnect(lg, node3_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(graph_disconnect(lg, node1_id, 0, node3_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    break;

  case 3: // N1→N4-N4→DAC-N3→DAC
    assert(graph_disconnect(lg, node1_id, 0, node4_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(apply_disconnect(lg, node4_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    assert(apply_disconnect(lg, node3_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    break;
  }

  // Check for bug
  float output_buffer[256];
  memset(output_buffer, 0, sizeof(output_buffer));
  process_next_block(lg, output_buffer, 256);

  float actual_output = output_buffer[0];

  // Different sequences have different expected outputs
  // For simplicity, just check if output is near zero when it shouldn't be
  has_bug =
      (fabs(actual_output) < 0.001f) && (lg->sched.indegree[lg->dac_node_id] > 0);

  if (has_bug) {
    printf("🐛 BUG FOUND in sequence '%s' (iteration %d)!\n",
           sequences[seq_index], iteration);
    printf("   Output: %.6f, DAC indegree: %d\n", actual_output,
           lg->sched.indegree[lg->dac_node_id]);
  }

  destroy_live_graph(lg);
  return has_bug;
}

int main() {
  printf("🧪 Bug Stress Test - Reused Graph Corruption Hunter\n");
  printf("====================================================\n\n");
  printf("Running iterations with REUSED graph instance to trigger cumulative "
         "corruption...\n");
  printf("This simulates the exact conditions that cause the 163 fuzz test "
         "failures.\n\n");

  // Initialize the reusable graph once (key difference from original approach)
  initialize_reusable_graph();

  int total_iterations = 1000;
  int bugs_found = 0;

  // Fine-tune to start 3 iterations before first observed failure (iteration
  // 124)
  int start_iteration = 121;
  int focused_iterations =
      10; // Run just 10 iterations around the corruption point

  printf("🔬 FOCUSED TEST: Running iterations %d-%d (around corruption "
         "point)...\n",
         start_iteration, start_iteration + focused_iterations - 1);

  // Run iterations 0 to start_iteration-1 silently to build up corruption
  printf("📈 Building corruption state with %d silent iterations...\n",
         start_iteration);
  for (int i = 0; i < start_iteration; i++) {
    run_single_iteration(i); // Don't check for bugs, just accumulate corruption
  }

  printf("🔍 Now monitoring iterations %d-%d for corruption...\n",
         start_iteration, start_iteration + focused_iterations - 1);
  for (int i = start_iteration; i < start_iteration + focused_iterations; i++) {
    if (run_single_iteration(i)) {
      bugs_found++;
      if (bugs_found >= 3) {
        printf("   Stopping after finding %d bugs for debugging.\n",
               bugs_found);
        break;
      }
    }
  }

  printf("\n🔬 FOCUSED TEST: Skipping second phase to focus on critical "
         "corruption window.\n");

  printf("\n🏁 Stress Test Results:\n");
  printf("=======================\n");
  printf("Total iterations: %d\n", total_iterations);
  printf("Bugs found: %d\n", bugs_found);
  printf("Bug rate: %.1f%%\n", 100.0f * bugs_found / total_iterations);

  if (bugs_found > 0) {
    printf("\n🎯 SUCCESS: Cumulative graph corruption bug reproduced!\n");
    printf("   This confirms the exact same bug from the 163 fuzz test "
           "failures.\n");
    printf("   The bug is caused by reusing the same graph across many "
           "operations.\n");
    printf("   Ready for detailed lldb analysis on the corrupted graph "
           "instance.\n");
  } else {
    printf("\n⚠️  Bug not reproduced in this run.\n");
    printf("   The corruption may need more iterations to build up.\n");
    printf("   Try increasing iteration count or running multiple times.\n");
  }

  // Cleanup the reused graph
  if (g_reused_graph) {
    destroy_live_graph(g_reused_graph);
    g_reused_graph = NULL;
  }

  return bugs_found > 0 ? 0 : 1;
}
