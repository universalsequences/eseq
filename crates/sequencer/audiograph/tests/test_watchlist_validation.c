#include "graph_engine.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>

// Custom node that writes first sample of input to its state
typedef struct {
  float last_input_sample;
  float processing_count;
} TestRecorderState;

void test_recorder_process(float *const *in, float *const *out, int nframes,
                           void *state, void *memory) {
  TestRecorderState *s = (TestRecorderState *)state;

  // Record the first sample of input
  if (nframes > 0 && in[0]) {
    s->last_input_sample = in[0][0];
  }

  // Increment processing count
  s->processing_count += 1.0f;

  // Pass through the input to output
  if (in[0] && out[0]) {
    for (int i = 0; i < nframes; i++) {
      out[0][i] = in[0][i];
    }
  }
}

void test_recorder_init(void *state, int sampleRate, int maxBlock,
                        const void *initial_state) {
  (void)sampleRate;
  (void)maxBlock;
  (void)initial_state;
  TestRecorderState *s = (TestRecorderState *)state;
  s->last_input_sample = 0.0f;
  s->processing_count = 0.0f;
}

NodeVTable create_test_recorder_vtable() {
  return (NodeVTable){.process = test_recorder_process,
                      .init = test_recorder_init,
                      .reset = NULL,
                      .migrate = NULL};
}

// Helper to compare floats with tolerance
bool float_equals(float a, float b, float tolerance) {
  return fabsf(a - b) < tolerance;
}

int main() {
  printf("Testing watchlist with custom recorder node...\n");

  // Create a graph
  LiveGraph *lg = create_live_graph(16, 128, "validation_test", 1);
  assert(lg != NULL);

  // Add a number node that outputs a constant value
  int number_id = live_add_number(lg, 0.5f, "constant");
  printf("Created number node %d outputting 0.5\n", number_id);

  // Add our custom recorder node
  NodeVTable recorder_vtable = create_test_recorder_vtable();
  int recorder_id =
      apply_add_node(lg, recorder_vtable, sizeof(TestRecorderState), 10,
                     "recorder", 1, 1, NULL);
  printf("Created recorder node %d\n", recorder_id);

  // Add recorder to watchlist
  add_node_to_watchlist(lg, recorder_id);

  // Connect: number -> recorder -> DAC
  graph_connect(lg, number_id, 0, recorder_id, 0);
  graph_connect(lg, recorder_id, 0, 0, 0);

  printf("\nProcessing first block...\n");
  float output_buffer[128];
  // Snapshots are throttled on the audio path, so process enough blocks to
  // reach the next watchlist refresh after changing the graph.
  for (int i = 0; i < 4; i++) {
    process_next_block(lg, output_buffer, 128);
  }

  // Get recorder state and validate
  size_t state_size;
  void *state = get_node_state(lg, recorder_id, &state_size);

  printf("Retrieved recorder state: size=%zu\n", state_size);
  assert(state != NULL);
  assert(state_size == sizeof(TestRecorderState));

  TestRecorderState *recorder_state = (TestRecorderState *)state;
  printf("Recorded input sample: %f\n", recorder_state->last_input_sample);
  printf("Processing count: %f\n", recorder_state->processing_count);

  // Validate the recorded input matches the number node output (0.5)
  if (float_equals(recorder_state->last_input_sample, 0.5f, 0.001f)) {
    printf("✓ Recorder captured CORRECT input value (0.5)\n");
  } else {
    printf("✗ Recorder captured INCORRECT input: expected 0.5, got %f\n",
           recorder_state->last_input_sample);
    free(state);
    destroy_live_graph(lg);
    return 1;
  }

  // Processing count should be 1 after first block
  if (float_equals(recorder_state->processing_count, 1.0f, 0.001f)) {
    printf("✓ Processing count is correct (1.0)\n");
  } else {
    printf("✗ Processing count incorrect: expected 1.0, got %f\n",
           recorder_state->processing_count);
    free(state);
    destroy_live_graph(lg);
    return 1;
  }

  free(state);

  // Change the number node's value and process another block
  printf("\nChanging number node value to 0.8...\n");

  // We need to update the number node's state directly or use parameter updates
  // For simplicity, let's create a new number node with different value
  int number2_id = live_add_number(lg, 0.8f, "constant2");

  // Disconnect old number and connect new one
  graph_disconnect(lg, number_id, 0, recorder_id, 0);
  graph_connect(lg, number2_id, 0, recorder_id, 0);

  printf("Connected new number node %d outputting 0.8\n", number2_id);

  process_next_block(lg, output_buffer, 128);

  // Get updated recorder state
  void *state2 = get_node_state(lg, recorder_id, &state_size);
  assert(state2 != NULL);

  TestRecorderState *recorder_state2 = (TestRecorderState *)state2;
  printf("Updated recorded input sample: %f\n",
         recorder_state2->last_input_sample);
  printf("Updated processing count: %f\n", recorder_state2->processing_count);

  // Should now show 0.8 as input after the throttled snapshot refresh.
  if (float_equals(recorder_state2->last_input_sample, 0.8f, 0.001f)) {
    printf("✓ Recorder captured CORRECT updated input (0.8)\n");
  } else {
    printf(
        "✗ Recorder captured INCORRECT updated input: expected 0.8, got %f\n",
        recorder_state2->last_input_sample);
    free(state2);
    destroy_live_graph(lg);
    return 1;
  }

  if (float_equals(recorder_state2->processing_count, 5.0f, 0.001f)) {
    printf("✓ Processing count incremented correctly (5.0)\n");
  } else {
    printf("✗ Processing count incorrect: expected 5.0, got %f\n",
           recorder_state2->processing_count);
    free(state2);
    destroy_live_graph(lg);
    return 1;
  }

  free(state2);

  // Test that output signal also matches
  printf("\nValidating output signal...\n");
  float max_output = 0.0f;
  for (int i = 0; i < 128; i++) {
    if (fabsf(output_buffer[i]) > max_output) {
      max_output = fabsf(output_buffer[i]);
    }
  }
  printf("Output signal peak: %f\n", max_output);

  if (float_equals(max_output, 0.8f, 0.001f)) {
    printf("✓ Output signal matches expected value\n");
  } else {
    printf("✗ Output signal doesn't match: expected ~0.8, got %f\n",
           max_output);
  }

  // Test removing from watchlist
  printf("\nRemoving recorder from watchlist...\n");
  remove_node_from_watchlist(lg, recorder_id);
  process_next_block(lg, output_buffer, 128);

  void *removed_state = get_node_state(lg, recorder_id, NULL);
  assert(removed_state == NULL);
  printf("✓ Removed node correctly returns NULL state\n");

  destroy_live_graph(lg);

  printf("\nAll validation tests PASSED! The watchlist correctly captures node "
         "state! ✓\n");
  return 0;
}
