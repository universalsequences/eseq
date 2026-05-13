#include <assert.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"

// Custom node states
typedef struct {
  float value;
} NumberGenState;

typedef struct {
  float value1;
  float value2;
} DualOutputState;

typedef struct {
  // No state needed for multiplier
  float dummy;
} MultiplierState;

// Custom node process functions
static void number_gen_process(float *const *inputs, float *const *outputs,
                               int block_size, void *state, void *buffers) {
  (void)inputs; // Unused
  NumberGenState *s = (NumberGenState *)state;

  for (int i = 0; i < block_size; i++) {
    outputs[0][i] = s->value;
  }
}

static void dual_output_process(float *const *inputs, float *const *outputs,
                                int block_size, void *state, void *buffers) {
  (void)inputs; // Unused
  DualOutputState *s = (DualOutputState *)state;

  for (int i = 0; i < block_size; i++) {
    outputs[0][i] = s->value1;
    outputs[1][i] = s->value2;
  }
}

static void multiplier_process(float *const *inputs, float *const *outputs,
                               int block_size, void *state, void *buffers) {
  (void)state; // Unused

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
  if (initial_state) {
    const float *init_data = (const float *)initial_state;
    s->value = init_data[0];
  } else {
    s->value = 1.0f; // Default value
  }
}

static void dual_output_init(void *state, int sr, int mb,
                             const void *initial_state) {
  (void)sr;
  (void)mb;
  DualOutputState *s = (DualOutputState *)state;
  if (initial_state) {
    const float *init_data = (const float *)initial_state;
    s->value1 = init_data[0];
    s->value2 = init_data[1];
  } else {
    s->value1 = 2.0f; // Default values
    s->value2 = 3.0f;
  }
}

