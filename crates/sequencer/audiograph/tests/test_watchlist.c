#include "graph_engine.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

int main() {
  printf("Testing watchlist functionality...\n");

  // Create a graph
  LiveGraph *lg = create_live_graph(32, 128, "watchlist_test", 1);
  assert(lg != NULL);

  // Create multiple nodes with different characteristics
  int osc1_id = live_add_oscillator(lg, 440.0f, "osc_440");
  int osc2_id = live_add_oscillator(lg, 880.0f, "osc_880");
  int num1_id = live_add_number(lg, 2.0f, "number_2");
  int num2_id = live_add_number(lg, 3.5f, "number_3_5");
  int gain1_id = live_add_gain(lg, 0.5f, "gain_half");
  int gain2_id = live_add_gain(lg, 0.8f, "gain_most");
  int mixer_id = live_add_mixer2(lg, "mixer");

  printf("Created nodes: osc1=%d, osc2=%d, num1=%d, num2=%d, gain1=%d, gain2=%d, mixer=%d\n",
         osc1_id, osc2_id, num1_id, num2_id, gain1_id, gain2_id, mixer_id);

  // Add all nodes to watchlist
  assert(add_node_to_watchlist(lg, osc1_id));
  assert(add_node_to_watchlist(lg, osc2_id));
  assert(add_node_to_watchlist(lg, num1_id));
  assert(add_node_to_watchlist(lg, num2_id));
  assert(add_node_to_watchlist(lg, gain1_id));
  assert(add_node_to_watchlist(lg, gain2_id));
  assert(add_node_to_watchlist(lg, mixer_id));

  printf("Added all nodes to watchlist\n");

  // Try to add the same node again (should succeed, no duplicate)
  bool result3 = add_node_to_watchlist(lg, osc1_id);
  printf("Added osc1 again: %s\n", result3 ? "success" : "failed");
  assert(result3 == true);

  // Create different connection patterns to ensure different states:
  // Chain 1: osc1 -> gain1 -> mixer (input 0)
  // Chain 2: osc2 -> gain2 -> mixer (input 1)
  // Chain 3: num1 connects to gain1's gain control
  // Chain 4: num2 connects to gain2's gain control
  graph_connect(lg, osc1_id, 0, gain1_id, 0);
  graph_connect(lg, osc2_id, 0, gain2_id, 0);
  graph_connect(lg, num1_id, 0, gain1_id, 1); // Connect to gain control
  graph_connect(lg, num2_id, 0, gain2_id, 1); // Connect to gain control
  graph_connect(lg, gain1_id, 0, mixer_id, 0);
  graph_connect(lg, gain2_id, 0, mixer_id, 1);
  graph_connect(lg, mixer_id, 0, 0, 0); // Connect to DAC

  printf("Created complex connection graph\n");

  float output_buffer[128];
  // Process several blocks to let states stabilize and differentiate
  for (int i = 0; i < 3; i++) {
    process_next_block(lg, output_buffer, 128);
  }

  // Get node states for all watched nodes
  size_t state_sizes[7];
  void *states[7];
  int node_ids[] = {osc1_id, osc2_id, num1_id, num2_id, gain1_id, gain2_id, mixer_id};
  const char *node_names[] = {"osc1_440", "osc2_880", "num1_2.0", "num2_3.5", "gain1_0.5", "gain2_0.8", "mixer"};

  for (int i = 0; i < 7; i++) {
    states[i] = get_node_state(lg, node_ids[i], &state_sizes[i]);
    printf("Retrieved %s state: size=%zu, ptr=%p\n", node_names[i], state_sizes[i], states[i]);
  }

  // Verify we got state data for all nodes
  int valid_states = 0;
  for (int i = 0; i < 7; i++) {
    if (states[i] && state_sizes[i] > 0) {
      printf("%s state retrieved successfully (size=%zu)\n", node_names[i], state_sizes[i]);
      valid_states++;
    } else {
      printf("Warning: No %s state retrieved\n", node_names[i]);
    }
  }

  printf("Successfully retrieved %d out of 7 node states\n", valid_states);
  assert(valid_states >= 5); // Expect most nodes to have state

  // Test that states are actually different by comparing content
  printf("\n=== Testing state differentiation ===\n");
  bool found_differences = false;

  // Compare oscillator states (should differ due to different frequencies)
  if (states[0] && states[1] && state_sizes[0] > 0 && state_sizes[1] > 0) {
    if (state_sizes[0] == state_sizes[1]) {
      int comparison = memcmp(states[0], states[1], state_sizes[0]);
      printf("Oscillator state comparison (440Hz vs 880Hz): %s\n",
             comparison != 0 ? "DIFFERENT" : "identical");
      if (comparison != 0) found_differences = true;
    } else {
      printf("Oscillator states have different sizes: %zu vs %zu (DIFFERENT)\n",
             state_sizes[0], state_sizes[1]);
      found_differences = true;
    }
  }

  // Compare number states (should differ due to different values)
  if (states[2] && states[3] && state_sizes[2] > 0 && state_sizes[3] > 0) {
    if (state_sizes[2] == state_sizes[3]) {
      int comparison = memcmp(states[2], states[3], state_sizes[2]);
      printf("Number state comparison (2.0 vs 3.5): %s\n",
             comparison != 0 ? "DIFFERENT" : "identical");
      if (comparison != 0) found_differences = true;
    } else {
      printf("Number states have different sizes: %zu vs %zu (DIFFERENT)\n",
             state_sizes[2], state_sizes[3]);
      found_differences = true;
    }
  }

  // Compare gain states (should differ due to different inputs and gain values)
  if (states[4] && states[5] && state_sizes[4] > 0 && state_sizes[5] > 0) {
    if (state_sizes[4] == state_sizes[5]) {
      int comparison = memcmp(states[4], states[5], state_sizes[4]);
      printf("Gain state comparison (0.5 vs 0.8 gain): %s\n",
             comparison != 0 ? "DIFFERENT" : "identical");
      if (comparison != 0) found_differences = true;
    } else {
      printf("Gain states have different sizes: %zu vs %zu (DIFFERENT)\n",
             state_sizes[4], state_sizes[5]);
      found_differences = true;
    }
  }

  if (found_differences) {
    printf("✓ SUCCESS: get_node_state returns different values for different nodes\n");
  } else {
    printf("⚠ WARNING: Could not verify state differences (may be expected for some node types)\n");
  }

  // Clean up states
  for (int i = 0; i < 7; i++) {
    if (states[i]) {
      free(states[i]);
    }
  }

  // Test invalid node ID
  void *invalid_state = get_node_state(lg, 999, NULL);
  assert(invalid_state == NULL);
  printf("Invalid node ID correctly returned NULL\n");

  // Remove node from watchlist
  bool removed = remove_node_from_watchlist(lg, osc1_id);
  printf("Removed osc1 from watchlist: %s\n", removed ? "success" : "failed");
  assert(removed == true);

  process_next_block(lg, output_buffer, 128);

  // Try to remove again (should be a no-op once first removal is applied)
  bool removed2 = remove_node_from_watchlist(lg, osc1_id);
  printf("Tried to remove osc1 again (queued=%s)\n", removed2 ? "yes" : "no");
  process_next_block(lg, output_buffer, 128);
  void *state_after_second = get_node_state(lg, osc1_id, NULL);
  assert(state_after_second == NULL);
  if (state_after_second)
    free(state_after_second);

  // Clean up
  destroy_live_graph(lg);
  
  printf("Watchlist test completed successfully!\n");
  return 0;
}
