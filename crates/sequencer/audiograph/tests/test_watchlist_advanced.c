#include "graph_engine.h"
#include <assert.h>
#include <stdio.h>
#include <unistd.h>

int main() {
  printf("Testing advanced watchlist functionality with state changes...\n");

  initialize_engine(128, 48000);
  engine_start_workers(2);

  // Create a graph
  LiveGraph *lg = create_live_graph(16, 128, "watchlist_advanced_test", 1);
  assert(lg != NULL);

  // Add nodes with state
  int osc_id = live_add_oscillator(lg, 440.0f, "test_osc");
  int gain_id = live_add_gain(lg, 0.5f, "test_gain");
  
  printf("Created oscillator node: %d\n", osc_id);
  printf("Created gain node: %d\n", gain_id);

  // Connect the nodes
  graph_connect(lg, osc_id, 0, gain_id, 0);
  graph_connect(lg, gain_id, 0, 0, 0); // Connect to DAC

  // Add to watchlist
  add_node_to_watchlist(lg, osc_id);
  add_node_to_watchlist(lg, gain_id);
  printf("Added nodes to watchlist\n");

  float output_buffer[128];
  
  // Process several blocks and monitor state changes
  for (int i = 0; i < 5; i++) {
    printf("\n--- Processing block %d ---\n", i + 1);
    
    process_next_block(lg, output_buffer, 128);
    
    // Get current states
    size_t osc_state_size, gain_state_size;
    void *osc_state = get_node_state(lg, osc_id, &osc_state_size);
    void *gain_state = get_node_state(lg, gain_id, &gain_state_size);
    
    if (osc_state && osc_state_size >= sizeof(float)) {
      // Oscillator state likely contains phase and frequency info
      float *osc_data = (float *)osc_state;
      printf("Oscillator state - first float: %f\n", osc_data[0]);
      
      if (osc_state_size >= 2 * sizeof(float)) {
        printf("Oscillator state - second float: %f\n", osc_data[1]);
      }
    }
    
    if (gain_state && gain_state_size >= sizeof(float)) {
      // Gain state likely contains gain value
      float *gain_data = (float *)gain_state;
      printf("Gain state - gain value: %f\n", gain_data[0]);
    }
    
    // Check output signal level
    float max_output = 0.0f;
    for (int j = 0; j < 128; j++) {
      float abs_val = output_buffer[j] < 0 ? -output_buffer[j] : output_buffer[j];
      if (abs_val > max_output) max_output = abs_val;
    }
    printf("Output peak level: %f\n", max_output);
    
    free(osc_state);
    free(gain_state);
  }

  // Test removing one node from watchlist
  printf("\nRemoving oscillator from watchlist...\n");
  remove_node_from_watchlist(lg, osc_id);
  
  // Process another block
  process_next_block(lg, output_buffer, 128);
  
  // Only gain state should be available now
  void *osc_state = get_node_state(lg, osc_id, NULL);
  void *gain_state = get_node_state(lg, gain_id, NULL);
  
  printf("After removal - Osc state available: %s\n", osc_state ? "yes" : "no");
  printf("After removal - Gain state available: %s\n", gain_state ? "yes" : "no");
  
  if (osc_state) free(osc_state);
  if (gain_state) free(gain_state);

  // Clean up
  engine_stop_workers();
  destroy_live_graph(lg);
  
  printf("\nAdvanced watchlist test completed successfully!\n");
  return 0;
}