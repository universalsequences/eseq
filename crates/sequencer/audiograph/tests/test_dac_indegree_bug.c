#include <assert.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"

// Custom node states (same as 4-node topology test)
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

// Custom node process functions
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

// Custom node init functions
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
  s->value2 = 3.0f; // This should appear at DAC when N2→DAC is connected
}

static void multiplier_init(void *state, int sr, int mb,
                            const void *initial_state) {
  (void)sr;
  (void)mb;
  (void)state;
}

// Custom VTables
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

// Test the exact failing case with preceding context: Perm_615, 616, 617
// This reproduces the cumulative state that leads to the bug
int main() {
  printf("🐛 DAC Indegree Bug Reproduction Test (with context)\n");
  printf("====================================================\n");
  printf("Reproducing: Permutations 615→616→617 sequence\n");
  printf("Expected issue: DAC indegree=0 but N2→DAC edge still active\n\n");

  const int block_size = 256;
  LiveGraph *lg = create_live_graph(32, block_size, "dac_indegree_bug_test", 1);
  if (!lg) {
    printf("❌ Failed to create live graph\n");
    return 1;
  }

  // Create nodes
  int node1_id = add_node(lg, NUMBER_GEN_VTABLE, sizeof(NumberGenState),
                          "number_gen", 0, 1);
  int node2_id = add_node(lg, DUAL_OUTPUT_VTABLE, sizeof(DualOutputState),
                          "dual_output", 0, 2);
  int node3_id = add_node(lg, MULTIPLIER_VTABLE, sizeof(MultiplierState),
                          "multiplier", 2, 1);
  int node4_id = live_add_gain(lg, 0.5f, "gain");

  if (node1_id < 0 || node2_id < 0 || node3_id < 0 || node4_id < 0) {
    printf("❌ Failed to create nodes\n");
    return 1;
  }

  apply_graph_edits(lg->graphEditQueue, lg);

  printf("✅ Nodes created: N1=%d, N2=%d, N3=%d, N4=%d, DAC=%d\n", node1_id,
         node2_id, node3_id, node4_id, lg->dac_node_id);

  // Create all initial connections
  printf("\n🔗 Creating initial topology...\n");

  // All 6 edges from the original topology
  graph_connect(lg, node1_id, 0, node3_id, 0);        // N1→N3
  graph_connect(lg, node1_id, 0, node4_id, 0);        // N1→N4
  graph_connect(lg, node2_id, 0, node3_id, 1);        // N2→N3
  apply_connect(lg, node2_id, 1, lg->dac_node_id, 0); // N2→DAC (key edge!)
  graph_connect(lg, node3_id, 0, lg->dac_node_id, 0); // N3→DAC
  graph_connect(lg, node4_id, 0, lg->dac_node_id, 0); // N4→DAC

  apply_graph_edits(lg->graphEditQueue, lg);

  // Validate initial state
  printf("📊 Initial state:\n");
  printf("   DAC indegree: %d\n", lg->sched.indegree[lg->dac_node_id]);

  float output_buffer[block_size];
  memset(output_buffer, 0, sizeof(output_buffer));
  process_next_block(lg, output_buffer, block_size);
  printf("   DAC output: %.3f\n", output_buffer[0]);

  // Now reproduce the exact sequence leading up to the bug, including
  // backtracking This simulates permutations 615-617 with all the intermediate
  // steps
  printf("\n🔌 Executing sequence that leads to bug (Perms 615-617)...\n");

  bool success;

  // Starting state: All edges connected
  // We need to simulate the partial disconnection state before perm 615

  // First, disconnect N1→N4 (this was done in earlier permutations)
  printf("Setup: Disconnecting N1→N4 (from earlier permutations)...\n");
  success = graph_disconnect(lg, node1_id, 0, node4_id, 0);
  if (!success) {
    printf("❌ Failed to disconnect N1→N4\n");
    return 1;
  }
  apply_graph_edits(lg->graphEditQueue, lg);

  // Disconnect N4→DAC (from earlier permutations)
  printf("Setup: Disconnecting N4→DAC (from earlier permutations)...\n");
  success = graph_disconnect(lg, node4_id, 0, lg->dac_node_id, 0);
  if (!success) {
    printf("❌ Failed to disconnect N4→DAC\n");
    return 1;
  }
  apply_graph_edits(lg->graphEditQueue, lg);

  // Disconnect N2→N3 (from earlier permutations)
  printf("Setup: Disconnecting N2→N3 (from earlier permutations)...\n");
  success = graph_disconnect(lg, node2_id, 0, node3_id, 1);
  if (!success) {
    printf("❌ Failed to disconnect N2→N3\n");
    return 1;
  }
  apply_graph_edits(lg->graphEditQueue, lg);

  // Now we should be in the state where the bug manifests
  // Remaining edges: N1→N3, N2→DAC, N3→DAC

  printf("\n🔬 State before critical sequence:\n");
  printf("   DAC indegree: %d\n", lg->sched.indegree[lg->dac_node_id]);

  // Permutation 615: Disconnect N3→DAC, then N1→N3, then reconnect both
  // (backtrack)
  printf("\n--- PERM 615 SEQUENCE ---\n");
  printf("Perm 615: Disconnecting N3→DAC...\n");
  success = graph_disconnect(lg, node3_id, 0, lg->dac_node_id, 0);
  if (!success) {
    printf("❌ Failed to disconnect N3→DAC\n");
    return 1;
  }
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("   DAC indegree after N3→DAC disconnect: %d\n",
         lg->sched.indegree[lg->dac_node_id]);

  printf("Perm 615: Disconnecting N1→N3...\n");
  success = graph_disconnect(lg, node1_id, 0, node3_id, 0);
  if (!success) {
    printf("❌ Failed to disconnect N1→N3\n");
    return 1;
  }
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("   DAC indegree after N1→N3 disconnect: %d\n",
         lg->sched.indegree[lg->dac_node_id]);

  // Backtrack: Reconnect N1→N3
  printf("Backtrack: Reconnecting N1→N3...\n");
  success = graph_connect(lg, node1_id, 0, node3_id, 0);
  if (!success) {
    printf("❌ Failed to reconnect N1→N3\n");
    return 1;
  }
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("   DAC indegree after N1→N3 reconnect: %d\n",
         lg->sched.indegree[lg->dac_node_id]);

  // Backtrack: Reconnect N3→DAC
  printf("Backtrack: Reconnecting N3→DAC...\n");
  success = graph_connect(lg, node3_id, 0, lg->dac_node_id, 0);
  if (!success) {
    printf("❌ Failed to reconnect N3→DAC\n");
    return 1;
  }
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("   DAC indegree after N3→DAC reconnect: %d\n",
         lg->sched.indegree[lg->dac_node_id]);

  // Permutation 616: Disconnect N3→DAC (and that's it - this sets up the final
  // bug state)
  printf("\n--- PERM 616 SEQUENCE ---\n");
  printf("Perm 616: Disconnecting N3→DAC...\n");
  success = graph_disconnect(lg, node3_id, 0, lg->dac_node_id, 0);
  if (!success) {
    printf("❌ Failed to disconnect N3→DAC\n");
    return 1;
  }
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("   DAC indegree after final N3→DAC disconnect: %d\n",
         lg->sched.indegree[lg->dac_node_id]);

  // Now we should be in the exact state that causes the bug in Perm 617
  printf("\n--- PERM 617 VALIDATION (EXPECTING BUG HERE) ---\n");

  printf("\n🔍 FINAL STATE ANALYSIS:\n");
  printf("   DAC indegree: %d\n", lg->sched.indegree[lg->dac_node_id]);

  // Check which edges are still connected to DAC
  RTNode *dac = &lg->nodes[lg->dac_node_id];
  printf("   DAC input connections:\n");
  for (int i = 0; i < dac->nInputs; i++) {
    int edge_id = dac->inEdgeId[i];
    if (edge_id >= 0) {
      LiveEdge *edge = &lg->edges[edge_id];
      printf("     Input[%d]: Edge %d from Node %d (port %d)\n", i, edge_id,
             edge->src_node, edge->src_port);
    } else {
      printf("     Input[%d]: No connection\n", i);
    }
  }

  // Process audio and check output
  memset(output_buffer, 0, sizeof(output_buffer));
  process_next_block(lg, output_buffer, block_size);
  float actual_output = output_buffer[0];

  printf("   DAC output: %.3f\n", actual_output);

  // Expected: N2→DAC should give 3.0 (node2 port 1 outputs 3.0)
  // The remaining active edges should be: N1→N3 and N2→DAC
  float expected_output = 3.0f; // Only N2→DAC remains

  printf("\n🎯 VALIDATION:\n");
  printf("   Expected DAC output: %.3f\n", expected_output);
  printf("   Actual DAC output: %.3f\n", actual_output);
  printf("   Difference: %.6f\n", actual_output - expected_output);

  bool bug_reproduced = (lg->sched.indegree[lg->dac_node_id] == 0 &&
                         fabs(actual_output - expected_output) > 0.001f);

  if (bug_reproduced) {
    printf("🐛 BUG REPRODUCED! DAC indegree is 0 but should process N2→DAC "
           "connection\n");
    printf("   This proves the indegree tracking bug exists in this exact "
           "scenario\n");

    // Additional debugging info
    printf("\n🔬 DEBUG INFO:\n");
    printf("   Node 2 (dual_output) state:\n");
    DualOutputState *n2_state = (DualOutputState *)lg->nodes[node2_id].state;
    if (n2_state) {
      printf("     value1=%.3f, value2=%.3f\n", n2_state->value1,
             n2_state->value2);
    }

    destroy_live_graph(lg);
    return 1; // Return error code to indicate bug was reproduced
  } else {
    printf("✅ No bug reproduced - indegree tracking appears correct\n");
    destroy_live_graph(lg);
    return 0;
  }
}