static void multiplier_init(void *state, int sr, int mb,
                            const void *initial_state) {
  (void)sr;
  (void)mb;
  (void)state;
  (void)initial_state;
  // No initialization needed
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

int main() {
  printf("Starting 4-node topology test...\n");

  // Initialize graph
  const int block_size = 256;
  LiveGraph *lg = create_live_graph(32, block_size, "4_node_topology_test", 1);
  assert(lg != NULL);

  printf("\n=== Adding Nodes ===\n");

  // Node 1: Simple number generator (generates 1)
  int node1_id = add_node(lg, NUMBER_GEN_VTABLE, sizeof(NumberGenState),
                          "number_gen", 0, 1, NULL, 0);
  printf("Node 1 (number gen=1): id=%d\n", node1_id);
  assert(node1_id >= 0);

  // Node 2: Dual output number generator (generates 2 and 3)
  int node2_id = add_node(lg, DUAL_OUTPUT_VTABLE, sizeof(DualOutputState),
                          "dual_output", 0, 2, NULL, 0);
  printf("Node 2 (dual output=2,3): id=%d\n", node2_id);
  assert(node2_id >= 0);

  // Node 3: 2-input/1-output multiplier
  int node3_id = add_node(lg, MULTIPLIER_VTABLE, sizeof(MultiplierState),
                          "multiplier", 2, 1, NULL, 0);
  printf("Node 3 (2-input multiplier): id=%d\n", node3_id);
  assert(node3_id >= 0);

  // Node 4: Gain node (gain = 0.5)
  int node4_id = live_add_gain(lg, 0.5f, "gain");
  printf("Node 4 (gain=0.5): id=%d\n", node4_id);
  assert(node4_id >= 0);

  // Apply node creation edits first
  printf("\n=== Applying Node Creation Edits ===\n");
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✓ Applied node creation edits\n");

  // Debug: Check node port counts
  printf("\n=== Node Port Verification ===\n");
  printf("Node 1 - Inputs: %d, Outputs: %d\n", lg->nodes[node1_id].nInputs,
         lg->nodes[node1_id].nOutputs);
  printf("Node 2 - Inputs: %d, Outputs: %d\n", lg->nodes[node2_id].nInputs,
         lg->nodes[node2_id].nOutputs);
  printf("Node 3 - Inputs: %d, Outputs: %d\n", lg->nodes[node3_id].nInputs,
         lg->nodes[node3_id].nOutputs);
  printf("Node 4 - Inputs: %d, Outputs: %d\n", lg->nodes[node4_id].nInputs,
         lg->nodes[node4_id].nOutputs);
  printf("DAC - Inputs: %d, Outputs: %d\n", lg->nodes[lg->dac_node_id].nInputs,
         lg->nodes[lg->dac_node_id].nOutputs);

  printf("\n=== Setting up Connections ===\n");

  // Connection topology:
  // Node 1 output 0 -> Node 3 input 0
  // Node 1 output 0 -> Node 4 input 0
  // Node 2 output 0 -> Node 3 input 1
  // Node 2 output 1 -> DAC
  // Node 3 output 0 -> DAC
  // Node 4 output 0 -> DAC

  printf("Connecting Node 1 output 0 -> Node 3 input 0\n");
  bool conn1 = graph_connect(lg, node1_id, 0, node3_id, 0);
  assert(conn1);

  printf("Connecting Node 1 output 0 -> Node 4 input 0\n");
  bool conn2 = graph_connect(lg, node1_id, 0, node4_id, 0);
  assert(conn2);

  printf("Connecting Node 2 output 0 -> Node 3 input 1\n");
  bool conn3 = graph_connect(lg, node2_id, 0, node3_id, 1);
  assert(conn3);

  printf("Connecting Node 2 output 1 -> DAC\n");
  bool conn4 = apply_connect(lg, node2_id, 1, lg->dac_node_id, 0);
  assert(conn4);

  printf("Connecting Node 3 output 0 -> DAC\n");
  bool conn5 = apply_connect(lg, node3_id, 0, lg->dac_node_id, 0);
  assert(conn5);

  printf("Connecting Node 4 output 0 -> DAC\n");
  bool conn6 = apply_connect(lg, node4_id, 0, lg->dac_node_id, 0);
  assert(conn6);

  printf("\n=== Processing Initial Audio ===\n");

  // Apply connection edits
  printf("\n=== Applying Connection Edits ===\n");
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✓ Applied all connection edits\n");

  // Process some audio to verify initial state
  float output_buffer[block_size];
  memset(output_buffer, 0, sizeof(output_buffer));

  process_next_block(lg, output_buffer, block_size);

  // Expected output calculation:
  // Node 1 generates: 1
  // Node 2 generates: 2, 3
  // Node 3 computes: 1 * 2 = 2
  // Node 4 computes: 1 * 0.5 = 0.5
  // DAC receives: Node 2 output 1 (3) + Node 3 output 0 (2) + Node 4 output 0
  // (0.5) = 5.5

  float expected_dac = 3.0f + 2.0f + 0.5f; // 5.5
  printf("Initial DAC output: %.2f (expected: %.2f)\n", output_buffer[0],
         expected_dac);

  // Verify within tolerance
  if (fabs(output_buffer[0] - expected_dac) < 0.001f) {
    printf("✓ Initial DAC output is correct\n");
  } else {
    printf("✗ Initial DAC output is incorrect!\n");
    return 1;
  }

  printf("\n=== Deleting Edge: Node 1 -> Node 4 ===\n");

  // Delete the connection from Node 1 to Node 4
  printf("Queuing disconnect: Node 1 output 0 -> Node 4 input 0\n");
  bool disconn1 = graph_disconnect(lg, node1_id, 0, node4_id, 0);
  assert(disconn1);
  printf("✓ Disconnect queued successfully\n");

  // Apply the disconnection edit
  printf("Applying disconnect edit...\n");
  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✓ Disconnect edit applied\n");

  printf("\n=== Processing Audio After Edge Deletion ===\n");

  // Process audio again after deletion
  memset(output_buffer, 0, sizeof(output_buffer));

  process_next_block(lg, output_buffer, block_size);

  // Expected output after deletion:
  // Node 1 still generates: 1 (but no longer connected to Node 4)
  // Node 2 still generates: 2, 3
  // Node 3 still computes: 1 * 2 = 2 (Node 1 still connected to Node 3)
  // Node 4 now computes: 0 * 0.5 = 0 (no input, should be 0)
  // DAC receives: Node 2 output 1 (3) + Node 3 output 0 (2) + Node 4 output 0
  // (0) = 5.0

  float expected_dac_after = 3.0f + 2.0f + 0.0f; // 5.0
  printf("DAC output after deletion: %.2f (expected: %.2f)\n", output_buffer[0],
         expected_dac_after);

  // Verify within tolerance
  if (fabs(output_buffer[0] - expected_dac_after) < 0.001f) {
    printf("✓ DAC output after deletion is correct\n");
  } else {
    printf(
        "✗ DAC output after deletion is incorrect! This indicates the bug!\n");
    printf("  Difference: %.6f\n", output_buffer[0] - expected_dac_after);
  }

  printf("\n=== Test Summary ===\n");
  printf("This test reproduces the topology:\n");
  printf("Node 1 (gen=1) -> Node 3 (mult) -> DAC\n");
  printf("Node 1 (gen=1) -> Node 4 (gain=0.5) -> DAC [DELETED]\n");
  printf("Node 2 (gen=2,3) -> Node 3 (mult) -> DAC\n");
  printf("Node 2 (gen=2,3) -> DAC\n");
  printf("Expected behavior: After deleting Node 1->Node 4, DAC should output "
         "5.0\n");

  // Cleanup
  destroy_live_graph(lg);
  printf("\nTest completed.\n");

  return (fabs(output_buffer[0] - expected_dac_after) < 0.001f) ? 0 : 1;
}
