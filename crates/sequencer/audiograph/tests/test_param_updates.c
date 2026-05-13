#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_types.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>

// ===================== Custom State Output Node =====================

// State output node memory layout (size 1 float, outputs state[0])
#define STATE_OUTPUT_MEMORY_SIZE 1
#define STATE_OUTPUT_VALUE 0

// Custom processing function that outputs state[0]
void state_output_process(float *const *in, float *const *out, int n,
                          void *memory, void *buffers) {
  (void)in; // State output has no inputs
  float *mem = (float *)memory;
  float value = mem[STATE_OUTPUT_VALUE];
  float *y = out[0];
  for (int i = 0; i < n; i++)
    y[i] = value;
}

// Custom VTable for state output node
const NodeVTable STATE_OUTPUT_VTABLE = {
    .process = state_output_process, .init = NULL, .migrate = NULL};

// Helper function to add state output node to live graph
int live_add_state_output(LiveGraph *lg, float initial_value,
                          const char *name) {
  (void)initial_value; // We'll set the value after node creation

  // Create VTable with our custom process function
  NodeVTable vtable = {
      .process = state_output_process, .init = NULL, .migrate = NULL};

  // Use the standard add_node function
  int node_id =
      add_node(lg, vtable, STATE_OUTPUT_MEMORY_SIZE * sizeof(float), name, 0, 1,
               NULL, 0); // No initial state needed - will be set via params

  // We'll set the initial value after the node is created via parameter update
  return node_id;
}

void test_param_updates() {
  printf("=== Testing Parameter Updates ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "param_update_test", 1);
  assert(lg != NULL);

  // 1. Create custom operator that outputs state[0] (initially 0.0)
  int state_node = live_add_state_output(lg, 0.0f, "state_output");
  assert(state_node >= 0);
  printf("✓ Created state output node: id=%d (initial state[0]=0.0)\n",
         state_node);

  // Apply queued node creation
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied queued node creation\n");

  // Set initial value to 0.0 (state is initially zero, but let's be explicit)
  RTNode *state_node_ptr = &lg->nodes[state_node];
  if (state_node_ptr->state) {
    float *mem = (float *)state_node_ptr->state;
    mem[STATE_OUTPUT_VALUE] = 0.0f;
  }
  printf("✓ Set initial state[0] = 0.0\n");

  // Connect state output node to DAC
  bool connect_dac = apply_connect(lg, state_node, 0, lg->dac_node_id, 0);
  assert(connect_dac);
  printf("✓ Connected state output to DAC\n");

  // 2. Run process_next_block and confirm output is 0
  float output_buffer[block_size];
  memset(output_buffer, 0, sizeof(output_buffer));

  process_next_block(lg, output_buffer, block_size);

  // Verify all samples are 0.0
  float expected1 = 0.0f;
  bool all_correct = true;
  for (int i = 0; i < block_size; i++) {
    if (fabsf(output_buffer[i] - expected1) >= 0.001f) {
      printf("ERROR: Sample %d: got %.6f, expected %.6f\n", i, output_buffer[i],
             expected1);
      all_correct = false;
      if (i >= 5)
        break; // Don't spam too many errors
    }
  }

  assert(all_correct);
  printf("✓ Initial processing: All %d samples = %.1f (correct!)\n", block_size,
         expected1);

  // 3. Run params_push for that logical_id and idx=0 with value=1.0
  ParamMsg param_msg = {
      .logical_id = state_node, .idx = STATE_OUTPUT_VALUE, .fvalue = 1.0f};

  bool push_result = params_push(lg->params, param_msg);
  assert(push_result);
  printf("✓ Pushed parameter update: logical_id=%d, idx=%d, value=%.1f\n",
         state_node, STATE_OUTPUT_VALUE, 1.0f);

  // 4. Run process_next_block and confirm output is 1
  memset(output_buffer, 0, sizeof(output_buffer)); // Clear buffer first

  process_next_block(lg, output_buffer, block_size);

  // Verify all samples are 1.0
  float expected2 = 1.0f;
  all_correct = true;
  for (int i = 0; i < block_size; i++) {
    if (fabsf(output_buffer[i] - expected2) >= 0.001f) {
      printf("ERROR: Sample %d: got %.6f, expected %.6f\n", i, output_buffer[i],
             expected2);
      all_correct = false;
      if (i >= 5)
        break; // Don't spam too many errors
    }
  }

  assert(all_correct);
  printf("✓ After parameter update: All %d samples = %.1f (correct!)\n",
         block_size, expected2);

  // Additional test: Update to different value
  ParamMsg param_msg2 = {
      .logical_id = state_node, .idx = STATE_OUTPUT_VALUE, .fvalue = 42.5f};

  push_result = params_push(lg->params, param_msg2);
  assert(push_result);
  printf("✓ Pushed second parameter update: value=%.1f\n", 42.5f);

  process_next_block(lg, output_buffer, block_size);

  float expected3 = 42.5f;
  all_correct = true;
  for (int i = 0; i < 10; i++) { // Check first 10 samples
    if (fabsf(output_buffer[i] - expected3) >= 0.001f) {
      printf("ERROR: Sample %d: got %.6f, expected %.6f\n", i, output_buffer[i],
             expected3);
      all_correct = false;
    }
  }

  assert(all_correct);
  printf("✓ Second update: All samples = %.1f (correct!)\n", expected3);

  // Test edge case: Invalid node ID
  ParamMsg invalid_msg = {.logical_id = 999, // Invalid node ID
                          .idx = 0,
                          .fvalue = 5.0f};

  push_result = params_push(lg->params, invalid_msg);
  assert(push_result); // Should still push successfully (handled during
                       // processing)
  printf("✓ Invalid node ID parameter push accepted (will be ignored during "
         "processing)\n");

  // Process and verify no change
  process_next_block(lg, output_buffer, block_size);

  // Should still be 42.5f (unchanged)
  for (int i = 0; i < 5; i++) {
    assert(fabsf(output_buffer[i] - expected3) < 0.001f);
  }
  printf("✓ Invalid parameter ignored: Output unchanged = %.1f\n",
         output_buffer[0]);

  destroy_live_graph(lg);
  printf("=== Parameter Updates Test Completed Successfully ===\n\n");
}

int main() {
  initialize_engine(64, 48000);
  test_param_updates();
  return 0;
}
