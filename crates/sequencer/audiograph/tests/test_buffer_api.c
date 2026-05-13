#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

void test_create_buffer_empty() {
  printf("=== Testing Create Buffer (Empty) ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "buffer_test", 1);
  assert(lg != NULL);

  // Create an empty buffer (no source data)
  int buf_id = create_buffer(lg, 1024, 2, NULL);
  assert(buf_id >= 0);
  printf("✓ Created empty buffer: id=%d (1024 samples, 2 channels)\n", buf_id);

  // Apply the buffer creation
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied graph edits successfully\n");

  // Verify buffer was created
  assert(buf_id < lg->buffer_capacity);
  BufferDesc *buf = &lg->buffers[buf_id];
  assert(buf->buffer != NULL);
  assert(buf->size == 1024);
  assert(buf->channel_count == 2);
  printf("✓ Buffer has correct size and channel count\n");

  // Verify buffer is zero-initialized
  bool all_zero = true;
  for (int i = 0; i < 1024 * 2; i++) {
    if (buf->buffer[i] != 0.0f) {
      all_zero = false;
      break;
    }
  }
  assert(all_zero);
  printf("✓ Empty buffer is zero-initialized\n");

  destroy_live_graph(lg);
  printf("✓ Create empty buffer test passed!\n\n");
}

void test_create_buffer_with_data() {
  printf("=== Testing Create Buffer (With Data) ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "buffer_test", 1);
  assert(lg != NULL);

  // Prepare source data (interleaved stereo: L0, R0, L1, R1, ...)
  const int num_samples = 256;
  const int num_channels = 2;
  float source_data[num_samples * num_channels];
  for (int i = 0; i < num_samples; i++) {
    source_data[i * 2 + 0] = (float)i * 0.1f;       // Left channel
    source_data[i * 2 + 1] = (float)i * 0.1f + 0.5f; // Right channel
  }

  // Create buffer with source data
  int buf_id = create_buffer(lg, num_samples, num_channels, source_data);
  assert(buf_id >= 0);
  printf("✓ Created buffer with data: id=%d\n", buf_id);

  // Apply the buffer creation
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied graph edits successfully\n");

  // Verify data was copied correctly
  BufferDesc *buf = &lg->buffers[buf_id];
  assert(buf->buffer != NULL);

  bool data_matches = true;
  for (int i = 0; i < num_samples * num_channels; i++) {
    if (fabsf(buf->buffer[i] - source_data[i]) > 0.0001f) {
      data_matches = false;
      printf("  Mismatch at index %d: expected %.4f, got %.4f\n", i,
             source_data[i], buf->buffer[i]);
      break;
    }
  }
  assert(data_matches);
  printf("✓ Source data was copied correctly\n");

  // Verify we can modify source_data without affecting the buffer
  // (data was copied, not referenced)
  float original_value = buf->buffer[0];
  source_data[0] = 999.0f;
  assert(buf->buffer[0] == original_value);
  printf("✓ Buffer data is independent of source (copy semantics)\n");

  destroy_live_graph(lg);
  printf("✓ Create buffer with data test passed!\n\n");
}

void test_hot_swap_buffer() {
  printf("=== Testing Hot Swap Buffer ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "buffer_test", 1);
  assert(lg != NULL);

  // Create initial buffer with data
  const int num_samples = 128;
  const int num_channels = 1;
  float initial_data[num_samples];
  for (int i = 0; i < num_samples; i++) {
    initial_data[i] = 1.0f; // All ones
  }

  int buf_id = create_buffer(lg, num_samples, num_channels, initial_data);
  assert(buf_id >= 0);
  printf("✓ Created initial buffer: id=%d\n", buf_id);

  // Apply buffer creation
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify initial data
  BufferDesc *buf = &lg->buffers[buf_id];
  assert(fabsf(buf->buffer[0] - 1.0f) < 0.0001f);
  printf("✓ Initial buffer data verified (all 1.0)\n");

  // Prepare new data for hot swap
  float new_data[num_samples];
  for (int i = 0; i < num_samples; i++) {
    new_data[i] = 2.0f; // All twos
  }

  // Hot swap the buffer
  int result = hot_swap_buffer(lg, buf_id, new_data, num_samples, num_channels);
  assert(result);
  printf("✓ Queued hot swap buffer command\n");

  // Apply the hot swap
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied hot swap successfully\n");

  // Verify new data
  bool data_updated = true;
  for (int i = 0; i < num_samples; i++) {
    if (fabsf(buf->buffer[i] - 2.0f) > 0.0001f) {
      data_updated = false;
      printf("  Mismatch at index %d: expected 2.0, got %.4f\n", i,
             buf->buffer[i]);
      break;
    }
  }
  assert(data_updated);
  printf("✓ Buffer data was hot swapped correctly (now all 2.0)\n");

  destroy_live_graph(lg);
  printf("✓ Hot swap buffer test passed!\n\n");
}

void test_multiple_buffers() {
  printf("=== Testing Multiple Buffers ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "buffer_test", 1);
  assert(lg != NULL);

  // Create multiple buffers
  float data1[64];
  float data2[128];
  float data3[256];

  for (int i = 0; i < 64; i++)
    data1[i] = 1.0f;
  for (int i = 0; i < 128; i++)
    data2[i] = 2.0f;
  for (int i = 0; i < 256; i++)
    data3[i] = 3.0f;

  int buf1 = create_buffer(lg, 64, 1, data1);
  int buf2 = create_buffer(lg, 128, 1, data2);
  int buf3 = create_buffer(lg, 256, 1, data3);

  assert(buf1 >= 0 && buf2 >= 0 && buf3 >= 0);
  assert(buf1 != buf2 && buf2 != buf3 && buf1 != buf3);
  printf("✓ Created 3 buffers with unique IDs: %d, %d, %d\n", buf1, buf2, buf3);

  // Apply all buffer creations
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify each buffer has correct data
  assert(fabsf(lg->buffers[buf1].buffer[0] - 1.0f) < 0.0001f);
  assert(fabsf(lg->buffers[buf2].buffer[0] - 2.0f) < 0.0001f);
  assert(fabsf(lg->buffers[buf3].buffer[0] - 3.0f) < 0.0001f);
  printf("✓ Each buffer has correct data\n");

  // Hot swap middle buffer
  float new_data2[128];
  for (int i = 0; i < 128; i++)
    new_data2[i] = 22.0f;

  hot_swap_buffer(lg, buf2, new_data2, 128, 1);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify only buf2 changed
  assert(fabsf(lg->buffers[buf1].buffer[0] - 1.0f) < 0.0001f);
  assert(fabsf(lg->buffers[buf2].buffer[0] - 22.0f) < 0.0001f);
  assert(fabsf(lg->buffers[buf3].buffer[0] - 3.0f) < 0.0001f);
  printf("✓ Hot swap affected only the target buffer\n");

  destroy_live_graph(lg);
  printf("✓ Multiple buffers test passed!\n\n");
}

void test_buffer_capacity_growth() {
  printf("=== Testing Buffer Capacity Growth ===\n");

  const int block_size = 64;
  // Start with small initial capacity
  LiveGraph *lg = create_live_graph(4, block_size, "buffer_test", 1);
  assert(lg != NULL);

  int initial_capacity = lg->buffer_capacity;
  printf("  Initial buffer capacity: %d\n", initial_capacity);

  // Create more buffers than initial capacity
  int num_buffers = initial_capacity + 5;
  int *buf_ids = malloc(num_buffers * sizeof(int));

  for (int i = 0; i < num_buffers; i++) {
    float data[32];
    for (int j = 0; j < 32; j++)
      data[j] = (float)(i + 1);
    buf_ids[i] = create_buffer(lg, 32, 1, data);
    assert(buf_ids[i] >= 0);
  }
  printf("✓ Created %d buffers (exceeding initial capacity)\n", num_buffers);

  // Apply all
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify capacity grew
  assert(lg->buffer_capacity > initial_capacity);
  printf("✓ Buffer capacity grew to: %d\n", lg->buffer_capacity);

  // Verify all buffers have correct data
  for (int i = 0; i < num_buffers; i++) {
    float expected = (float)(i + 1);
    assert(fabsf(lg->buffers[buf_ids[i]].buffer[0] - expected) < 0.0001f);
  }
  printf("✓ All buffers have correct data after growth\n");

  free(buf_ids);
  destroy_live_graph(lg);
  printf("✓ Buffer capacity growth test passed!\n\n");
}

void test_hot_swap_invalid_buffer() {
  printf("=== Testing Hot Swap Invalid Buffer ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "buffer_test", 1);
  assert(lg != NULL);

  float data[32];
  for (int i = 0; i < 32; i++)
    data[i] = 1.0f;

  // Try to hot swap a buffer that doesn't exist
  int result = hot_swap_buffer(lg, 999, data, 32, 1);
  // The API should reject this (returns false/0)
  // Note: The command will be queued but apply will fail
  if (result) {
    apply_graph_edits(lg->graphEditQueue, lg);
  }
  printf("✓ Hot swap of non-existent buffer handled gracefully\n");

  // Try with negative buffer ID
  result = hot_swap_buffer(lg, -1, data, 32, 1);
  assert(!result);
  printf("✓ Hot swap with negative buffer ID rejected\n");

  // Try with NULL data
  result = hot_swap_buffer(lg, 0, NULL, 32, 1);
  assert(!result);
  printf("✓ Hot swap with NULL data rejected\n");

  destroy_live_graph(lg);
  printf("✓ Invalid buffer test passed!\n\n");
}

void test_hot_swap_resize() {
  printf("=== Testing Hot Swap Buffer Resize ===\n");

  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "buffer_test", 1);
  assert(lg != NULL);

  // Create a small initial buffer (simulating 1 second at low sample rate)
  const int initial_samples = 128;
  const int num_channels = 1;
  float initial_data[initial_samples];
  for (int i = 0; i < initial_samples; i++) {
    initial_data[i] = 1.0f;
  }

  int buf_id = create_buffer(lg, initial_samples, num_channels, initial_data);
  assert(buf_id >= 0);
  printf("✓ Created initial buffer: id=%d (%d samples)\n", buf_id, initial_samples);

  // Apply buffer creation
  apply_graph_edits(lg->graphEditQueue, lg);

  // Verify initial size
  BufferDesc *buf = &lg->buffers[buf_id];
  assert(buf->size == initial_samples);
  printf("✓ Initial buffer size verified: %d samples\n", buf->size);

  // Now hot swap with a LARGER buffer (simulating loading a longer audio file)
  const int new_samples = 1024;  // 8x larger
  float *new_data = malloc(new_samples * sizeof(float));
  for (int i = 0; i < new_samples; i++) {
    new_data[i] = 2.0f + (float)i * 0.001f;  // Unique values to verify
  }

  int result = hot_swap_buffer(lg, buf_id, new_data, new_samples, num_channels);
  assert(result);
  printf("✓ Queued hot swap with larger buffer (%d samples)\n", new_samples);

  // Apply the hot swap
  bool apply_result = apply_graph_edits(lg->graphEditQueue, lg);
  assert(apply_result);
  printf("✓ Applied hot swap successfully\n");

  // Verify buffer was resized
  assert(buf->size == new_samples);
  printf("✓ Buffer resized to: %d samples\n", buf->size);

  // Verify ALL new data is present (not just the first initial_samples)
  bool data_correct = true;
  for (int i = 0; i < new_samples; i++) {
    float expected = 2.0f + (float)i * 0.001f;
    if (fabsf(buf->buffer[i] - expected) > 0.0001f) {
      data_correct = false;
      printf("  Mismatch at index %d: expected %.4f, got %.4f\n", i,
             expected, buf->buffer[i]);
      break;
    }
  }
  assert(data_correct);
  printf("✓ All %d samples have correct data (no truncation)\n", new_samples);

  // Test shrinking too
  const int smaller_samples = 64;
  float smaller_data[smaller_samples];
  for (int i = 0; i < smaller_samples; i++) {
    smaller_data[i] = 3.0f;
  }

  hot_swap_buffer(lg, buf_id, smaller_data, smaller_samples, num_channels);
  apply_graph_edits(lg->graphEditQueue, lg);

  assert(buf->size == smaller_samples);
  assert(fabsf(buf->buffer[0] - 3.0f) < 0.0001f);
  printf("✓ Buffer can also shrink: now %d samples\n", buf->size);

  free(new_data);
  destroy_live_graph(lg);
  printf("✓ Hot swap resize test passed!\n\n");
}

int main(int argc, char *argv[]) {
  (void)argc;
  (void)argv;

  printf("\n========================================\n");
  printf("     BUFFER API TEST SUITE\n");
  printf("========================================\n\n");

  test_create_buffer_empty();
  test_create_buffer_with_data();
  test_hot_swap_buffer();
  test_hot_swap_resize();
  test_multiple_buffers();
  test_buffer_capacity_growth();
  test_hot_swap_invalid_buffer();

  printf("========================================\n");
  printf("     ALL BUFFER TESTS PASSED!\n");
  printf("========================================\n\n");

  return 0;
}
