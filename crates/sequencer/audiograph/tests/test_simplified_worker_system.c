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

// Test parameters
#define TEST_SAMPLE_RATE 48000
#define TEST_BLOCK_SIZE 128
#define TEST_NUM_BLOCKS 200
#define TEST_NUM_WORKERS 4

// Global test state
static int blocks_processed = 0;
static double total_processing_time = 0.0;

// Get time in microseconds
static uint64_t get_time_us(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (uint64_t)tv.tv_sec * 1000000 + tv.tv_usec;
}

// Create a test graph with dependencies
static LiveGraph* create_test_graph(void) {
    // Initialize engine
    initialize_engine(TEST_BLOCK_SIZE, TEST_SAMPLE_RATE);
    
    // Create live graph with moderate complexity
    LiveGraph *lg = create_live_graph(64, TEST_BLOCK_SIZE, "SimplifiedWorkerTest", 1);
    if (!lg) {
        printf("Failed to create live graph\n");
        return NULL;
    }
    
    // Create a complex graph to test worker coordination
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
    
    return lg;
}

// Test the simplified worker system
static int run_processing_test(LiveGraph *lg) {
    printf("Testing simplified worker system (no audio thread helping)...\n");
    
    if (!lg) {
        printf("No live graph available\n");
        return -1;
    }
    
    uint64_t test_start = get_time_us();
    
    // Create output buffer for processing
    float output_buffer[TEST_BLOCK_SIZE];
    
    // Process multiple blocks
    for (int block = 0; block < TEST_NUM_BLOCKS; block++) {
        uint64_t block_start = get_time_us();
        
        // Process block using the simplified system
        process_live_block(lg, TEST_BLOCK_SIZE);
        
        uint64_t block_end = get_time_us();
        double block_time = (double)(block_end - block_start);
        
        blocks_processed++;
        total_processing_time += block_time;
        
        // Print progress every 50 blocks
        if ((block + 1) % 50 == 0) {
            printf("Processed %d blocks, avg time: %.2f μs\n", 
                   block + 1, total_processing_time / (block + 1));
        }
        
        // Small delay to simulate real-time constraints
        usleep(2000); // 2ms between blocks (less than typical 2.67ms at 48kHz/128)
    }
    
    uint64_t test_end = get_time_us();
    double total_test_time = (double)(test_end - test_start);
    
    printf("Total test time: %.2f ms\n", total_test_time / 1000.0);
    
    return 0;
}

// Test ReadyQ synchronization specifically
static int test_readyq_synchronization(LiveGraph *lg) {
    printf("\nTesting ReadyQ semaphore synchronization...\n");
    
    if (!lg || !lg->sched.readyQueue) {
        printf("No ReadyQ available for testing\n");
        return -1;
    }
    
    ReadyQ *rq = lg->sched.readyQueue;
    
    // Test 1: Empty queue should have length 0
    int qlen = atomic_load_explicit(&rq->qlen, memory_order_acquire);
    if (qlen != 0) {
        printf("ERROR: Initial queue length is %d, expected 0\n", qlen);
        return -1;
    }
    
    // Test 2: Push items and verify length increases
    for (int i = 1; i <= 5; i++) {
        if (!rq_push(rq, i)) {
            printf("ERROR: Failed to push item %d\n", i);
            return -1;
        }
        qlen = atomic_load_explicit(&rq->qlen, memory_order_acquire);
        if (qlen != i) {
            printf("ERROR: After pushing %d items, length is %d\n", i, qlen);
            return -1;
        }
    }
    
    // Test 3: Pop items and verify length decreases
    for (int i = 5; i >= 1; i--) {
        int32_t item;
        if (!rq_try_pop(rq, &item)) {
            printf("ERROR: Failed to pop item when length should be %d\n", i);
            return -1;
        }
        qlen = atomic_load_explicit(&rq->qlen, memory_order_acquire);
        if (qlen != i - 1) {
            printf("ERROR: After popping, expected length %d but got %d\n", i - 1, qlen);
            return -1;
        }
    }
    
    // Test 4: Verify queue is empty again
    qlen = atomic_load_explicit(&rq->qlen, memory_order_acquire);
    if (qlen != 0) {
        printf("ERROR: Final queue length is %d, expected 0\n", qlen);
        return -1;
    }
    
    printf("ReadyQ synchronization test PASSED\n");
    return 0;
}

