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
  s->value2 = 3.0f;
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

// Edge definition for systematic disconnection
typedef struct {
  int src_node;
  int src_port;
  int dst_node;
  int dst_port;
  const char *name;
  bool is_active;
} Edge;

// Comprehensive state validation
typedef struct {
  float expected_dac_output;
  bool node1_connected_to_node3;
  bool node1_connected_to_node4;
  bool node2_connected_to_node3;
  bool node2_connected_to_dac;
  bool node3_connected_to_dac;
  bool node4_connected_to_dac;
  int active_edge_count;
} ExpectedState;

// Global test state
static LiveGraph *lg = NULL;
static int node1_id, node2_id, node3_id, node4_id;
static Edge edges[6];
static int total_permutations = 0;
static int successful_tests = 0;
static int failed_tests = 0;

// Calculate expected DAC output based on active edges
static float calculate_expected_dac_output(Edge *edges, int edge_count) {
  float dac_sum = 0.0f;

  // Track what's connected to each node's inputs
  bool node3_has_node1 = false;
  bool node3_has_node2 = false;
  bool node4_has_node1 = false;

  // Scan active edges to determine connections
  for (int i = 0; i < edge_count; i++) {
    if (!edges[i].is_active)
      continue;

    if (edges[i].src_node == node1_id && edges[i].dst_node == node3_id) {
      node3_has_node1 = true;
    }
    if (edges[i].src_node == node2_id && edges[i].dst_node == node3_id) {
      node3_has_node2 = true;
    }
    if (edges[i].src_node == node1_id && edges[i].dst_node == node4_id) {
      node4_has_node1 = true;
    }
  }

  // Calculate node outputs
  float node1_out = 1.0f;  // Always generates 1.0
  float node2_out0 = 2.0f; // Always generates 2.0 on port 0
  float node2_out1 = 3.0f; // Always generates 3.0 on port 1

  // Node 3 (multiplier): output = input0 * input1
  float node3_input0 = node3_has_node1 ? node1_out : 0.0f;
  float node3_input1 = node3_has_node2 ? node2_out0 : 0.0f;
  float node3_out = node3_input0 * node3_input1;

  // Node 4 (gain=0.5): output = input * 0.5
  float node4_input = node4_has_node1 ? node1_out : 0.0f;
  float node4_out = node4_input * 0.5f;

  // Sum contributions to DAC
  for (int i = 0; i < edge_count; i++) {
    if (!edges[i].is_active)
      continue;

    if (edges[i].dst_node == lg->dac_node_id) {
      if (edges[i].src_node == node2_id && edges[i].src_port == 1) {
        dac_sum += node2_out1; // 3.0
      }
      if (edges[i].src_node == node3_id) {
        dac_sum += node3_out;
      }
      if (edges[i].src_node == node4_id) {
        dac_sum += node4_out;
      }
    }
  }

  return dac_sum;
}

