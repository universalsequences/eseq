#include <assert.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>

#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"

// Custom dual-output node for testing
typedef struct {
  float port0_value;
  float port1_value;
} DualOutputState;

static void dual_output_process(float *const *inputs, float *const *outputs,
                                int block_size, void *state, void *buffers) {
  (void)inputs; // No inputs
  DualOutputState *s = (DualOutputState *)state;

  // Fill output port 0 with first value
  for (int i = 0; i < block_size; i++) {
    outputs[0][i] = s->port0_value;
  }

  // Fill output port 1 with second value
  for (int i = 0; i < block_size; i++) {
    outputs[1][i] = s->port1_value;
  }
}

static void dual_output_init(void *state, int sampleRate, int maxBlock,
                             const void *initial_state) {
  (void)sampleRate;
  (void)maxBlock;
  DualOutputState *s = (DualOutputState *)state;

  // Set values from initial_state if provided
  if (initial_state) {
    const float *init_data = (const float *)initial_state;
    s->port0_value = init_data[0]; // Port 0 value
    s->port1_value = init_data[1]; // Port 1 value
  } else {
    s->port0_value = 10.0f; // Default: Port 0 outputs 10.0
    s->port1_value = 15.0f; // Default: Port 1 outputs 15.0
  }
}

static const NodeVTable DUAL_OUTPUT_VTABLE = {.process = dual_output_process,
                                              .init = dual_output_init,
                                              .reset = NULL,
                                              .migrate = NULL};

// Test the specific complex topology from the image:
// - Node 1: 1 input (unconnected), 2 outputs
// - Node 2: 1 input (from Node 1 port 0), 1 output (to DAC and Node 3 port 0)
// - Node 3: 1 input (from Node 1 port 1 and Node 2 port 0), 1 output (to DAC)
// - DAC: receives from Node 2 and Node 3

