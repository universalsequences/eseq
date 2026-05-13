#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <stdio.h>

int main() {
  printf("=== Queue API Test ===\n");
  printf("Testing pre-allocated ID API with failure tracking\n\n");

  // Create a test graph
  LiveGraph *lg = create_live_graph(10, 128, "queue_api_test", 1);
  assert(lg != NULL);

  printf("✓ LiveGraph created with auto-DAC (ID: %d)\n", lg->dac_node_id);

  // Test 1: Normal add_node operations with immediate ID return
  printf("\nTest 1: Add nodes with immediate IDs...\n");

  // Memory is now allocated by the library
  int osc_id = add_node(lg, OSC_VTABLE, OSC_MEMORY_SIZE * sizeof(float),
                        "test_osc", 0, 1, NULL, 0); // Osc: 0 inputs, 1 output
  int gain_id = add_node(lg, GAIN_VTABLE, GAIN_MEMORY_SIZE * sizeof(float),
                         "test_gain", 1, 1, NULL, 0); // Gain: 1 input, 1 output

  assert(osc_id > 0);        // Should get immediate logical ID
  assert(gain_id > 0);       // Should get immediate logical ID
  assert(osc_id != gain_id); // Should be different

  printf("✓ Got immediate IDs: osc=%d, gain=%d\n", osc_id, gain_id);

  // Test 2: Connect using pre-allocated IDs (before they're processed)
  printf("\nTest 2: Connect using pre-allocated IDs...\n");

  bool connect_result = graph_connect(lg, osc_id, 0, gain_id, 0);
  assert(connect_result == true);
  printf("✓ Connected osc->gain using pre-allocated IDs\n");

  bool connect_to_dac = graph_connect(lg, gain_id, 0, lg->dac_node_id, 0);
  assert(connect_to_dac == true);
  printf("✓ Connected gain->DAC\n");

  // Test 3: Apply edits and verify everything was created
  printf("\nTest 3: Apply queued operations...\n");
  printf("  Initial node count: %d\n", lg->node_count);
  printf("  Node capacity: %d\n", lg->node_capacity);

  int initial_count = lg->node_count;
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);

  printf("  Apply result: %s\n", apply_result ? "SUCCESS" : "FAILED");
  printf("  Final node count: %d\n", lg->node_count);

  if (!apply_result) {
    printf("  Failed IDs count: %d\n", lg->failed_ids_count);
    for (int i = 0; i < lg->failed_ids_count; i++) {
      printf("    Failed ID: %llu\n", lg->failed_ids[i]);
    }
  }

  assert(apply_result == true);

  printf("✓ Applied all queued edits successfully\n");
  printf("  Node count: %d -> %d\n", initial_count, lg->node_count);

  // Test 4: Verify is_failed_node works
  printf("\nTest 4: Test failed node detection...\n");

  assert(is_failed_node(lg, osc_id) == false);  // Should not be failed
  assert(is_failed_node(lg, gain_id) == false); // Should not be failed
  assert(is_failed_node(lg, 99999) == false); // Doesn't exist, but not "failed"

  printf("✓ is_failed_node correctly identifies non-failed nodes\n");

  printf("\n=== Queue API Test Results ===\n");
  printf("✅ All queue API tests passed successfully!\n");
  printf("   - Immediate ID allocation works\n");
  printf("   - Pre-allocated IDs can be used in connections\n");
  printf("   - Queue processing applies operations in order\n");
  printf("   - Failed node tracking infrastructure ready\n");

  return 0;
}
