#include "graph_engine.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <math.h>

int main() {
  printf("Testing that watched nodes are never orphaned...\n\n");

  // Create a graph
  LiveGraph *lg = create_live_graph(16, 128, "watchlist_no_orphan_test", 1);
  assert(lg != NULL);

  // Create nodes: oscillator -> gain (not connected to DAC)
  int osc_id = live_add_oscillator(lg, 440.0f, "scope_osc");
  int gain_id = live_add_gain(lg, 0.7f, "scope_gain");

  // Also create a separate chain that IS connected to DAC for comparison
  int main_osc_id = live_add_oscillator(lg, 220.0f, "main_osc");

  printf("Created nodes:\n");
  printf("  Scope chain: osc_id=%d, gain_id=%d (NOT connected to DAC)\n", osc_id, gain_id);
  printf("  Main chain: main_osc_id=%d (WILL be connected to DAC)\n\n", main_osc_id);

  // Connect scope chain (but NOT to DAC - simulating a scope~ visualization)
  graph_connect(lg, osc_id, 0, gain_id, 0);

  // Connect main chain to DAC
  graph_connect(lg, main_osc_id, 0, 0, 0); // Connect to DAC

  // Process a block to establish initial state
  float output_buffer[128];
  process_next_block(lg, output_buffer, 128);

  // Check initial orphan status
  printf("=== Initial Orphan Status (before watchlist) ===\n");
  printf("  Scope osc (not connected to DAC): %s\n",
         lg->sched.is_orphaned[osc_id] ? "ORPHANED" : "ACTIVE");
  printf("  Scope gain (not connected to DAC): %s\n",
         lg->sched.is_orphaned[gain_id] ? "ORPHANED" : "ACTIVE");
  printf("  Main osc (connected to DAC): %s\n",
         lg->sched.is_orphaned[main_osc_id] ? "ORPHANED" : "ACTIVE");

  // The scope chain should be orphaned since it's not connected to DAC
  assert(lg->sched.is_orphaned[osc_id] == true);
  assert(lg->sched.is_orphaned[gain_id] == true);
  assert(lg->sched.is_orphaned[main_osc_id] == false);
  printf("✓ Unconnected nodes correctly marked as orphaned\n\n");

  // Now add the scope nodes to watchlist
  printf("=== Adding scope nodes to watchlist ===\n");
  bool added1 = add_node_to_watchlist(lg, osc_id);
  bool added2 = add_node_to_watchlist(lg, gain_id);
  assert(added1 && added2);
  printf("✓ Added scope nodes to watchlist\n");

  // Force orphan status update
  update_orphaned_status(lg);

  // Check orphan status after adding to watchlist
  printf("\n=== Orphan Status AFTER Watchlist ===\n");
  printf("  Scope osc (watched): %s\n",
         lg->sched.is_orphaned[osc_id] ? "ORPHANED" : "ACTIVE");
  printf("  Scope gain (watched): %s\n",
         lg->sched.is_orphaned[gain_id] ? "ORPHANED" : "ACTIVE");
  printf("  Main osc (connected to DAC): %s\n",
         lg->sched.is_orphaned[main_osc_id] ? "ORPHANED" : "ACTIVE");

  // CRITICAL TEST: Watched nodes should NOT be orphaned even without DAC connection
  assert(lg->sched.is_orphaned[osc_id] == false);
  assert(lg->sched.is_orphaned[gain_id] == false);
  assert(lg->sched.is_orphaned[main_osc_id] == false);
  printf("✓ SUCCESS: Watched nodes are NOT orphaned despite no DAC connection!\n\n");

  // Process blocks and verify watched nodes are actually processing
  printf("=== Testing watched node processing ===\n");

  // Get initial state snapshots
  size_t osc_state_size1, gain_state_size1;
  void *osc_state1 = get_node_state(lg, osc_id, &osc_state_size1);
  void *gain_state1 = get_node_state(lg, gain_id, &gain_state_size1);

  // Store initial oscillator phase if available
  float initial_phase = 0.0f;
  if (osc_state1 && osc_state_size1 >= sizeof(float)) {
    initial_phase = *((float*)osc_state1);
    printf("Initial oscillator phase: %f\n", initial_phase);
  }

  // Process several blocks
  for (int i = 0; i < 5; i++) {
    process_next_block(lg, output_buffer, 128);
  }

  // Get state after processing
  size_t osc_state_size2, gain_state_size2;
  void *osc_state2 = get_node_state(lg, osc_id, &osc_state_size2);
  void *gain_state2 = get_node_state(lg, gain_id, &gain_state_size2);

  // Check if oscillator phase has changed (indicating it's processing)
  if (osc_state2 && osc_state_size2 >= sizeof(float)) {
    float current_phase = *((float*)osc_state2);
    printf("Current oscillator phase: %f\n", current_phase);

    if (fabsf(current_phase - initial_phase) > 0.001f) {
      printf("✓ Oscillator phase changed - node IS processing!\n");
    } else {
      printf("⚠ WARNING: Oscillator phase unchanged - node may not be processing\n");
    }
  }

  // Clean up states
  if (osc_state1) free(osc_state1);
  if (gain_state1) free(gain_state1);
  if (osc_state2) free(osc_state2);
  if (gain_state2) free(gain_state2);

  // Test removing from watchlist restores orphan behavior
  printf("\n=== Testing watchlist removal ===\n");
  remove_node_from_watchlist(lg, osc_id);
  remove_node_from_watchlist(lg, gain_id);

  memset(output_buffer, 0, sizeof(output_buffer));
  process_next_block(lg, output_buffer, 128);

  // Force orphan status update again
  update_orphaned_status(lg);

  printf("After removing from watchlist:\n");
  printf("  Scope osc: %s\n",
         lg->sched.is_orphaned[osc_id] ? "ORPHANED" : "ACTIVE");
  printf("  Scope gain: %s\n",
         lg->sched.is_orphaned[gain_id] ? "ORPHANED" : "ACTIVE");

  // Should be orphaned again after removal from watchlist
  assert(lg->sched.is_orphaned[osc_id] == true);
  assert(lg->sched.is_orphaned[gain_id] == true);
  printf("✓ Nodes correctly re-orphaned after watchlist removal\n");

  // Clean up
  destroy_live_graph(lg);

  printf("\n✅ WATCHLIST NO-ORPHAN TEST COMPLETED SUCCESSFULLY!\n");
  printf("Watched nodes remain active even without DAC connections.\n");
  return 0;
}
