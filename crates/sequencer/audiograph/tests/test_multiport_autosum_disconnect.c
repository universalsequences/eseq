#include "../graph_engine.h"
#include "../graph_nodes.h"
#include "../graph_edit.h"
#include <assert.h>
#include <stdio.h>

// Test case: Connect two edges to same input (auto-sum) then disconnect one 
// This was the specific case that caused dropout with partial Option A fixes
int main() {
  printf("=== Testing Multi-Port Auto-Sum Disconnection ===\n");
  
  // Create a live graph
  LiveGraph *lg = create_live_graph(16, 128, "test_multiport_autosum", 1);
  assert(lg);

  // Add DAC manually (normally done by system)
  lg->dac_node_id = apply_add_node(lg, DAC_VTABLE, 0, 999, "DAC", 1, 1, NULL);
  assert(lg->dac_node_id >= 0);

  // Create two number nodes so we can connect both outputs to same input
  int num_node1 = live_add_number(lg, 5.0f, "num1");
  int num_node2 = live_add_number(lg, 5.0f, "num2");
  assert(num_node1 >= 0 && num_node2 >= 0);
  printf("✓ Created number nodes: id=%d and id=%d (both output 5.0)\n", num_node1, num_node2);
  
  // Create a single-input gain node  
  int gain_node = live_add_gain(lg, 2.0f, "gain");
  assert(gain_node >= 0);
  printf("✓ Created gain node: id=%d (gain=2.0)\n", gain_node);
  
  // Apply all node creations
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied queued node creations\n");
  
  // Connect gain to DAC
  bool connect_dac = apply_connect(lg, gain_node, 0, lg->dac_node_id, 0);
  assert(connect_dac);
  printf("✓ Connected gain to DAC\n");
  
  // Process initial state (should be 0)
  float output_buffer[128];
  process_next_block(lg, output_buffer, 128);
  printf("✓ Initial output: %.3f (expected 0.0)\n", output_buffer[0]);
  assert(output_buffer[0] == 0.0f);
  
  // Now the critical test: Connect BOTH num nodes to the SAME input of gain_node
  // This should create an auto-SUM node with 2 inputs
  printf("\n--- Connecting both nodes to same input (creates auto-SUM) ---\n");
  
  bool connect1 = apply_connect(lg, num_node1, 0, gain_node, 0);  // num1:0 → gain:0 
  assert(connect1);
  printf("✓ Connected num_node1:0 → gain_node:0\n");
  
  bool connect2 = apply_connect(lg, num_node2, 0, gain_node, 0);  // num2:0 → gain:0 (auto-sum!)
  assert(connect2);
  printf("✓ Connected num_node2:0 → gain_node:0 (should create auto-SUM)\n");
  
  // Verify SUM was created  
  int sum_id = lg->nodes[gain_node].fanin_sum_node_id[0];
  assert(sum_id != -1);
  RTNode *sum_node = &lg->nodes[sum_id];
  printf("✓ Auto-SUM created with ID=%d, %d inputs\n", sum_id, sum_node->nInputs);
  assert(sum_node->nInputs == 2);
  
  // Debug the indegree values
  printf("DEBUG: After creating SUM:\n");
  printf("  num_node1 indegree: %d\n", lg->sched.indegree[num_node1]);
  printf("  num_node2 indegree: %d\n", lg->sched.indegree[num_node2]);
  printf("  sum_node indegree: %d\n", lg->sched.indegree[sum_id]); 
  printf("  gain_node indegree: %d\n", lg->sched.indegree[gain_node]);
  
  // Process - should get 5.0 + 5.0 = 10.0, then gain of 2.0 = 20.0
  process_next_block(lg, output_buffer, 128);
  printf("✓ Two connections: Output = %.3f (expected 20.0)\n", output_buffer[0]);
  assert(output_buffer[0] == 20.0f);
  
  printf("\n--- Disconnecting one edge (the critical test case) ---\n");
  
  // This is the operation that used to cause dropout!
  // Disconnect num_node2:0 from gain_node:0 (removes one input from SUM)
  bool disconnect = apply_disconnect(lg, num_node2, 0, gain_node, 0);
  assert(disconnect);
  printf("✓ Disconnected num_node2:0 from gain_node:0\n");
  
  // Note: SUM should be collapsed to direct connection (optimization)
  // Verify the SUM was collapsed and replaced with direct connection
  if (lg->nodes[gain_node].fanin_sum_node_id[0] == -1) {
    printf("✓ SUM was collapsed to direct connection (optimization)\n");
  } else {
    printf("✓ SUM still exists with %d inputs\n", lg->nodes[lg->nodes[gain_node].fanin_sum_node_id[0]].nInputs);
  }
  
  // Debug the indegree values after disconnect
  printf("DEBUG: After disconnect:\n");
  printf("  num_node1 indegree: %d\n", lg->sched.indegree[num_node1]);
  printf("  num_node2 indegree: %d\n", lg->sched.indegree[num_node2]);
  printf("  gain_node indegree: %d\n", lg->sched.indegree[gain_node]);
  
  // Only print sum_node indegree if SUM still exists
  if (lg->nodes[gain_node].fanin_sum_node_id[0] != -1) {
    printf("  sum_node indegree: %d\n", lg->sched.indegree[sum_id]);
  } else {
    printf("  sum_node: collapsed (no longer exists)\n");
  }
  
  // Process - should get 5.0, then gain of 2.0 = 10.0
  // If this outputs 0.0, the bug is present (scheduling dropout)
  process_next_block(lg, output_buffer, 128);
  printf("✓ After disconnect: Output = %.3f (expected 10.0)\n", output_buffer[0]);
  
  if (output_buffer[0] == 0.0f) {
    printf("❌ DROPOUT DETECTED! The bug is still present.\n");
    printf("   This means indegree/pending are still out of sync.\n");
    return 1;
  } else if (output_buffer[0] == 10.0f) {
    printf("✅ No dropout - disconnect worked correctly!\n");
  } else {
    printf("❌ Unexpected output value: %.3f\n", output_buffer[0]);
    return 1;
  }
  
  // Test multiple blocks to ensure stability
  printf("\n--- Testing stability over multiple blocks ---\n");
  for (int i = 0; i < 5; i++) {
    process_next_block(lg, output_buffer, 128);
    if (output_buffer[0] != 10.0f) {
      printf("❌ Instability detected at block %d: %.3f\n", i, output_buffer[0]);
      return 1;
    }
  }
  printf("✅ Stable over 5 blocks\n");
  
  // Cleanup
  destroy_live_graph(lg);
  printf("=== Multi-Port Auto-Sum Disconnect Test Completed Successfully ===\n");
  
  return 0;
}