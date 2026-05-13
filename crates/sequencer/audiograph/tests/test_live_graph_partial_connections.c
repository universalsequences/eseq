/******************************************************************************
 * AudioGraph Live Graph Partial Connections Test
 *
 * This test validates that a LiveGraph can correctly handle a scenario where:
 * - 8 oscillators are created in the graph
 * - Only 2 oscillators are connected to the mixer
 * - 6 oscillators run but don't contribute to output
 * - The system validates output from only the 2 connected oscillators
 *
 * This tests the system's ability to handle unconnected/orphaned nodes
 * gracefully while maintaining correct output validation.
 ******************************************************************************/

#include "graph_api.h"
#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <pthread.h>
#include <sys/sysctl.h>
#include <time.h>

// Test configuration
#define NUM_WORKERS 8     // High worker count to maximize contention
#define NUM_TEST_BLOCKS 3 // Same as other tests for consistency
#define BLOCK_SIZE 128    // Standard audio block size
#define SAMPLE_RATE 48000
#define NUM_OSCILLATORS 8 // Create 8 oscillators
#define NUM_CONNECTED 2   // But only connect 2 to the mixer

// Test state
typedef struct {
  LiveGraph *live_graph;
  _Atomic bool test_running;

  // Node IDs
  int oscillator_nodes[NUM_OSCILLATORS];
  int gain_nodes[NUM_OSCILLATORS];
  int mixer_node;
  int output_node;

  // Output validation - expect output from only 2 connected oscillators
  float reference_output[BLOCK_SIZE];
  _Atomic int corruption_count;
  _Atomic int total_blocks_processed;

  // Performance tracking
  struct timespec start_time, end_time;
  _Atomic uint64_t total_processing_time_ns;
  _Atomic uint64_t max_block_time_ns;
  _Atomic uint64_t min_block_time_ns;
} PartialConnectionsTestState;

static PartialConnectionsTestState g_partial_test;

// Get CPU count for worker sizing
int get_cpu_count() {
  int cpu_count = 1;
  size_t size = sizeof(cpu_count);
  if (sysctlbyname("hw.ncpu", &cpu_count, &size, NULL, 0) != 0) {
    cpu_count = 4; // fallback
  }
  return cpu_count;
}