void test_complex_topology() {
  printf("🧪 Complex Topology Test\n");
  printf("========================\n\n");

  // Initialize graph
  const int block_size = 256;
  LiveGraph *lg = create_live_graph(32, block_size, "complex_topology_test", 1);
  assert(lg != NULL);

  printf("=== Phase 1: Creating Nodes (No Connections) ===\n");

  // Node 1: Custom dual-output node (0 inputs, 2 outputs)
  int node1 = add_node(lg, DUAL_OUTPUT_VTABLE, sizeof(DualOutputState),
                       "dual_node1", 0, 2, NULL, 0);
  assert(node1 >= 0);
  printf("✓ Created Node 1 (DUAL_OUTPUT, port0=10.0, port1=15.0): id=%d\n",
         node1);

  // Node 2: GAIN node with value 2.0 (1 input, 1 output)
  int node2 = live_add_gain(lg, 2.0f, "node2");
  assert(node2 >= 0);
  printf("✓ Created Node 2 (GAIN, value=2.0): id=%d\n", node2);

  // Node 3: GAIN node with value 3.0 (1 input, 1 output)
  int node3 = live_add_gain(lg, 3.0f, "node3");
  assert(node3 >= 0);
  printf("✓ Created Node 3 (GAIN, value=3.0): id=%d\n", node3);

  printf("\n=== Phase 2: Applying Node Creation Edits ===\n");
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✓ Applied all node creation edits\n");

  // Verify initial state
  printf("\n=== Phase 3: Verifying Initial Node State ===\n");
  printf("Node 1 - Inputs: %d, Outputs: %d\n", lg->nodes[node1].nInputs,
         lg->nodes[node1].nOutputs);
  printf("Node 2 - Inputs: %d, Outputs: %d\n", lg->nodes[node2].nInputs,
         lg->nodes[node2].nOutputs);
  printf("Node 3 - Inputs: %d, Outputs: %d\n", lg->nodes[node3].nInputs,
         lg->nodes[node3].nOutputs);
  printf("DAC - Inputs: %d, Outputs: %d\n", lg->nodes[lg->dac_node_id].nInputs,
         lg->nodes[lg->dac_node_id].nOutputs);

  // Verify Node 1 has 2 outputs as expected
  if (lg->nodes[node1].nOutputs != 2) {
    printf("🐛 BUG DETECTED: Node 1 has %d outputs, should have 2\n",
           lg->nodes[node1].nOutputs);
  } else {
    printf("✓ Node 1 correctly has 2 output ports\n");
  }

  // Check initial indegrees
  printf("\nInitial indegrees:\n");
  printf("Node 1 indegree: %d (expected: 0)\n", lg->sched.indegree[node1]);
  printf("Node 2 indegree: %d (expected: 0)\n", lg->sched.indegree[node2]);
  printf("Node 3 indegree: %d (expected: 0)\n", lg->sched.indegree[node3]);
  printf("DAC indegree: %d (expected: 0)\n", lg->sched.indegree[lg->dac_node_id]);

  printf("\n=== Phase 4: Creating Connections ===\n");

  // Connection 1: Node 1 port 0 -> Node 2 port 0
  printf("Connecting Node 1 port 0 -> Node 2 port 0...\n");
  bool conn1 = graph_connect(lg, node1, 0, node2, 0);
  assert(conn1);
  printf("✓ Connection 1 created successfully\n");

  // Connection 2: Node 1 port 1 -> Node 3 port 0
  printf("Connecting Node 1 port 1 -> Node 3 port 0...\n");
  bool conn2 = graph_connect(lg, node1, 1, node3, 0);
  assert(conn2);
  printf("✓ Connection 2 created successfully\n");

  // Check Node 3's state after first connection
  printf("  Node 3 after first connection - inEdgeId[0]: %d\n",
         lg->nodes[node3].inEdgeId[0]);

  // Connection 3: Node 2 port 0 -> Node 3 port 0 (multi-input)
  printf("Connecting Node 2 port 0 -> Node 3 port 0 (multi-input)...\n");
  bool conn3 = graph_connect(lg, node2, 0, node3, 0);
  assert(conn3);
  printf("✓ Connection 3 created successfully\n");

  // Check Node 3's state after second connection
  printf("  Node 3 after second connection - inEdgeId[0]: %d, "
         "fanin_sum_node_id[0]: %d\n",
         lg->nodes[node3].inEdgeId[0], lg->nodes[node3].fanin_sum_node_id[0]);

  // Connection 4: Node 2 port 0 -> DAC port 0
  printf("Connecting Node 2 port 0 -> DAC port 0...\n");
  bool conn4 = apply_connect(lg, node2, 0, lg->dac_node_id, 0);
  assert(conn4);
  printf("✓ Connection 4 created successfully\n");

  // Connection 5: Node 3 port 0 -> DAC port 0 (multi-input)
  printf("Connecting Node 3 port 0 -> DAC port 0 (multi-input)...\n");
  bool conn5 = apply_connect(lg, node3, 0, lg->dac_node_id, 0);
  assert(conn5);
  printf("✓ Connection 5 created successfully\n");

  printf("\n=== Phase 5: Applying Connection Edits ===\n");
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✓ Applied all connection edits\n");

  printf("\n=== Phase 6: Verifying Graph State ===\n");

  // Check final indegrees
  printf("Final indegrees:\n");
  printf("Node 1 indegree: %d (expected: 0)\n", lg->sched.indegree[node1]);
  printf("Node 2 indegree: %d (expected: 1)\n", lg->sched.indegree[node2]);
  printf("Node 3 indegree: %d (expected: 1 with auto-sum)\n",
         lg->sched.indegree[node3]);
  printf("DAC indegree: %d (expected: 1 with auto-sum)\n",
         lg->sched.indegree[lg->dac_node_id]);

  // With auto-summing, destination nodes have indegree=1 (connected to SUM
  // node) The SUM nodes themselves have the multi-input indegrees
  assert(lg->sched.indegree[node1] == 0);
  assert(lg->sched.indegree[node2] == 1);
  assert(lg->sched.indegree[node3] == 1); // Connected to auto-generated SUM node
  assert(lg->sched.indegree[lg->dac_node_id] ==
         1); // Connected to auto-generated SUM node
  printf("✓ All indegrees are correct (accounting for auto-sum behavior)\n");

  // Check edge connections
  printf("\nEdge verification:\n");

  // Node 1 outputs
  printf("Node 1 output connections:\n");
  printf("  Port 0 -> Edge %d\n", lg->nodes[node1].outEdgeId[0]);
  printf("  Port 1 -> Edge %d\n", lg->nodes[node1].outEdgeId[1]);
  assert(lg->nodes[node1].outEdgeId[0] >= 0);
  assert(lg->nodes[node1].outEdgeId[1] >= 0);

  // Node 2 connections
  printf("Node 2 connections:\n");
  printf("  Input port 0 <- Edge %d\n", lg->nodes[node2].inEdgeId[0]);
  printf("  Output port 0 -> Edge %d\n", lg->nodes[node2].outEdgeId[0]);
  assert(lg->nodes[node2].inEdgeId[0] >= 0);
  assert(lg->nodes[node2].outEdgeId[0] >= 0);

  // Node 3 connections (should have SUM node created)
  printf("Node 3 connections:\n");
  printf("  Input port 0 <- Edge %d\n", lg->nodes[node3].inEdgeId[0]);
  printf("  Has fanin_sum_node_id[0]: %d\n",
         lg->nodes[node3].fanin_sum_node_id[0]);
  if (lg->nodes[node3].fanin_sum_node_id[0] >= 0) {
    int sum_node = lg->nodes[node3].fanin_sum_node_id[0];
    printf("  ✓ SUM node %d created for Node 3 multi-input\n", sum_node);
    printf("  SUM node inputs: %d, outputs: %d\n", lg->nodes[sum_node].nInputs,
           lg->nodes[sum_node].nOutputs);
    printf("  SUM node indegree: %d (should be 2)\n", lg->sched.indegree[sum_node]);

    // Verify the SUM node has the correct indegree
    assert(lg->sched.indegree[sum_node] == 2);
    printf("  ✓ SUM node indegree is correct\n");
  } else {
    printf("  ❌ No SUM node created - this is a bug!\n");
  }

  // DAC connections (should have SUM node created)
  printf("DAC connections:\n");
  printf("  Input port 0 <- Edge %d\n", lg->nodes[lg->dac_node_id].inEdgeId[0]);
  printf("  Has fanin_sum_node_id[0]: %d\n",
         lg->nodes[lg->dac_node_id].fanin_sum_node_id[0]);
  if (lg->nodes[lg->dac_node_id].fanin_sum_node_id[0] >= 0) {
    int sum_node = lg->nodes[lg->dac_node_id].fanin_sum_node_id[0];
    printf("  ✓ SUM node %d created for DAC multi-input\n", sum_node);
    printf("  SUM node inputs: %d, outputs: %d\n", lg->nodes[sum_node].nInputs,
           lg->nodes[sum_node].nOutputs);
    printf("  SUM node indegree: %d (should be 2)\n", lg->sched.indegree[sum_node]);

    // Verify the SUM node has the correct indegree
    assert(lg->sched.indegree[sum_node] == 2);
    printf("  ✓ SUM node indegree is correct\n");
  } else {
    printf("  ❌ No SUM node created - this is a bug!\n");
  }

  printf("\n=== Phase 7: Testing Audio Processing ===\n");

  float output_buffer[block_size];

  // Process a block
  process_next_block(lg, output_buffer, block_size);

  float output_value = output_buffer[0];
  printf("Processed block, output value: %.6f\n", output_value);

  // Calculate expected output:
  // Node 1 port 0 outputs 10.0, port 1 outputs 15.0
  // Node 2 receives 10.0 (from Node 1 port 0), multiplies by 2.0 = 20.0
  // Node 3 receives 15.0 (from Node 1 port 1) + 20.0 (from Node 2), sums
  // to 35.0, multiplies by 3.0 = 105.0 DAC receives 20.0 (from Node 2) + 105.0
  // (from Node 3) = 125.0
  float expected_output = 125.0f;

  printf("Expected calculation:\n");
  printf("  Node 1 port 0 -> 10.0, port 1 -> 15.0\n");
  printf("  Node 2 -> 10.0 * 2.0 = 20.0\n");
  printf("  Node 3 -> (15.0 + 20.0) * 3.0 = 105.0\n");
  printf("  DAC -> 20.0 + 105.0 = 125.0\n");
  printf("Expected output: %.6f\n", expected_output);

  bool output_correct = fabs(output_value - expected_output) < 0.001f;
  if (output_correct) {
    printf("✓ Output matches expected value!\n");
  } else {
    printf("✗ Output mismatch! Expected %.6f, got %.6f\n", expected_output,
           output_value);
    printf("Difference: %.6f\n", fabs(output_value - expected_output));
  }

  assert(output_correct);

  printf("\n=== Phase 8: Verifying Graph Invariants ===\n");

  // Check that no nodes are marked as orphaned (since all contribute to DAC)
  printf("Orphan status check:\n");
  printf("Node 1 orphaned: %s\n", lg->sched.is_orphaned[node1] ? "YES" : "NO");
  printf("Node 2 orphaned: %s\n", lg->sched.is_orphaned[node2] ? "YES" : "NO");
  printf("Node 3 orphaned: %s\n", lg->sched.is_orphaned[node3] ? "YES" : "NO");
  printf("DAC orphaned: %s\n", lg->sched.is_orphaned[lg->dac_node_id] ? "YES" : "NO");

  // None should be orphaned since they all contribute to DAC output
  assert(!lg->sched.is_orphaned[node1]);
  assert(!lg->sched.is_orphaned[node2]);
  assert(!lg->sched.is_orphaned[node3]);
  assert(!lg->sched.is_orphaned[lg->dac_node_id]);
  printf("✓ No nodes incorrectly marked as orphaned\n");

  // Check edge capacity (we don't have a count field, but can check capacity)
  printf("Edge capacity in graph: %d\n", lg->edge_capacity);
  printf("✓ Graph topology verification complete\n");

  printf("\n=== Phase 9: Testing Disconnection from Multi-Input ===\n");

  // Process a block to establish baseline
  process_next_block(lg, output_buffer, block_size);
  float baseline_output = output_buffer[0];
  printf("✓ Baseline output with both connections: %.6f\n", baseline_output);

  // Disconnect Node 1 port 1 from Node 3 (remove one input from the SUM)
  printf("Disconnecting Node 1 port 1 -> Node 3 port 0...\n");
  bool disconnect_result = graph_disconnect(lg, node1, 1, node3, 0);
  assert(disconnect_result);
  printf("✓ Disconnection queued successfully\n");

  // Apply the disconnection
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✓ Disconnection applied\n");

  // Process multiple blocks to see if there are artifacts
  printf("Processing blocks after disconnection:\n");
  for (int i = 0; i < 5; i++) {
    process_next_block(lg, output_buffer, block_size);
    float current_output = output_buffer[0];
    printf("  Block %d: %.6f\n", i + 1, current_output);

    // After disconnection, Node 3 should only receive 20.0 from Node 2
    // Expected: Node 3 = 20.0 * 3.0 = 60.0, DAC = 20.0 + 60.0 = 80.0
    float expected_after_disconnect = 80.0f;

    if (i >= 1) { // Allow first block for transition
      bool output_correct =
          fabs(current_output - expected_after_disconnect) < 0.001f;
      if (!output_correct) {
        printf("  🐛 ARTIFACT DETECTED: Expected %.6f, got %.6f (diff: %.6f)\n",
               expected_after_disconnect, current_output,
               fabs(current_output - expected_after_disconnect));
        printf("  This suggests the SUM node is holding stale data from the "
               "disconnected input\n");
      } else if (i == 1) {
        printf("  ✓ Output stabilized to correct value after disconnection\n");
      }
    }
  }

  // Check if SUM node still exists and what its state is
  printf("\nPost-disconnection SUM node analysis:\n");
  if (lg->nodes[node3].fanin_sum_node_id[0] >= 0) {
    int sum_node = lg->nodes[node3].fanin_sum_node_id[0];
    printf("  SUM node %d still exists\n", sum_node);
    printf("  SUM node inputs: %d, outputs: %d\n", lg->nodes[sum_node].nInputs,
           lg->nodes[sum_node].nOutputs);
    printf("  SUM node indegree: %d\n", lg->sched.indegree[sum_node]);

    // Check if SUM node should be removed (only 1 input remaining)
    if (lg->sched.indegree[sum_node] <= 1) {
      printf("  ⚠️  SUM node should potentially be removed (only %d input "
             "remaining)\n",
             lg->sched.indegree[sum_node]);
    }
  } else {
    printf("  SUM node has been removed (fanin_sum_node_id = -1)\n");
    printf("  Node 3 should now have direct connection\n");
  }

  destroy_live_graph(lg);
  printf("\n🎉 Complex topology and disconnection test completed!\n");
}

int main() {
  test_complex_topology();
  return 0;
}
