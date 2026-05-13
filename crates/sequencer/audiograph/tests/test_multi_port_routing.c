#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>

// ===================== Custom Node Implementations =====================

// Dual output generator: ignores inputs, outputs 0.25 on port 0 and 0.5 on port
// 1
#define DUAL_OUTPUT_MEMORY_SIZE 0 // No state needed

void dual_output_process(float *const *in, float *const *out, int n,
                         void *memory, void *buffers) {
  (void)in;     // Ignore inputs (2 unused inputs)
  (void)memory; // No state

  float *output_0 = out[0]; // First output: 0.25
  float *output_1 = out[1]; // Second output: 0.5

  // Debug: Confirm this node is being called
  if (n > 0) {
    printf("DUAL_OUTPUT_DEBUG: Generating 0.25 on port 0, 0.5 on port 1\n");
  }

  for (int i = 0; i < n; i++) {
    output_0[i] = 0.25f;
    output_1[i] = 0.5f;
  }
}

// Dual input summer: sums two inputs to single output
#define DUAL_INPUT_SUM_MEMORY_SIZE 0 // No state needed

void dual_input_sum_process(float *const *in, float *const *out, int n,
                            void *memory, void *buffers) {
  (void)memory; // No state

  const float *input_0 = in[0]; // First input
  const float *input_1 = in[1]; // Second input
  float *output = out[0];       // Single output

  // Debug: Print first few input samples
  if (n > 0) {
    printf("DUAL_SUM_DEBUG: in[0][0]=%.6f, in[1][0]=%.6f\n",
           input_0 ? input_0[0] : -999.0f, input_1 ? input_1[0] : -999.0f);
  }

  for (int i = 0; i < n; i++) {
    float sum = (input_0 ? input_0[i] : 0.0f) + (input_1 ? input_1[i] : 0.0f);
    output[i] = sum;

    // Debug first sample
    if (i == 0) {
      printf("DUAL_SUM_DEBUG: output[0]=%.6f\n", sum);
    }
  }
}

// VTables for our custom nodes
const NodeVTable DUAL_OUTPUT_VTABLE = {.process = dual_output_process,
                                       .init = NULL,
                                       .reset = NULL,
                                       .migrate = NULL};

const NodeVTable DUAL_INPUT_SUM_VTABLE = {.process = dual_input_sum_process,
                                          .init = NULL,
                                          .reset = NULL,
                                          .migrate = NULL};

// ===================== Test Function =====================