// Create a live graph with 8 oscillators but only 2 connected
bool create_partial_connections_graph() {
  printf("Creating live graph with %d oscillators (only %d connected)...\n",
         NUM_OSCILLATORS, NUM_CONNECTED);

  g_partial_test.live_graph =
      create_live_graph(32, BLOCK_SIZE, "partial_test_graph", 1);
  if (!g_partial_test.live_graph) {
    printf("ERROR: Failed to create live graph\n");
    return false;
  }

  const float test_freq = 10.0f;       // Low frequency for predictable output
  const float individual_gain = 0.05f; // Low gain per oscillator

  // Create all 8 oscillators and gains
  for (int i = 0; i < NUM_OSCILLATORS; i++) {
    char osc_name[32], gain_name[32];
    snprintf(osc_name, sizeof(osc_name), "osc_%d", i);
    snprintf(gain_name, sizeof(gain_name), "gain_%d", i);

    // All oscillators identical: same frequency, same phase
    g_partial_test.oscillator_nodes[i] =
        live_add_oscillator(g_partial_test.live_graph, test_freq, osc_name);
    g_partial_test.gain_nodes[i] =
        live_add_gain(g_partial_test.live_graph, individual_gain, gain_name);

    if (g_partial_test.oscillator_nodes[i] < 0 ||
        g_partial_test.gain_nodes[i] < 0) {
      printf("ERROR: Failed to create oscillator %d or gain %d\n", i, i);
      return false;
    }

    // Connect osc -> gain for all oscillators
    if (!graph_connect(g_partial_test.live_graph,
                       g_partial_test.oscillator_nodes[i], 0,
                       g_partial_test.gain_nodes[i], 0)) {
      printf("ERROR: Failed to connect oscillator %d to gain %d\\n", i, i);
      return false;
    }
  }

  // Create 2-input mixer (only handles 2 inputs)
  g_partial_test.mixer_node =
      live_add_mixer2(g_partial_test.live_graph, "partial_mixer");
  if (g_partial_test.mixer_node < 0) {
    printf("ERROR: Failed to create 2-input mixer node\n");
    return false;
  }

  // Connect ONLY the first 2 gains to mixer (gains 2-7 will be orphaned)
  printf("Connecting gains 0 and 1 to mixer (gains 2-7 will be orphaned)...\n");
  for (int i = 0; i < NUM_CONNECTED; i++) {
    if (!graph_connect(g_partial_test.live_graph, g_partial_test.gain_nodes[i],
                       0, g_partial_test.mixer_node, i)) {
      printf("ERROR: Failed to connect gain %d to mixer\n", i);
      return false;
    }
    printf("  Connected gain_%d -> mixer\n", i);
  }

  // Use the auto-created DAC node as the final sink for all audio
  g_partial_test.output_node = g_partial_test.live_graph->dac_node_id;

  if (!graph_connect(g_partial_test.live_graph, g_partial_test.mixer_node, 0,
                     g_partial_test.output_node, 0)) {
    printf("ERROR: Failed to connect mixer to DAC\\n");
    return false;
  }

  apply_graph_edits(g_partial_test.live_graph->graphEditQueue,
                    g_partial_test.live_graph);

  printf("Live graph created successfully:\\n");
  printf("  Total nodes: %d\n", g_partial_test.live_graph->node_count);
  printf("  Connected path: osc_0 + osc_1 -> gains -> mixer -> DAC\n");
  printf("  Orphaned nodes: osc_2 through osc_7 (and their gains)\n");
  printf("  Expected output: %d oscillators at %.1f Hz, each with gain %.3f\n",
         NUM_CONNECTED, test_freq, individual_gain);
  printf("  Total expected amplitude: %.3f (%d * %.3f)\n",
         NUM_CONNECTED * individual_gain, NUM_CONNECTED, individual_gain);
  printf("  Note: %d oscillators will run but not contribute to output\n",
         NUM_OSCILLATORS - NUM_CONNECTED);

  return true;
}

