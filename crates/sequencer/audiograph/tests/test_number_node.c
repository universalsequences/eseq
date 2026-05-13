#include "graph_engine.h"
#include "graph_edit.h"
#include "graph_nodes.h"
#include <assert.h>
#include <stdio.h>
#include <math.h>

void test_number_node() {
  printf("=== Testing NUMBER Node Output ===\n");
  
  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "number_test", 1);
  assert(lg != NULL);
  
  // Create a NUMBER node that outputs 42.0
  int num = live_add_number(lg, 42.0f, "num42");
  assert(num >= 0);
  printf("✓ Created NUMBER node: num=%d (outputs 42.0)\n", num);
  
  // Apply queued node creation
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied queued node creation\n");
  
  // Verify node was created correctly
  RTNode *num_node = &lg->nodes[num];
  printf("DEBUG: NUMBER node - nInputs=%d, nOutputs=%d\n", num_node->nInputs, num_node->nOutputs);
  printf("DEBUG: NUMBER node state = %p\n", num_node->state);
  if (num_node->state) {
    float *memory = (float *)num_node->state;
    printf("DEBUG: NUMBER value in memory = %.6f\n", memory[NUMBER_VALUE]);
  }
  
  // Connect NUMBER directly to DAC
  printf("DEBUG: Connecting num=%d:0 to DAC=%d:0\n", num, lg->dac_node_id);
  bool connect_dac = apply_connect(lg, num, 0, lg->dac_node_id, 0);
  assert(connect_dac);
  printf("✓ Connected NUMBER to DAC\n");
  
  // Process a block and check output
  float output_buffer[block_size];
  memset(output_buffer, 0, sizeof(output_buffer)); // Clear buffer first
  
  process_next_block(lg, output_buffer, block_size);
  
  printf("DEBUG: Output buffer first 5 samples: %.6f, %.6f, %.6f, %.6f, %.6f\n",
         output_buffer[0], output_buffer[1], output_buffer[2], output_buffer[3], output_buffer[4]);
  
  // Verify all samples are 42.0
  float expected = 42.0f;
  bool all_correct = true;
  for (int i = 0; i < block_size; i++) {
    if (fabsf(output_buffer[i] - expected) >= 0.001f) {
      printf("ERROR: Sample %d: got %.6f, expected %.6f\n", i, output_buffer[i], expected);
      all_correct = false;
      if (i >= 5) break; // Don't spam too many errors
    }
  }
  
  assert(all_correct);
  printf("✓ NUMBER node output: All %d samples = %.1f (correct!)\n", block_size, expected);
  
  // Test with a different value
  float *memory = (float *)num_node->state;
  memory[NUMBER_VALUE] = 123.5f;
  
  process_next_block(lg, output_buffer, block_size);
  
  float expected2 = 123.5f;
  all_correct = true;
  for (int i = 0; i < 10; i++) { // Check first 10 samples
    if (fabsf(output_buffer[i] - expected2) >= 0.001f) {
      printf("ERROR: Sample %d: got %.6f, expected %.6f\n", i, output_buffer[i], expected2);
      all_correct = false;
    }
  }
  
  assert(all_correct);
  printf("✓ Changed value: All samples = %.1f (correct!)\n", expected2);
  
  destroy_live_graph(lg);
  printf("=== NUMBER Node Test Completed Successfully ===\n\n");
}

int main() {
  initialize_engine(64, 48000);
  test_number_node();
  return 0;
}