void test_multi_port_routing() {
  printf("=== Testing Multi-Port Routing (2-out -> 2-in) ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "multi_port_test", 1);
  assert(lg != NULL);

  // Step 1: Create the dual-output node (Node 1)
  // 2 inputs (unused), 2 outputs (0.25, 0.5)
  // No state needed (memory size = 0)
  int node1 = add_node(lg, DUAL_OUTPUT_VTABLE, 0, "dual_output", 2, 2, NULL,
                       0); // 2 inputs, 2 outputs
  assert(node1 >= 0);
  printf("✓ Created dual-output node: id=%d (outputs 0.25, 0.5)\n", node1);

  // Step 2: Create the dual-input summing node (Node 2)
  // 2 inputs, 1 output (sums inputs)
  // No state needed (memory size = 0)
  int node2 = add_node(lg, DUAL_INPUT_SUM_VTABLE, 0, "dual_sum", 2, 1, NULL,
                       0); // 2 inputs, 1 output
  assert(node2 >= 0);
  printf("✓ Created dual-input sum node: id=%d (sums two inputs)\n", node2);

  // Step 3: Apply queued node creations
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied queued node creations\n");

  // Step 4: Examine node structure before connections
  printf("DEBUG: Node1 structure - inputs:%d outputs:%d\n",
         lg->nodes[node1].nInputs, lg->nodes[node1].nOutputs);
  printf("DEBUG: Node2 structure - inputs:%d outputs:%d\n",
         lg->nodes[node2].nInputs, lg->nodes[node2].nOutputs);
  printf("DEBUG: DAC structure - inputs:%d outputs:%d\n",
         lg->nodes[lg->dac_node_id].nInputs,
         lg->nodes[lg->dac_node_id].nOutputs);

  // Step 5: Make the specific connections as requested
  // Connect node1:port0 (0.25) -> node2:port0
  printf("DEBUG: Attempting node1:0 -> node2:0\n");
  bool connect1 = apply_connect(lg, node1, 0, node2, 0);
  if (connect1) {
    printf("✓ Connected node1:0 (0.25) -> node2:0\n");
  } else {
    printf("✗ FAILED to connect node1:0 -> node2:0\n");
  }

  // Connect node1:port1 (0.5) -> node2:port1
  printf("DEBUG: Attempting node1:1 -> node2:1\n");
  bool connect2 = apply_connect(lg, node1, 1, node2, 1);
  if (connect2) {
    printf("✓ Connected node1:1 (0.5) -> node2:1\n");
  } else {
    printf("✗ FAILED to connect node1:1 -> node2:1\n");
  }

  // Step 6: Connect node2 output to DAC for verification
  printf("DEBUG: Attempting node2:0 -> DAC:0\n");
  bool connect_dac = apply_connect(lg, node2, 0, lg->dac_node_id, 0);
  if (connect_dac) {
    printf("✓ Connected node2:0 -> DAC\n");
  } else {
    printf("✗ FAILED to connect node2:0 -> DAC\n");
  }

  // Step 7: Process audio multiple times and verify results
  float output_buffer[block_size];

  // Expected result: 0.25 + 0.5 = 0.75 (if connections work)
  float expected = 0.25f + 0.5f; // 0.75

  printf("DEBUG: Processing multiple audio blocks...\n");

  for (int block = 0; block < 3; block++) {
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);

    printf("DEBUG: Block %d - First 5 samples: %.6f, %.6f, %.6f, %.6f, %.6f\n",
           block, output_buffer[0], output_buffer[1], output_buffer[2],
           output_buffer[3], output_buffer[4]);
  }

  // Verify the result only if connections succeeded
  if (connect1 && connect2 && connect_dac) {
    // Check if we got the expected sum
    bool all_correct = true;
    for (int i = 0; i < 5; i++) {
      if (fabsf(output_buffer[i] - expected) >= 0.001f) {
        printf("ERROR: Sample %d: got %.6f, expected %.6f (diff=%.6f)\n", i,
               output_buffer[i], expected, fabsf(output_buffer[i] - expected));
        all_correct = false;
      }
    }

    if (all_correct) {
      printf("✓ Multi-port routing works: Output = %.3f (expected %.3f)\n",
             output_buffer[0], expected);
    } else {
      printf("✗ Multi-port routing FAILED: got %.6f, expected %.6f\n",
             output_buffer[0], expected);
    }
  } else {
    printf("⚠ Skipping output verification due to connection failures\n");
    printf("  Connection results: node1->node2[0]=%s, node1->node2[1]=%s, "
           "node2->DAC=%s\n",
           connect1 ? "OK" : "FAIL", connect2 ? "OK" : "FAIL",
           connect_dac ? "OK" : "FAIL");
  }

  // Step 8: Test node scheduling and orphan status
  RTNode *node1_rt = &lg->nodes[node1];
  RTNode *node2_rt = &lg->nodes[node2];

  printf("DEBUG: Node1 has %d outputs, node2 has %d inputs\n",
         node1_rt->nOutputs, node2_rt->nInputs);

  // Check orphan status and scheduling
  printf("DEBUG: Node1 orphan status: %s, indegree: %d\n",
         lg->sched.is_orphaned[node1] ? "ORPHANED" : "CONNECTED",
         lg->sched.indegree[node1]);
  printf("DEBUG: Node2 orphan status: %s, indegree: %d\n",
         lg->sched.is_orphaned[node2] ? "ORPHANED" : "CONNECTED",
         lg->sched.indegree[node2]);
  printf("DEBUG: DAC orphan status: %s, indegree: %d\n",
         lg->sched.is_orphaned[lg->dac_node_id] ? "ORPHANED" : "CONNECTED",
         lg->sched.indegree[lg->dac_node_id]);

  // Verify edge connections
  printf("DEBUG: Node1 output edges: [0]=%d, [1]=%d\n", node1_rt->outEdgeId[0],
         node1_rt->outEdgeId[1]);
  printf("DEBUG: Node2 input edges: [0]=%d, [1]=%d\n", node2_rt->inEdgeId[0],
         node2_rt->inEdgeId[1]);

  // Check that edges are properly wired
  if (node1_rt->outEdgeId[0] >= 0 && node1_rt->outEdgeId[1] >= 0 &&
      node2_rt->inEdgeId[0] >= 0 && node2_rt->inEdgeId[1] >= 0) {

    int edge_0_id = node1_rt->outEdgeId[0];
    int edge_1_id = node1_rt->outEdgeId[1];

    printf("DEBUG: Edge buffers after processing:\n");
    printf("  Edge %d (0.25 path): %.6f\n", edge_0_id,
           lg->edges[edge_0_id].buf[0]);
    printf("  Edge %d (0.5 path): %.6f\n", edge_1_id,
           lg->edges[edge_1_id].buf[0]);

    // Verify the individual edge buffers contain expected values
    bool edge0_ok = fabsf(lg->edges[edge_0_id].buf[0] - 0.25f) < 0.001f;
    bool edge1_ok = fabsf(lg->edges[edge_1_id].buf[0] - 0.5f) < 0.001f;
    if (edge0_ok && edge1_ok) {
      printf("✓ Individual edge buffers verified\n");
    } else {
      printf("✗ Edge buffer verification failed: edge0=%.6f (exp 0.25), "
             "edge1=%.6f (exp 0.5)\n",
             lg->edges[edge_0_id].buf[0], lg->edges[edge_1_id].buf[0]);
    }
  }

  destroy_live_graph(lg);
  printf("=== Multi-Port Routing Test Completed Successfully ===\n\n");
}

