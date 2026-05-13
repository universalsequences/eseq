#include "graph_engine.h"
#include "graph_nodes.h"
#include "graph_edit.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>

// Test multi-channel (stereo) audio support
int main() {
  printf("=== Testing Stereo (Multi-Channel) Audio Support ===\n\n");

  initialize_engine(128, 48000);

  // Test 1: Create stereo graph
  printf("Test 1: Creating stereo graph (2 channels)...\n");
  LiveGraph *lg = create_live_graph(16, 128, "stereo_test", 2);
  assert(lg != NULL);
  assert(lg->num_channels == 2);
  printf("✓ Created stereo LiveGraph with %d channels\n", lg->num_channels);

  // Verify DAC has 2 inputs and 2 outputs for stereo
  int dac_id = lg->dac_node_id;
  assert(dac_id >= 0);
  RTNode *dac = &lg->nodes[dac_id];
  assert(dac->nInputs == 2);
  assert(dac->nOutputs == 2);
  printf("✓ DAC node has %d inputs and %d outputs (stereo)\n",
         dac->nInputs, dac->nOutputs);

  // Test 2: Create separate signal chains for left and right channels
  printf("\nTest 2: Creating separate left/right channel signals...\n");

  // Left channel: NUMBER outputting 100.0
  int left_num = live_add_number(lg, 100.0f, "left_channel");
  assert(left_num >= 0);

  // Right channel: NUMBER outputting 200.0
  int right_num = live_add_number(lg, 200.0f, "right_channel");
  assert(right_num >= 0);

  apply_graph_edits(lg->graphEditQueue, lg);
  printf("✓ Created left channel signal (100.0) and right channel signal (200.0)\n");

  // Connect left to DAC channel 0, right to DAC channel 1
  bool left_conn = apply_connect(lg, left_num, 0, dac_id, 0);
  bool right_conn = apply_connect(lg, right_num, 0, dac_id, 1);
  assert(left_conn && right_conn);
  printf("✓ Connected left->DAC[0], right->DAC[1]\n");

  // Test 3: Process audio and verify interleaved output
  printf("\nTest 3: Processing stereo audio block...\n");

  // Buffer size = nframes * num_channels for interleaved stereo
  int nframes = 128;
  float output_buffer[nframes * 2]; // stereo interleaved

  process_next_block(lg, output_buffer, nframes);

  // Verify interleaved format: [L0, R0, L1, R1, L2, R2, ...]
  bool all_correct = true;
  for (int i = 0; i < nframes; i++) {
    float left_sample = output_buffer[i * 2 + 0];
    float right_sample = output_buffer[i * 2 + 1];

    if (fabsf(left_sample - 100.0f) > 0.001f ||
        fabsf(right_sample - 200.0f) > 0.001f) {
      printf("✗ Frame %d: L=%.1f (expected 100.0), R=%.1f (expected 200.0)\n",
             i, left_sample, right_sample);
      all_correct = false;
      break;
    }
  }

  if (all_correct) {
    printf("✓ All %d frames have correct interleaved stereo output\n", nframes);
    printf("  Left channel = 100.0, Right channel = 200.0\n");
  }

  // Test 4: Verify first few samples
  printf("\nTest 4: Verifying interleaved sample order...\n");
  printf("  Samples [0-7]: ");
  for (int i = 0; i < 4; i++) {
    printf("[%.0f,%.0f] ", output_buffer[i*2], output_buffer[i*2+1]);
  }
  printf("\n");

  assert(fabsf(output_buffer[0] - 100.0f) < 0.001f); // Frame 0, Left
  assert(fabsf(output_buffer[1] - 200.0f) < 0.001f); // Frame 0, Right
  assert(fabsf(output_buffer[2] - 100.0f) < 0.001f); // Frame 1, Left
  assert(fabsf(output_buffer[3] - 200.0f) < 0.001f); // Frame 1, Right
  printf("✓ Interleaved format verified: [L,R,L,R,...]\n");

  // Test 5: Modify one channel and verify independence
  printf("\nTest 5: Testing channel independence...\n");

  // Change left channel to 50.0 via parameter update
  ParamMsg msg = {
    .logical_id = left_num,
    .idx = 0, // NUMBER_VALUE index
    .fvalue = 50.0f
  };
  params_push(lg->params, msg);

  process_next_block(lg, output_buffer, nframes);

  float left_val = output_buffer[0];
  float right_val = output_buffer[1];

  assert(fabsf(left_val - 50.0f) < 0.001f);
  assert(fabsf(right_val - 200.0f) < 0.001f);
  printf("✓ Left channel changed to %.1f, right channel unchanged at %.1f\n",
         left_val, right_val);

  // Test 6: Create a mono graph for comparison
  printf("\nTest 6: Creating mono graph for comparison...\n");
  LiveGraph *lg_mono = create_live_graph(16, 128, "mono_test", 1);
  assert(lg_mono != NULL);
  assert(lg_mono->num_channels == 1);

  RTNode *dac_mono = &lg_mono->nodes[lg_mono->dac_node_id];
  assert(dac_mono->nInputs == 1);
  assert(dac_mono->nOutputs == 1);
  printf("✓ Created mono LiveGraph with %d channel\n", lg_mono->num_channels);
  printf("✓ Mono DAC has %d input and %d output\n",
         dac_mono->nInputs, dac_mono->nOutputs);

  int mono_num = live_add_number(lg_mono, 42.0f, "mono_signal");
  apply_graph_edits(lg_mono->graphEditQueue, lg_mono);
  apply_connect(lg_mono, mono_num, 0, lg_mono->dac_node_id, 0);

  float mono_buffer[nframes]; // mono = nframes samples
  process_next_block(lg_mono, mono_buffer, nframes);

  assert(fabsf(mono_buffer[0] - 42.0f) < 0.001f);
  printf("✓ Mono output verified: %.1f\n", mono_buffer[0]);

  // Test 7: Test silence on unconnected channels
  printf("\nTest 7: Testing unconnected channel behavior...\n");

  LiveGraph *lg_partial = create_live_graph(16, 128, "partial_stereo", 2);
  int partial_num = live_add_number(lg_partial, 99.0f, "left_only");
  apply_graph_edits(lg_partial->graphEditQueue, lg_partial);

  // Only connect to left channel (channel 0), leave right (channel 1) unconnected
  apply_connect(lg_partial, partial_num, 0, lg_partial->dac_node_id, 0);

  float partial_buffer[nframes * 2];
  process_next_block(lg_partial, partial_buffer, nframes);

  float left_connected = partial_buffer[0];
  float right_unconnected = partial_buffer[1];

  assert(fabsf(left_connected - 99.0f) < 0.001f);
  assert(fabsf(right_unconnected - 0.0f) < 0.001f);
  printf("✓ Connected left=%.1f, unconnected right=%.1f (silence)\n",
         left_connected, right_unconnected);

  // Test 8: Test with gain nodes in stereo
  printf("\nTest 8: Testing gain processing on stereo channels...\n");

  LiveGraph *lg_gain = create_live_graph(16, 128, "stereo_gain", 2);

  int left_src = live_add_number(lg_gain, 10.0f, "left_src");
  int right_src = live_add_number(lg_gain, 20.0f, "right_src");
  int left_gain = live_add_gain(lg_gain, 2.0f, "left_gain");
  int right_gain = live_add_gain(lg_gain, 3.0f, "right_gain");

  apply_graph_edits(lg_gain->graphEditQueue, lg_gain);

  // Chain: left_src -> left_gain -> DAC[0]
  //        right_src -> right_gain -> DAC[1]
  apply_connect(lg_gain, left_src, 0, left_gain, 0);
  apply_connect(lg_gain, right_src, 0, right_gain, 0);
  apply_connect(lg_gain, left_gain, 0, lg_gain->dac_node_id, 0);
  apply_connect(lg_gain, right_gain, 0, lg_gain->dac_node_id, 1);

  float gain_buffer[nframes * 2];
  process_next_block(lg_gain, gain_buffer, nframes);

  float left_gained = gain_buffer[0];  // 10.0 * 2.0 = 20.0
  float right_gained = gain_buffer[1]; // 20.0 * 3.0 = 60.0

  assert(fabsf(left_gained - 20.0f) < 0.001f);
  assert(fabsf(right_gained - 60.0f) < 0.001f);
  printf("✓ Stereo gain processing: L=%.1f (10*2), R=%.1f (20*3)\n",
         left_gained, right_gained);

  // Cleanup
  destroy_live_graph(lg);
  destroy_live_graph(lg_mono);
  destroy_live_graph(lg_partial);
  destroy_live_graph(lg_gain);

  printf("\n=== All Stereo Tests Passed! ===\n");
  return 0;
}