#include "graph_engine.h"
#include "graph_edit.h"
#include "graph_nodes.h"
#include <assert.h>
#include <stdio.h>
#include <math.h>

void test_sum_behavior() {
  printf("=== Testing Auto-SUM Audio Behavior ===\n");
  
  const int block_size = 64;
  LiveGraph *lg = create_live_graph(32, block_size, "sum_behavior_test", 1);
  assert(lg != NULL);
  
  // Create NUMBER nodes that output constant values
  int num1 = live_add_number(lg, 10.0f, "num1");  // outputs 10.0
  int num2 = live_add_number(lg, 20.0f, "num2");  // outputs 20.0
  int num3 = live_add_number(lg, 5.0f, "num3");   // outputs 5.0
  
  // Create a gain node to receive the summed inputs
  int gain = live_add_gain(lg, 1.0f, "gain");
  
  assert(num1 >= 0 && num2 >= 0 && num3 >= 0 && gain >= 0);
  printf("✓ Created NUMBER nodes: num1=%d(10.0), num2=%d(20.0), num3=%d(5.0), gain=%d\n", 
         num1, num2, num3, gain);
  
  // Apply queued node creations
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied queued node creations\n");
  
  // Debug: Check DAC node
  printf("DEBUG: DAC node ID = %d\n", lg->dac_node_id);
  if (lg->dac_node_id >= 0) {
    RTNode *dac = &lg->nodes[lg->dac_node_id];
    printf("DEBUG: DAC has %d inputs, %d outputs\n", dac->nInputs, dac->nOutputs);
    if (dac->inEdgeId) {
      printf("DEBUG: DAC inEdgeId[0] = %d\n", dac->inEdgeId[0]);
    }
  }
  
  // Debug: Check gain node  
  RTNode *gain_node = &lg->nodes[gain];
  printf("DEBUG: Gain has %d inputs, %d outputs\n", gain_node->nInputs, gain_node->nOutputs);
  
  // Connect gain to DAC so we can read the output
  printf("DEBUG: Attempting to connect gain=%d:0 to DAC=%d:0\n", gain, lg->dac_node_id);
  bool connect_dac = apply_connect(lg, gain, 0, lg->dac_node_id, 0);
  if (!connect_dac) {
    printf("ERROR: Failed to connect gain to DAC\n");
  }
  assert(connect_dac);
  printf("✓ Connected gain to DAC\n");
  
  // Test 1: Single connection (should output 10.0)
  bool connect1 = apply_connect(lg, num1, 0, gain, 0);
  assert(connect1);
  printf("✓ Connected num1(10.0) to gain\n");
  
  // Process a block and check output
  float output_buffer[block_size];
  process_next_block(lg, output_buffer, block_size);
  
  // Verify output is 10.0 (with gain of 1.0)
  float expected1 = 10.0f * 1.0f;
  for (int i = 0; i < 10; i++) { // Check first 10 samples
    assert(fabsf(output_buffer[i] - expected1) < 0.001f);
  }
  printf("✓ Single connection: Output = %.1f (expected %.1f)\n", output_buffer[0], expected1);
  
  // Test 2: Add second connection (should auto-sum to 30.0)
  bool connect2 = apply_connect(lg, num2, 0, gain, 0);
  assert(connect2);
  printf("✓ Connected num2(20.0) to gain (should create auto-SUM)\n");
  
  // Verify SUM was created
  int sum_id = lg->nodes[gain].fanin_sum_node_id[0];
  assert(sum_id >= 0);
  RTNode *sum_node = &lg->nodes[sum_id];
  printf("✓ Auto-SUM created with ID %d\n", sum_id);
  printf("DEBUG: SUM node - nInputs=%d, nOutputs=%d\n", sum_node->nInputs, sum_node->nOutputs);
  printf("DEBUG: SUM inputs: [0]=%d, [1]=%d\n", sum_node->inEdgeId[0], sum_node->inEdgeId[1]);
  printf("DEBUG: SUM output: [0]=%d\n", sum_node->outEdgeId[0]);
  printf("DEBUG: Gain input after SUM: [0]=%d\n", lg->nodes[gain].inEdgeId[0]);
  printf("DEBUG: SUM orphaned status: %s\n", lg->sched.is_orphaned[sum_id] ? "ORPHANED" : "CONNECTED");
  printf("DEBUG: SUM indegree: %d\n", lg->sched.indegree[sum_id]);
  printf("DEBUG: SUM vtable.process: %p\n", (void*)sum_node->vtable.process);
  printf("DEBUG: Gain indegree after SUM: %d\n", lg->sched.indegree[gain]);
  printf("DEBUG: Gain orphaned status: %s\n", lg->sched.is_orphaned[gain] ? "ORPHANED" : "CONNECTED");
  printf("DEBUG: SUM succCount: %d\n", sum_node->succCount);
  if (sum_node->succCount > 0) {
    printf("DEBUG: SUM successor[0]: %d\n", sum_node->succ[0]);
  }
  
  // Check the edge buffer connection
  int sum_output_edge = sum_node->outEdgeId[0];
  int gain_input_edge = lg->nodes[gain].inEdgeId[0];
  printf("DEBUG: SUM output edge=%d, Gain input edge=%d (should match)\n", sum_output_edge, gain_input_edge);
  if (sum_output_edge >= 0) {
    printf("DEBUG: SUM edge buffer first sample = %.3f (buffer addr=%p)\n", 
           lg->edges[sum_output_edge].buf[0], (void*)lg->edges[sum_output_edge].buf);
  }
  
  // Process block and verify sum
  process_next_block(lg, output_buffer, block_size);
  
  // Check edge buffer AFTER processing
  if (sum_output_edge >= 0) {
    printf("DEBUG: SUM edge buffer AFTER processing = %.3f\n", lg->edges[sum_output_edge].buf[0]);
  }
  
  float expected2 = (10.0f + 20.0f) * 1.0f; // 30.0
  printf("DEBUG: Two connections - actual output = %.6f, expected = %.6f\n", output_buffer[0], expected2);
  for (int i = 0; i < 10; i++) {
    if (fabsf(output_buffer[i] - expected2) >= 0.001f) {
      printf("ERROR: Sample %d: got %.6f, expected %.6f (diff=%.6f)\n", 
             i, output_buffer[i], expected2, fabsf(output_buffer[i] - expected2));
    }
    assert(fabsf(output_buffer[i] - expected2) < 0.001f);
  }
  printf("✓ Two connections: Output = %.1f (expected %.1f)\n", output_buffer[0], expected2);
  
  // Test 3: Add third connection (should grow SUM to 35.0)
  bool connect3 = apply_connect(lg, num3, 0, gain, 0);
  assert(connect3);
  printf("✓ Connected num3(5.0) to gain (should grow SUM)\n");
  
  // Verify SUM was grown
  assert(lg->nodes[sum_id].nInputs == 3);
  printf("✓ SUM grown to 3 inputs\n");
  
  // Process block and verify three-way sum
  process_next_block(lg, output_buffer, block_size);
  float expected3 = (10.0f + 20.0f + 5.0f) * 1.0f; // 35.0
  for (int i = 0; i < 10; i++) {
    assert(fabsf(output_buffer[i] - expected3) < 0.001f);
  }
  printf("✓ Three connections: Output = %.1f (expected %.1f)\n", output_buffer[0], expected3);
  
  // Test 4: Remove middle connection (should sum to 15.0)
  bool disconnect2 = apply_disconnect(lg, num2, 0, gain, 0);
  assert(disconnect2);
  printf("✓ Disconnected num2(20.0) from gain\n");
  
  // Process block and verify two-way sum without num2
  process_next_block(lg, output_buffer, block_size);
  float expected4 = (10.0f + 5.0f) * 1.0f; // 15.0
  for (int i = 0; i < 10; i++) {
    assert(fabsf(output_buffer[i] - expected4) < 0.001f);
  }
  printf("✓ After disconnect: Output = %.1f (expected %.1f)\n", output_buffer[0], expected4);
  
  // Test 5: Remove another connection (should collapse to single)
  bool disconnect3 = apply_disconnect(lg, num3, 0, gain, 0);
  assert(disconnect3);
  printf("✓ Disconnected num3(5.0) from gain (should collapse SUM)\n");
  
  // Verify SUM collapsed
  assert(lg->nodes[gain].fanin_sum_node_id[0] == -1);
  printf("✓ SUM collapsed back to direct connection\n");
  
  // Process block and verify single connection
  process_next_block(lg, output_buffer, block_size);
  float expected5 = 10.0f * 1.0f; // 10.0
  for (int i = 0; i < 10; i++) {
    assert(fabsf(output_buffer[i] - expected5) < 0.001f);
  }
  printf("✓ After collapse: Output = %.1f (expected %.1f)\n", output_buffer[0], expected5);
  
  // Test 6: Test with different gain value
  // Change gain to 2.0
  float *gain_memory = (float *)gain_node->state;
  gain_memory[GAIN_VALUE] = 2.0f;
  
  process_next_block(lg, output_buffer, block_size);
  float expected6 = 10.0f * 2.0f; // 20.0
  for (int i = 0; i < 10; i++) {
    assert(fabsf(output_buffer[i] - expected6) < 0.001f);
  }
  printf("✓ With gain=2.0: Output = %.1f (expected %.1f)\n", output_buffer[0], expected6);
  
  // Test 7: Re-add multiple connections with new gain
  bool reconnect2 = apply_connect(lg, num2, 0, gain, 0);
  bool reconnect3 = apply_connect(lg, num3, 0, gain, 0);
  assert(reconnect2 && reconnect3);
  printf("✓ Reconnected num2 and num3 to gain\n");
  
  // Verify SUM recreated
  sum_id = lg->nodes[gain].fanin_sum_node_id[0];
  assert(sum_id >= 0);
  assert(lg->nodes[sum_id].nInputs == 3);
  printf("✓ SUM recreated with 3 inputs\n");
  
  // Process and verify with gain=2.0
  process_next_block(lg, output_buffer, block_size);
  float expected7 = (10.0f + 20.0f + 5.0f) * 2.0f; // 70.0
  for (int i = 0; i < 10; i++) {
    assert(fabsf(output_buffer[i] - expected7) < 0.001f);
  }
  printf("✓ Final test: Output = %.1f (expected %.1f)\n", output_buffer[0], expected7);
  
  destroy_live_graph(lg);
  printf("=== Auto-SUM Audio Behavior Test Completed Successfully ===\n\n");
}

int main() {
  initialize_engine(64, 48000);
  test_sum_behavior();
  return 0;
}