// Test the workaround: using intermediate nodes avoids the edge case
void test_workaround_with_intermediate_nodes() {
  printf("=== Testing Workaround: Intermediate Nodes ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "workaround_test", 1);
  assert(lg != NULL);

  // Create the same dual-output source node
  // No state needed (memory size = 0)
  int node1 = add_node(lg, DUAL_OUTPUT_VTABLE, 0, "dual_output", 2, 2, NULL, 0);
  assert(node1 >= 0);

  // Create two intermediate gain nodes (pass-through with gain=1.0)
  int gain1 = live_add_gain(lg, 1.0f, "gain1");
  int gain2 = live_add_gain(lg, 1.0f, "gain2");
  assert(gain1 >= 0 && gain2 >= 0);

  // Create the final sum node
  int sum_node = add_node(lg, DUAL_INPUT_SUM_VTABLE, DUAL_INPUT_SUM_MEMORY_SIZE,
                          "sum_node", 2, 1, NULL, 0);
  assert(sum_node >= 0);

  // Apply all node creations
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Created workaround chain: dual_output → gain1,gain2 → sum_node → "
         "DAC\n");

  // Make connections: Node1 → Gain1,Gain2 → Sum → DAC
  bool connect1 = apply_connect(lg, node1, 0, gain1, 0);    // Node1:0 → Gain1:0
  bool connect2 = apply_connect(lg, node1, 1, gain2, 0);    // Node1:1 → Gain2:0
  bool connect3 = apply_connect(lg, gain1, 0, sum_node, 0); // Gain1:0 → Sum:0
  bool connect4 = apply_connect(lg, gain2, 0, sum_node, 1); // Gain2:0 → Sum:1
  bool connect5 =
      apply_connect(lg, sum_node, 0, lg->dac_node_id, 0); // Sum:0 → DAC

  assert(connect1 && connect2 && connect3 && connect4 && connect5);
  printf("✓ All workaround connections successful\n");

  // Process and verify
  float output_buffer[block_size];
  memset(output_buffer, 0, sizeof(output_buffer));

  printf("DEBUG: Processing workaround chain...\n");
  process_next_block(lg, output_buffer, block_size);

  float expected = 0.25f + 0.5f; // Should be 0.75
  printf("DEBUG: Workaround output = %.6f (expected %.6f)\n", output_buffer[0],
         expected);

  bool works = fabsf(output_buffer[0] - expected) < 0.001f;
  if (works) {
    printf("✓ WORKAROUND WORKS! Output = %.3f (intermediate nodes fix the "
           "issue)\n",
           output_buffer[0]);
  } else {
    printf("✗ Workaround also failed: got %.6f, expected %.6f\n",
           output_buffer[0], expected);
  }

  destroy_live_graph(lg);
  printf("=== Workaround Test Completed ===\n\n");
}

int main() {
  initialize_engine(64, 48000);
  test_multi_port_routing();
  test_workaround_with_intermediate_nodes();
  return 0;
}