// Process one block and validate output consistency
bool process_and_validate_partial_block(int block_num) {
  printf("\n=== PROCESSING PARTIAL BLOCK %d ===\n", block_num);
  printf("Partial block %d: Starting parallel processing with %d nodes\n",
         block_num, g_partial_test.live_graph->node_count);

  uint64_t start_time = nsec_now();

  // Process the live graph - all 8 oscillators will run, but only 2 contribute
  // to output
  process_live_block(g_partial_test.live_graph, BLOCK_SIZE);

  uint64_t end_time = nsec_now();
  uint64_t block_time = end_time - start_time;

  printf("Partial block %d: Completed parallel processing in %.2f μs\n",
         block_num, block_time / 1000.0);

  // Update timing stats
  atomic_fetch_add_explicit(&g_partial_test.total_processing_time_ns,
                            block_time, memory_order_relaxed);

  uint64_t current_max = atomic_load_explicit(&g_partial_test.max_block_time_ns,
                                              memory_order_relaxed);
  while (block_time > current_max &&
         !atomic_compare_exchange_weak_explicit(
             &g_partial_test.max_block_time_ns, &current_max, block_time,
             memory_order_relaxed, memory_order_relaxed))
    ;

  uint64_t current_min = atomic_load_explicit(&g_partial_test.min_block_time_ns,
                                              memory_order_relaxed);
  if (current_min == 0)
    current_min = block_time;
  while (block_time < current_min &&
         !atomic_compare_exchange_weak_explicit(
             &g_partial_test.min_block_time_ns, &current_min, block_time,
             memory_order_relaxed, memory_order_relaxed))
    ;

  // Get the output buffer
  int output_node = find_live_output(g_partial_test.live_graph);
  if (output_node < 0) {
    printf("ERROR: No output node found in live graph\n");
    return false;
  }

  printf("DEBUG: find_live_output() returned node_id=%d (DAC)\n", output_node);
  printf("  DAC node has %d inputs, %d outputs\n",
         g_partial_test.live_graph->nodes[output_node].nInputs,
         g_partial_test.live_graph->nodes[output_node].nOutputs);

  // Debug: Check mixer dependency count and inputs
  printf("  Mixer node (id=%d) has %d inputs, pending count: %d\n",
         g_partial_test.mixer_node,
         g_partial_test.live_graph->nodes[g_partial_test.mixer_node].nInputs,
         atomic_load_explicit(
             &g_partial_test.live_graph->sched.pending[g_partial_test.mixer_node],
             memory_order_acquire));

  if (g_partial_test.live_graph->nodes[output_node].nInputs == 0) {
    printf("ERROR: DAC node has no inputs - can't read signal!\n");
    return false;
  }

  int input_edge_id = g_partial_test.live_graph->nodes[output_node].inEdgeId[0];
  if (input_edge_id < 0) {
    printf("ERROR: DAC node input is not connected!\n");
    return false;
  }
  printf("  Using DAC input edge %d as final audio output\n", input_edge_id);

  float *output = g_partial_test.live_graph->edges[input_edge_id].buf;

  // For the first block, save as reference
  if (block_num == 0) {
    memcpy(g_partial_test.reference_output, output, BLOCK_SIZE * sizeof(float));
    printf("Reference output established: first sample = %.6f\n", output[0]);
    printf("  (Expected: predictable waveform from %d connected 10Hz "
           "oscillators)\n",
           NUM_CONNECTED);
    printf("  Note: %d oscillators are running but orphaned\n",
           NUM_OSCILLATORS - NUM_CONNECTED);
    return true;
  }

  // Validate that output matches expected from only 2 connected oscillators
  // With 10Hz oscillators at 48kHz sample rate, each block advances phase by:
  // 128 samples * (10Hz / 48000Hz) = 128 * 0.000208333 = 0.0266667 phase units
  // Expected output = NUM_CONNECTED * 0.05 * sawtooth(0.0266667 * block_num)
  float expected_phase =
      0.026666667f * block_num; // Phase advancement per block
  while (expected_phase >= 1.0f)
    expected_phase -= 1.0f; // Wrap phase
  float expected_sample =
      2.0f * expected_phase - 1.0f; // Convert to sawtooth [-1, 1]
  float expected_output =
      NUM_CONNECTED * 0.05f * expected_sample; // Only connected oscillators

  // Allow for small floating-point errors
  float deviation = fabs(output[0] - expected_output);
  bool corrupted = false;
  const float max_allowed_deviation =
      0.001f; // Relaxed threshold for floating-point precision

  if (deviation > max_allowed_deviation) {
    printf("ERROR: Partial block %d output doesn't match expected from %d "
           "connected oscillators:\n",
           block_num, NUM_CONNECTED);
    printf("  Actual: %.8f\n", output[0]);
    printf("  Expected: %.8f (phase=%.6f)\n", expected_output, expected_phase);
    printf("  Deviation: %.8f (threshold: %.8f)\n", deviation,
           max_allowed_deviation);
    printf(
        "  This suggests the %d orphaned oscillators are affecting output!\n",
        NUM_OSCILLATORS - NUM_CONNECTED);
    printf("  >>> DEVIATION CORRUPTION DETECTED <<<\n");
    corrupted = true;
  } else {
    printf("Partial block %d: Output matches expected from %d connected "
           "oscillators (deviation: %.8f)\n",
           block_num, NUM_CONNECTED, deviation);
    printf("  Orphaned oscillators correctly excluded from output\n");
  }

  // Check for NaN/infinity (definite signs of corruption)
  for (int i = 0; i < BLOCK_SIZE; i++) {
    if (!isfinite(output[i])) {
      printf("ERROR: Partial block %d sample %d is not finite: %f\n", block_num,
             i, output[i]);
      printf("  >>> NAN/INFINITY CORRUPTION DETECTED <<<\n");
      atomic_fetch_add_explicit(&g_partial_test.corruption_count, 1,
                                memory_order_relaxed);
      corrupted = true;
      break;
    }
  }

  if (corrupted) {
    atomic_fetch_add_explicit(&g_partial_test.corruption_count, 1,
                              memory_order_relaxed);
  }

  return !corrupted;
}