// Validate graph state after disconnections
static bool validate_graph_state(Edge *edges, int edge_count,
                                 const char *test_name) {
  const int block_size = 256;
  float output_buffer[block_size];
  memset(output_buffer, 0, sizeof(output_buffer));

  // Process audio
  process_next_block(lg, output_buffer, block_size);

  // Calculate expected output
  float expected = calculate_expected_dac_output(edges, edge_count);
  float actual = output_buffer[0];
  float tolerance = 0.001f;

  bool output_correct = fabs(actual - expected) < tolerance;

  // Count active edges
  int active_count = 0;
  for (int i = 0; i < edge_count; i++) {
    if (edges[i].is_active)
      active_count++;
  }

  // Validate indegrees make sense
  bool indegrees_valid = true;

  // Node 1 should always have indegree 0 (source node)
  if (lg->sched.indegree[node1_id] != 0) {
    printf("🐛 INDEGREE BUG: Node 1 has indegree %d, expected 0\n",
           lg->sched.indegree[node1_id]);
    indegrees_valid = false;
  }

  // Node 2 should always have indegree 0 (source node)
  if (lg->sched.indegree[node2_id] != 0) {
    printf("🐛 INDEGREE BUG: Node 2 has indegree %d, expected 0\n",
           lg->sched.indegree[node2_id]);
    indegrees_valid = false;
  }

  if (!output_correct || !indegrees_valid) {
    printf("❌ VALIDATION FAILED for %s:\n", test_name);
    printf("   Expected DAC: %.3f, Actual: %.3f (diff: %.6f)\n", expected,
           actual, actual - expected);
    printf("   Active edges: %d\n", active_count);
    printf("   Indegrees: N1=%d, N2=%d, N3=%d, N4=%d, DAC=%d\n",
           lg->sched.indegree[node1_id], lg->sched.indegree[node2_id],
           lg->sched.indegree[node3_id], lg->sched.indegree[node4_id],
           lg->sched.indegree[lg->dac_node_id]);
    printf("   Active edges: ");
    for (int i = 0; i < edge_count; i++) {
      if (edges[i].is_active) {
        printf("%s ", edges[i].name);
      }
    }
    printf("\n");

    // CRITICAL DEBUG: Check actual graph data structures
    if (lg->sched.indegree[lg->dac_node_id] == 0 &&
        fabs(actual - expected) > 0.001f) {
      printf("🔍 CRITICAL BUG: DAC indegree=0 but expected non-zero output\n");
      printf("   Examining actual DAC connections in graph data structures:\n");

      RTNode *dac = &lg->nodes[lg->dac_node_id];
      bool found_connections = false;
      for (int i = 0; i < dac->nInputs; i++) {
        int edge_id = dac->inEdgeId[i];
        if (edge_id >= 0) {
          LiveEdge *edge = &lg->edges[edge_id];
          printf("     DAC Input[%d]: Edge %d from Node %d (port %d)\n", i,
                 edge_id, edge->src_node, edge->src_port);
          found_connections = true;
        }
      }
      if (!found_connections) {
        printf(
            "     ❌ NO actual DAC input connections found in graph data!\n");
      }

      printf("   Orphaned status: DAC=%s\n",
             lg->sched.is_orphaned[lg->dac_node_id] ? "ORPHANED" : "CONNECTED");
    }

    return false;
  }

  return true;
}

// Recursive function to generate all permutations of edge disconnections
static void test_disconnection_permutation(Edge *edges, int edge_count,
                                           int depth, char *permutation_name) {
  total_permutations++;

  // Validate current state
  char test_name[256];
  snprintf(test_name, sizeof(test_name), "Perm_%d_%s", total_permutations,
           permutation_name);

  bool valid = validate_graph_state(edges, edge_count, test_name);
  if (valid) {
    successful_tests++;
  } else {
    failed_tests++;
    printf("🔍 Detailed state for failed test %s:\n", test_name);

    // Print detailed node states
    printf("   Node states after processing:\n");
    for (int i = 1; i <= 4; i++) {
      if (i <= lg->node_count) {
        printf("     Node %d: inputs=%d, outputs=%d\n", i, lg->nodes[i].nInputs,
               lg->nodes[i].nOutputs);
      }
    }

    // Stop on first failure for detailed debugging
    if (failed_tests == 1) {
      printf("🚨 Stopping on first failure for debugging\n");
      return;
    }
  }

  // If we've disconnected all edges, we're done with this branch
  if (depth == edge_count) {
    return;
  }

  // Try disconnecting each remaining active edge
  for (int i = 0; i < edge_count; i++) {
    if (!edges[i].is_active)
      continue; // Skip already disconnected edges

    // Disconnect this edge
    printf("🔌 Disconnecting %s (permutation %d, depth %d)\n", edges[i].name,
           total_permutations, depth + 1);

    bool disconnect_success =
        graph_disconnect(lg, edges[i].src_node, edges[i].src_port,
                         edges[i].dst_node, edges[i].dst_port);
    if (disconnect_success) {
      apply_graph_edits(lg->graphEditQueue, lg);
      edges[i].is_active = false;

      // Create new permutation name
      char new_name[512];
      snprintf(new_name, sizeof(new_name), "%s-%s", permutation_name,
               edges[i].name);

      // Recurse
      test_disconnection_permutation(edges, edge_count, depth + 1, new_name);

      // Reconnect for next iteration (backtrack)
      printf("🔄 Reconnecting %s for backtrack\n", edges[i].name);
      bool reconnect_success =
          graph_connect(lg, edges[i].src_node, edges[i].src_port,
                        edges[i].dst_node, edges[i].dst_port);
      if (reconnect_success) {
        apply_graph_edits(lg->graphEditQueue, lg);
        edges[i].is_active = true;
      } else {
        printf("❌ Failed to reconnect %s during backtrack!\n", edges[i].name);
        failed_tests++;
        return;
      }
    } else {
      printf("❌ Failed to disconnect %s\n", edges[i].name);
      failed_tests++;
      return;
    }
  }
}