// Test for race conditions in job processing
static int test_job_processing_races(LiveGraph *lg) {
    printf("\nTesting for race conditions in job processing...\n");
    
    if (!lg) {
        printf("No live graph available\n");
        return -1;
    }
    
    // Process several blocks rapidly to stress test the coordination
    int failed_blocks = 0;
    int inconsistent_states = 0;
    
    for (int block = 0; block < 50; block++) {
        // Reset counters
        int jobs_before = atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire);
        int queue_len_before = atomic_load_explicit(&lg->sched.readyQueue->qlen, memory_order_acquire);
        
        if (jobs_before != 0) {
            printf("WARNING: Block %d started with %d jobs in flight\n", block, jobs_before);
            inconsistent_states++;
        }
        
        if (queue_len_before != 0) {
            printf("WARNING: Block %d started with queue length %d\n", block, queue_len_before);
            inconsistent_states++;
        }
        
        // Process the block
        process_live_block(lg, TEST_BLOCK_SIZE);
        
        // Check final state
        int jobs_after = atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire);
        int queue_len_after = atomic_load_explicit(&lg->sched.readyQueue->qlen, memory_order_acquire);
        
        if (jobs_after != 0) {
            printf("ERROR: Block %d ended with %d jobs in flight\n", block, jobs_after);
            inconsistent_states++;
        }
        
        if (queue_len_after != 0) {
            printf("ERROR: Block %d ended with queue length %d\n", block, queue_len_after);
            inconsistent_states++;
        }
        
        // Brief pause
        usleep(1000); // 1ms
    }
    
    printf("Race condition test completed:\n");
    printf("  Failed blocks: %d/50\n", failed_blocks);
    printf("  Inconsistent states: %d\n", inconsistent_states);
    
    if (failed_blocks == 0 && inconsistent_states == 0) {
        printf("Race condition test PASSED\n");
        return 0;
    } else {
        printf("Race condition test FAILED\n");
        return -1;
    }
}

int main(void) {
    printf("=== Simplified Worker System Test ===\n");
    printf("This test validates the simplified worker system without audio thread helping:\n");
    printf("- Workers handle all processing\n");
    printf("- Audio thread only waits for completion\n");
    printf("- ReadyQ semaphore coordination\n");
    printf("- Race condition detection\n\n");
    
    printf("Block size: %d samples\n", TEST_BLOCK_SIZE);
    printf("Number of workers: %d\n", TEST_NUM_WORKERS);
    printf("Number of test blocks: %d\n", TEST_NUM_BLOCKS);
    printf("\n");
    
    // Create test setup
    LiveGraph *lg = create_test_graph();
    if (!lg) {
        printf("Failed to create test graph\n");
        return 1;
    }
    printf("✓ Created test graph with %d nodes\n", lg->node_count);
    
    // Start worker threads
    printf("✓ Starting %d worker threads...\n", TEST_NUM_WORKERS);
    engine_start_workers(TEST_NUM_WORKERS);
    
    // Run tests
    int test_result = 0;
    
    // Test 1: ReadyQ synchronization
    if (test_readyq_synchronization(lg) != 0) {
        test_result = 1;
    }
    
    // Test 2: Race condition detection
    if (test_job_processing_races(lg) != 0) {
        test_result = 1;
    }
    
    // Test 3: Full processing test
    if (run_processing_test(lg) == 0) {
        // Print final statistics
        printf("\n=== Processing Results ===\n");
        printf("Blocks processed: %d\n", blocks_processed);
        printf("Average block processing time: %.2f μs\n", 
               total_processing_time / blocks_processed);
        
        // Calculate expected real-time constraint (2.67ms for 128 samples at 48kHz)
        double expected_rt_us = (double)TEST_BLOCK_SIZE * 1000000.0 / TEST_SAMPLE_RATE;
        double cpu_usage_percent = (total_processing_time / blocks_processed) / expected_rt_us * 100.0;
        printf("Real-time constraint: %.2f μs per block\n", expected_rt_us);
        printf("CPU usage: %.2f%%\n", cpu_usage_percent);
        
        if (cpu_usage_percent > 50.0) {
            printf("WARNING: High CPU usage detected\n");
        }
    } else {
        test_result = 1;
    }
    
    // Stop workers
    printf("\n✓ Stopping worker threads...\n");
    engine_stop_workers();
    
    // Cleanup
    destroy_live_graph(lg);
    
    if (test_result == 0) {
        printf("\n✅ ALL TESTS PASSED - Simplified worker system is working correctly\n");
    } else {
        printf("\n❌ SOME TESTS FAILED - Issues detected in simplified worker system\n");
    }
    
    return test_result;
}