// Main test function
bool test_partial_connections() {
  printf("\\n=== AudioGraph Live Graph Partial Connections Test ===\n");
  printf("Testing scenario with orphaned nodes:\n");
  printf("- %d oscillators created, but only %d connected to output\n",
         NUM_OSCILLATORS, NUM_CONNECTED);
  printf("- %d oscillators will be orphaned (run but don't contribute)\n",
         NUM_OSCILLATORS - NUM_CONNECTED);
  printf("- Output validation expects only %d oscillator contributions\n\n",
         NUM_CONNECTED);

  // Initialize test state
  memset(&g_partial_test, 0, sizeof(g_partial_test));
  atomic_store_explicit(&g_partial_test.min_block_time_ns, UINT64_MAX,
                        memory_order_relaxed);

  // Initialize engine
  g_engine.blockSize = BLOCK_SIZE;
  g_engine.sampleRate = SAMPLE_RATE;

  // Create graph with partial connections
  if (!create_partial_connections_graph()) {
    return false;
  }

  // Determine worker count
  int cpu_count = get_cpu_count();
  int worker_count = (cpu_count > 1) ? cpu_count - 1 : 1;
  if (worker_count > NUM_WORKERS)
    worker_count = NUM_WORKERS;

  printf("\\nStarting %d worker threads (CPU count: %d)...\\n", worker_count,
         cpu_count);

  // Start the engine worker system
  engine_start_workers(worker_count);

  atomic_store_explicit(&g_partial_test.test_running, true,
                        memory_order_release);

  printf("Processing %d blocks with partial connections...\\n",
         NUM_TEST_BLOCKS);
  printf("Each block schedules %d nodes (%d connected, %d orphaned)\\n\\n",
         g_partial_test.live_graph->node_count, NUM_CONNECTED * 3,
         (NUM_OSCILLATORS - NUM_CONNECTED) * 2);

  // Start timing
  clock_gettime(CLOCK_MONOTONIC, &g_partial_test.start_time);

  bool all_blocks_valid = true;

  for (int i = 0; i < g_partial_test.live_graph->node_count; i++) {
    if (g_partial_test.live_graph->sched.is_orphaned[i]) {
      printf("Node %d is orphaned\n", i);
    } else {
      printf("Node %d is used\n", i);
    }
  }
  // Process blocks
  for (int block = 0; block < NUM_TEST_BLOCKS; block++) {
    bool valid = process_and_validate_partial_block(block);
    if (!valid)
      all_blocks_valid = false;

    atomic_fetch_add_explicit(&g_partial_test.total_blocks_processed, 1,
                              memory_order_relaxed);
  }

  clock_gettime(CLOCK_MONOTONIC, &g_partial_test.end_time);
  atomic_store_explicit(&g_partial_test.test_running, false,
                        memory_order_release);

  // Stop workers
  printf("\\nStopping worker threads...\\n");
  engine_stop_workers();

  // Analyze results
  int corruption_count = atomic_load(&g_partial_test.corruption_count);
  int total_blocks = atomic_load(&g_partial_test.total_blocks_processed);
  uint64_t total_time_ns =
      atomic_load(&g_partial_test.total_processing_time_ns);
  uint64_t max_time_ns = atomic_load(&g_partial_test.max_block_time_ns);
  uint64_t min_time_ns = atomic_load(&g_partial_test.min_block_time_ns);

  double duration_ms =
      (g_partial_test.end_time.tv_sec - g_partial_test.start_time.tv_sec) *
          1000.0 +
      (g_partial_test.end_time.tv_nsec - g_partial_test.start_time.tv_nsec) /
          1000000.0;

  double avg_block_time_us = (total_time_ns / 1000.0) / total_blocks;
  double max_block_time_us = max_time_ns / 1000.0;
  double min_block_time_us = min_time_ns / 1000.0;

  printf("\\n=== Partial Connections Test Results ===\\n");
  printf("Test duration: %.2f ms\\n", duration_ms);
  printf("Blocks processed: %d / %d\\n", total_blocks, NUM_TEST_BLOCKS);
  printf("Block timing:\\n");
  printf("  Average: %.2f μs\\n", avg_block_time_us);
  printf("  Min: %.2f μs\\n", min_block_time_us);
  printf("  Max: %.2f μs\\n", max_block_time_us);
  printf("Output validation:\\n");
  printf("  Corrupted blocks: %d / %d (%.2f%%)\\n", corruption_count,
         total_blocks, (float)corruption_count / total_blocks * 100.0f);
  printf("Graph composition:\\n");
  printf("  Total oscillators: %d\\n", NUM_OSCILLATORS);
  printf("  Connected oscillators: %d\\n", NUM_CONNECTED);
  printf("  Orphaned oscillators: %d\\n", NUM_OSCILLATORS - NUM_CONNECTED);

  // Success criteria
  bool success = (corruption_count == 0) && all_blocks_valid &&
                 (total_blocks == NUM_TEST_BLOCKS);

  if (success) {
    printf("\\n✅ SUCCESS: Partial connections test passed!\\n");
    printf("   - No output corruption detected across %d blocks\\n",
           NUM_TEST_BLOCKS);
    printf("   - Only %d connected oscillators contributed to output\\n",
           NUM_CONNECTED);
    printf("   - %d orphaned oscillators ran but were correctly excluded\\n",
           NUM_OSCILLATORS - NUM_CONNECTED);
    printf("   - Multi-threading handled partial connections correctly\\n");
    printf("   - LiveGraph gracefully manages unconnected nodes\\n");
  } else {
    printf("\\n❌ FAILURE: Partial connections test failed!\\n");
    if (corruption_count > 0) {
      printf("   - %d blocks showed output corruption\\n", corruption_count);
      printf("   - Orphaned oscillators may be affecting connected output\\n");
    }
    if (!all_blocks_valid) {
      printf("   - Some blocks had invalid output values\\n");
    }
    if (total_blocks != NUM_TEST_BLOCKS) {
      printf("   - Test did not complete all blocks (%d/%d)\\n", total_blocks,
             NUM_TEST_BLOCKS);
    }
  }

  // Cleanup - TODO: implement proper LiveGraph cleanup function
  // free_live_graph(g_partial_test.live_graph);

  return success;
}

int main() {
  printf("AudioGraph Live Graph Partial Connections Test\n");
  printf("==============================================\n");
  printf("This test validates handling of orphaned nodes:\n");
  printf("- Creates %d oscillators but only connects %d to output\\n",
         NUM_OSCILLATORS, NUM_CONNECTED);
  printf("- Tests that orphaned nodes don't interfere with connected output\n");
  printf("- Validates output matches only the connected oscillators\n");
  printf(
      "- Ensures multi-threading handles partial connections correctly\\n\\n");

  bool success = test_partial_connections();

  if (success) {
    printf("\\n🎉 PARTIAL CONNECTIONS TEST PASSED!\n");
    printf("LiveGraph correctly handles orphaned nodes without affecting "
           "connected output.\\n");
    return 0;
  } else {
    printf("\\n💥 PARTIAL CONNECTIONS TEST FAILED!\n");
    printf("Orphaned nodes are interfering with the connected signal path.\n");
    return 1;
  }
}
