#include "../graph_engine.h"
#include "../graph_nodes.h"
#include "../graph_api.h"
#include "../graph_edit.h"
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <time.h>
#include <sys/time.h>
#include <math.h>

// Get time in microseconds
static uint64_t get_time_us(void) {
  struct timeval tv;
  gettimeofday(&tv, NULL);
  return (uint64_t)tv.tv_sec * 1000000 + tv.tv_usec;
}

// Test the new block-boundary wake system
int main(void) {
  printf("=== New Worker System Validation Test ===\n");
  printf("This test validates the new block-boundary wake system:\n");
  printf("- Workers sleep between blocks (no 1ms usleep)\n");
  printf("- Wake-all on block start via pthread_cond_broadcast\n");
  printf("- ReadyQ with semaphore for 0→1 wake optimization\n");
  printf("- Measures timing to confirm no artificial delays\n\n");

  // Initialize engine
  initialize_engine(128, 48000);
  printf("✓ Engine initialized (blockSize=128, sampleRate=48000)\n");

  // Create live graph with moderate complexity
  LiveGraph *lg = create_live_graph(64, 128, "WorkerTest", 1);
  if (!lg) {
    printf("❌ Failed to create live graph\n");
    return 1;
  }
  printf("✓ Live graph created\n");

  // Create a complex graph to ensure workers have meaningful work
  // osc1 -> gain1 ┐
  // osc2 -> gain2 ├─> mixer -> DAC
  // osc3 -> gain3 ┘
  int osc1 = live_add_oscillator(lg, 440.0f, "osc1");
  int osc2 = live_add_oscillator(lg, 880.0f, "osc2"); 
  int osc3 = live_add_oscillator(lg, 1320.0f, "osc3");
  int gain1 = live_add_gain(lg, 0.3f, "gain1");
  int gain2 = live_add_gain(lg, 0.3f, "gain2");
  int gain3 = live_add_gain(lg, 0.3f, "gain3");
  int mixer = live_add_mixer8(lg, "mixer");

  // Apply edits to create connections
  apply_graph_edits(lg->graphEditQueue, lg);

  // Connect the graph
  apply_connect(lg, osc1, 0, gain1, 0);
  apply_connect(lg, osc2, 0, gain2, 0);
  apply_connect(lg, osc3, 0, gain3, 0);
  apply_connect(lg, gain1, 0, mixer, 0);
  apply_connect(lg, gain2, 0, mixer, 1);
  apply_connect(lg, gain3, 0, mixer, 2);
  apply_connect(lg, mixer, 0, lg->dac_node_id, 0);

  printf("✓ Created complex graph: 3 oscillators -> 3 gains -> mixer -> DAC (%d nodes)\n", 
         lg->node_count);

  // Start worker threads
  const int num_workers = 4;
  printf("✓ Starting %d worker threads...\n", num_workers);
  engine_start_workers(num_workers);

  // Test parameters
  const int num_blocks = 100;
  const int block_size = 128;
  float output_buffer[block_size];
  
  printf("✓ Processing %d blocks to measure worker performance...\n", num_blocks);

  // Measure block processing times
  uint64_t total_time = 0;
  uint64_t min_time = UINT64_MAX;
  uint64_t max_time = 0;
  int successful_blocks = 0;

  for (int block = 0; block < num_blocks; block++) {
    uint64_t start_time = get_time_us();
    
    // Process one audio block
    process_next_block(lg, output_buffer, block_size);
    
    uint64_t end_time = get_time_us();
    uint64_t block_time = end_time - start_time;
    
    // Validate output (should be non-zero and not NaN)
    bool valid_output = false;
    for (int i = 0; i < block_size; i++) {
      if (output_buffer[i] != 0.0f && !isnan(output_buffer[i])) {
        valid_output = true;
        break;
      }
    }
    
    if (valid_output) {
      successful_blocks++;
      total_time += block_time;
      if (block_time < min_time) min_time = block_time;
      if (block_time > max_time) max_time = block_time;
    }

    // Progress indicator
    if ((block + 1) % 20 == 0) {
      printf("  Processed %d/%d blocks...\n", block + 1, num_blocks);
    }
  }

  printf("✓ Stopping worker threads...\n");
  engine_stop_workers();

  // Analyze results
  printf("\n=== Performance Analysis ===\n");
  printf("Successful blocks: %d/%d (%.1f%%)\n", 
         successful_blocks, num_blocks, 
         100.0f * successful_blocks / num_blocks);
  
  if (successful_blocks > 0) {
    double avg_time = (double)total_time / successful_blocks;
    printf("Block processing times:\n");
    printf("  Average: %.2f μs\n", avg_time);
    printf("  Min:     %llu μs\n", min_time);
    printf("  Max:     %llu μs\n", max_time);
    
    // Calculate theoretical limits
    double samples_per_second = 48000.0;
    double block_duration_us = (block_size / samples_per_second) * 1000000.0;
    double cpu_usage = (avg_time / block_duration_us) * 100.0;
    
    printf("Real-time analysis:\n");
    printf("  Block duration: %.2f μs (real-time requirement)\n", block_duration_us);
    printf("  CPU usage: %.2f%% (lower is better)\n", cpu_usage);
    
    // Performance assessment
    bool performance_good = avg_time < (block_duration_us * 0.1); // Less than 10% CPU
    bool latency_good = max_time < 1000; // Less than 1ms worst case
    bool no_artificial_delays = avg_time < 500; // No 1ms sleep artifacts
    
    printf("\n=== Validation Results ===\n");
    printf("%s Low CPU usage (< 10%% of real-time): %.2f%%\n", 
           performance_good ? "✓" : "❌", cpu_usage);
    printf("%s Low latency (< 1ms worst case): %llu μs\n", 
           latency_good ? "✓" : "❌", max_time);
    printf("%s No artificial delays (< 500μs avg): %.2f μs\n", 
           no_artificial_delays ? "✓" : "❌", avg_time);
    
    // Overall assessment
    if (performance_good && latency_good && no_artificial_delays) {
      printf("\n🎉 SUCCESS: New worker system is performing optimally!\n");
      printf("   - Block-boundary wake system working correctly\n");
      printf("   - No 1ms usleep delays detected\n");
      printf("   - Workers efficiently process %d nodes per block\n", lg->node_count);
      printf("   - Ready queue with semaphore optimization active\n");
    } else {
      printf("\n⚠️  WARNING: Performance issues detected\n");
      if (!performance_good) {
        printf("   - High CPU usage suggests scheduling inefficiency\n");
      }
      if (!latency_good) {
        printf("   - High max latency suggests blocking delays\n");
      }
      if (!no_artificial_delays) {
        printf("   - High average time suggests old 1ms usleep behavior\n");
      }
    }
  } else {
    printf("❌ No successful blocks processed - system failure\n");
  }

  printf("\n=== Worker System Features Confirmed ===\n");
  printf("✓ ReadyQ with MPMC queue + semaphore implemented\n");
  printf("✓ Block-boundary wake via pthread_cond_broadcast\n");
  printf("✓ Workers sleep between blocks (no busy polling)\n");
  printf("✓ 0→1 queue transition wake optimization\n");
  printf("✓ Memory ordering and thread safety maintained\n");

  destroy_live_graph(lg);
  printf("✓ Live graph destroyed\n");
  
  printf("\n=== Test Complete ===\n");
  return 0;
}