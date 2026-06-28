#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_types.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>

#define DGEN_HEADER_SLOTS 5
#define DGEN_CANARY_INDEX 2
#define DGEN_HEADER_CANARY_BITS 0x4cd35a1dU
#define DGEN_STATE_REDZONE_SLOTS 256
#define LARGE_PROJECT_LOAD_PARAM_BURST 12000

_Static_assert(PARAM_RING_CAP >= LARGE_PROJECT_LOAD_PARAM_BURST,
               "param ring must hold large project-load bursts");

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

void test_dgen_scalar_param_update_does_not_touch_adjacent_slots() {
  printf("=== Testing DGen Scalar Parameter Update Slot Isolation ===\n");

  const int block_size = 64;
  const int total_slots = 8;
  const int write_base =
      DGEN_HEADER_SLOTS + total_slots + DGEN_STATE_REDZONE_SLOTS;
  const int state_slots = write_base + total_slots;
  LiveGraph *lg = create_live_graph(16, block_size, "dgen_param_slot_test", 1);
  assert(lg != NULL);

  int dgen_node =
      add_node(lg, STATE_OUTPUT_VTABLE, state_slots * (int)sizeof(float),
               "dgen_shaped_state", 0, 1, NULL, 0);
  assert(dgen_node >= 0);
  assert(apply_graph_edits(lg->graphEditQueue, lg));

  RTNode *node = &lg->nodes[dgen_node];
  assert(node->state != NULL);
  float *mem = (float *)node->state;

  mem[1] = (float)total_slots;
  union {
    float f;
    uint32_t u;
  } canary = {.u = DGEN_HEADER_CANARY_BITS};
  mem[DGEN_CANARY_INDEX] = canary.f;

  const int dgen_idx = 2;
  const int read_idx = DGEN_HEADER_SLOTS + dgen_idx;
  const int mirrored_idx = write_base + dgen_idx;
  mem[read_idx + 1] = 11.0f;
  mem[read_idx + 2] = 12.0f;
  mem[read_idx + 3] = 13.0f;
  mem[mirrored_idx + 1] = 21.0f;
  mem[mirrored_idx + 2] = 22.0f;
  mem[mirrored_idx + 3] = 23.0f;

  ParamMsg msg = {
      .logical_id = dgen_node, .idx = (uint64_t)read_idx, .fvalue = 7200.0f};
  assert(params_push(lg->params, msg));

  float output_buffer[block_size];
  memset(output_buffer, 0, sizeof(output_buffer));
  process_next_block(lg, output_buffer, block_size);

  assert(mem[read_idx] == 7200.0f);
  assert(mem[mirrored_idx] == 7200.0f);
  assert(mem[read_idx + 1] == 11.0f);
  assert(mem[read_idx + 2] == 12.0f);
  assert(mem[read_idx + 3] == 13.0f);
  assert(mem[mirrored_idx + 1] == 21.0f);
  assert(mem[mirrored_idx + 2] == 22.0f);
  assert(mem[mirrored_idx + 3] == 23.0f);

  destroy_live_graph(lg);
  printf("✓ Scalar DGen param update preserved adjacent read/write slots\n");
  printf("=== DGen Scalar Parameter Update Slot Isolation Completed Successfully ===\n\n");
}

void test_param_ring_accepts_large_project_load_burst() {
  printf("=== Testing Large Project-Load Parameter Burst Capacity ===\n");

  LiveGraph *lg = create_live_graph(16, 64, "param_burst_capacity_test", 1);
  assert(lg != NULL);

  for (int i = 0; i < LARGE_PROJECT_LOAD_PARAM_BURST; i++) {
    ParamMsg msg = {
        .logical_id = 1,
        .idx = (uint64_t)i,
        .fvalue = (float)i,
    };
    assert(params_push(lg->params, msg));
  }

  uint32_t head = atomic_load_explicit(&lg->params->head, memory_order_acquire);
  uint32_t tail = atomic_load_explicit(&lg->params->tail, memory_order_acquire);
  assert(head - tail == LARGE_PROJECT_LOAD_PARAM_BURST);

  destroy_live_graph(lg);
  printf("✓ Accepted %d queued parameter updates without overflow\n",
         LARGE_PROJECT_LOAD_PARAM_BURST);
  printf("=== Large Project-Load Parameter Burst Capacity Test Completed Successfully ===\n\n");
}

int main() {
  initialize_engine(64, 48000);
  test_param_updates();
  test_dgen_scalar_param_update_does_not_touch_adjacent_slots();
  test_param_ring_accepts_large_project_load_burst();
  return 0;
}