// Initialize the test topology
static bool setup_test_topology() {
  const int block_size = 256;
  lg = create_live_graph(32, block_size, "fuzz_test_graph", 1);
  if (!lg)
    return false;

  printf("🏗️  Setting up 4-node topology...\n");

  // Create nodes
  node1_id = add_node(lg, NUMBER_GEN_VTABLE, sizeof(NumberGenState),
                      "number_gen", 0, 1, NULL, 0);
  node2_id = add_node(lg, DUAL_OUTPUT_VTABLE, sizeof(DualOutputState),
                      "dual_output", 0, 2, NULL, 0);
  node3_id = add_node(lg, MULTIPLIER_VTABLE, sizeof(MultiplierState),
                      "multiplier", 2, 1, NULL, 0);
  node4_id = live_add_gain(lg, 0.5f, "gain");

  if (node1_id < 0 || node2_id < 0 || node3_id < 0 || node4_id < 0) {
    printf("❌ Failed to create nodes\n");
    return false;
  }

  // Apply node creation
  apply_graph_edits(lg->graphEditQueue, lg);

  printf("   Nodes created: N1=%d, N2=%d, N3=%d, N4=%d, DAC=%d\n", node1_id,
         node2_id, node3_id, node4_id, lg->dac_node_id);

  // Define and create all edges
  edges[0] = (Edge){node1_id, 0, node3_id, 0, "N1→N3", false};
  edges[1] = (Edge){node1_id, 0, node4_id, 0, "N1→N4", false};
  edges[2] = (Edge){node2_id, 0, node3_id, 1, "N2→N3", false};
  edges[3] = (Edge){node2_id, 1, lg->dac_node_id, 0, "N2→DAC", false};
  edges[4] = (Edge){node3_id, 0, lg->dac_node_id, 0, "N3→DAC", false};
  edges[5] = (Edge){node4_id, 0, lg->dac_node_id, 0, "N4→DAC", false};

  // Create all connections
  for (int i = 0; i < 6; i++) {
    printf("   Creating edge: %s\n", edges[i].name);

    bool success;
    if (edges[i].dst_node == lg->dac_node_id) {
      success = apply_connect(lg, edges[i].src_node, edges[i].src_port,
                              edges[i].dst_node, edges[i].dst_port);
    } else {
      success = graph_connect(lg, edges[i].src_node, edges[i].src_port,
                              edges[i].dst_node, edges[i].dst_port);
    }

    if (!success) {
      printf("❌ Failed to create edge: %s\n", edges[i].name);
      return false;
    }
    edges[i].is_active = true;
  }

  // Apply all connections
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✅ Topology setup complete\n\n");

  return true;
}

int main() {
  printf("🧪 4-Node Topology Fuzz Test\n");
  printf("=============================\n\n");
  printf(
      "This test will disconnect edges in EVERY possible order and validate\n");
  printf("the graph state at each step. With 6 edges, there are 6! = 720 "
         "possible\n");
  printf("disconnection sequences, plus intermediate states = 1956 total "
         "tests.\n\n");

  if (!setup_test_topology()) {
    printf("❌ Failed to setup test topology\n");
    return 1;
  }

  // Validate initial state
  printf("🔍 Validating initial state (all edges connected)...\n");
  if (!validate_graph_state(edges, 6, "InitialState")) {
    printf("❌ Initial state validation failed!\n");
    return 1;
  }
  printf("✅ Initial state is valid\n\n");

  // Run exhaustive permutation testing
  printf("🚀 Starting exhaustive disconnection permutation testing...\n\n");

  char initial_name[64] = "Initial";
  test_disconnection_permutation(edges, 6, 0, initial_name);

  printf("\n🏁 Fuzz Test Results:\n");
  printf("=====================\n");
  printf("Total permutations tested: %d\n", total_permutations);
  printf("Successful tests: %d\n", successful_tests);
  printf("Failed tests: %d\n", failed_tests);

  if (failed_tests == 0) {
    printf("🎉 ALL TESTS PASSED! No edge disconnection bugs found.\n");
  } else {
    printf("🐛 BUGS FOUND! %d tests failed.\n", failed_tests);
  }

  printf("Coverage: %.1f%% of possible disconnection states tested\n",
         100.0f * total_permutations / 1956.0f);

  // Cleanup
  destroy_live_graph(lg);

  return failed_tests > 0 ? 1 : 0;
}
