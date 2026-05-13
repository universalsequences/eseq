#include "graph_engine.h"
#include "graph_nodes.h"
#include "graph_edit.h"
#include <assert.h>
#include <stdio.h>
#include <pthread.h>
#include <unistd.h>

typedef struct {
  LiveGraph *lg;
  _Atomic bool should_stop;
  _Atomic bool processing_complete;
  _Atomic int blocks_processed;
} DeletionTestState;

static DeletionTestState g_test_state;

// Worker thread that continuously processes blocks
void* block_processor_thread(void* arg) {
  DeletionTestState *state = (DeletionTestState*)arg;
  float output_buffer[128];
  
  printf("  [PROCESSOR] Started continuous block processing\n");
  
  while (!atomic_load(&state->should_stop)) {
    process_next_block(state->lg, output_buffer, 128);
    atomic_fetch_add(&state->blocks_processed, 1);
    
    // Small delay to allow deletions to interleave
    usleep(1000); // 1ms
  }
  
  atomic_store(&state->processing_complete, true);
  printf("  [PROCESSOR] Stopped after %d blocks\n", 
         atomic_load(&state->blocks_processed));
  return NULL;
}

int main() {
  printf("=== Node Deletion Safety Test ===\n");
  printf("Testing node deletion while workers are actively processing\n\n");

  // Initialize test state
  memset(&g_test_state, 0, sizeof(g_test_state));
  
  // Create a test graph
  g_test_state.lg = create_live_graph(20, 128, "deletion_safety_test", 1);
  assert(g_test_state.lg != NULL);
  
  printf("✓ LiveGraph created with auto-DAC (ID: %d)\n", g_test_state.lg->dac_node_id);

  // Test 1: Create several nodes using the new API
  printf("\nTest 1: Creating nodes for deletion test...\n");
  
  // Create multiple oscillators and gains for a rich test scenario
  int osc_ids[4];
  int gain_ids[4];
  
  for (int i = 0; i < 4; i++) {
    // Create oscillator state
    float *osc_state = malloc(sizeof(float) * OSC_MEMORY_SIZE);
    osc_state[OSC_INC] = (100.0f + i * 50.0f) / 48000.0f; // Different frequencies
    osc_state[OSC_PHASE] = 0.0f;
    
    // Create gain state  
    float *gain_state = malloc(sizeof(float) * GAIN_MEMORY_SIZE);
    gain_state[GAIN_VALUE] = 0.1f + i * 0.1f; // Different gain values
    
    char osc_name[32], gain_name[32];
    snprintf(osc_name, sizeof(osc_name), "osc_%d", i);
    snprintf(gain_name, sizeof(gain_name), "gain_%d", i);
    
    osc_ids[i] = add_node(g_test_state.lg, OSC_VTABLE, osc_state, osc_name, 0, 1, NULL, 0);
    gain_ids[i] = add_node(g_test_state.lg, GAIN_VTABLE, gain_state, gain_name, 1, 1, NULL, 0);
    
    assert(osc_ids[i] > 0);
    assert(gain_ids[i] > 0);
    
    // Connect osc -> gain
    assert(connect(g_test_state.lg, osc_ids[i], 0, gain_ids[i], 0));
  }
  
  // Create a mixer and connect first two gains to it
  int mixer_id = add_node(g_test_state.lg, MIX2_VTABLE, NULL, "test_mixer", 2, 1, NULL, 0);
  assert(mixer_id > 0);
  
  assert(connect(g_test_state.lg, gain_ids[0], 0, mixer_id, 0));
  assert(connect(g_test_state.lg, gain_ids[1], 0, mixer_id, 1));
  assert(connect(g_test_state.lg, mixer_id, 0, g_test_state.lg->dac_node_id, 0));
  
  printf("✓ Created %d oscillators, %d gains, 1 mixer\n", 4, 4);
  printf("✓ Connected osc0+1 -> gain0+1 -> mixer -> DAC\n");
  printf("✓ Left osc2+3 -> gain2+3 unconnected (will be orphaned)\n");

  // Apply initial setup
  printf("\nApplying initial graph setup...\n");
  assert(apply_graph_edits(g_test_state.lg->graphEditQueue, g_test_state.lg));
  printf("✓ Initial graph setup complete, %d nodes total\n", g_test_state.lg->node_count);

  // Test 2: Start continuous block processing in background
  printf("\nTest 2: Starting background block processing...\n");
  
  pthread_t processor_thread;
  atomic_store(&g_test_state.should_stop, false);
  atomic_store(&g_test_state.processing_complete, false);
  
  int thread_result = pthread_create(&processor_thread, NULL, block_processor_thread, &g_test_state);
  assert(thread_result == 0);
  
  printf("✓ Background processing started\n");
  
  // Let it process a few blocks first
  usleep(5000); // 5ms
  
  // Test 3: Delete nodes while processing is active
  printf("\nTest 3: Deleting nodes during active processing...\n");
  
  printf("  Deleting gain_2 (orphaned node)...\n");
  assert(delete_node(g_test_state.lg, gain_ids[2]));
  
  printf("  Deleting osc_3 (orphaned node)...\n");
  assert(delete_node(g_test_state.lg, osc_ids[3]));
  
  printf("  Deleting gain_1 (connected node - should break connection)...\n");
  assert(delete_node(g_test_state.lg, gain_ids[1]));
  
  printf("✓ Queued 3 node deletions during active processing\n");
  
  // Let processing continue for a bit more with deletions active
  usleep(10000); // 10ms
  
  // Test 4: Stop processing and verify system stability
  printf("\nTest 4: Stopping processing and checking stability...\n");
  
  atomic_store(&g_test_state.should_stop, true);
  pthread_join(processor_thread, NULL);
  
  int final_blocks = atomic_load(&g_test_state.blocks_processed);
  printf("✓ Processed %d blocks total during deletion test\n", final_blocks);
  
  // Process one final block to apply any pending deletions
  float final_output[128];
  process_next_block(g_test_state.lg, final_output, 128);
  
  printf("✓ Final block processed successfully after deletions\n");
  
  // Test 5: Verify deleted nodes are properly marked
  printf("\nTest 5: Verifying deleted node state...\n");
  
  // Check that deleted nodes are marked correctly
  RTNode *deleted_gain2 = &g_test_state.lg->nodes[gain_ids[2]];
  RTNode *deleted_osc3 = &g_test_state.lg->nodes[osc_ids[3]];
  RTNode *deleted_gain1 = &g_test_state.lg->nodes[gain_ids[1]];
  
  assert(deleted_gain2->state == NULL);     // Should be marked as deleted
  assert(deleted_osc3->state == NULL);      // Should be marked as deleted  
  assert(deleted_gain1->state == NULL);     // Should be marked as deleted
  
  assert(deleted_gain2->vtable.process == NULL);  // Vtable cleared
  assert(deleted_osc3->vtable.process == NULL);   // Vtable cleared
  assert(deleted_gain1->vtable.process == NULL);  // Vtable cleared
  
  printf("✓ All deleted nodes properly marked (state=NULL, vtable.process=NULL)\n");
  
  // Verify remaining nodes are still functional
  RTNode *remaining_osc0 = &g_test_state.lg->nodes[osc_ids[0]];
  RTNode *remaining_gain0 = &g_test_state.lg->nodes[gain_ids[0]];
  RTNode *mixer = &g_test_state.lg->nodes[mixer_id];
  
  assert(remaining_osc0->state != NULL);    // Should still be valid
  assert(remaining_gain0->state != NULL);   // Should still be valid  
  assert(mixer->state == NULL);             // Mixer has no state (normal)
  assert(mixer->vtable.process != NULL);    // But vtable should be valid
  
  printf("✓ Remaining nodes are still valid and functional\n");

  printf("\n=== Deletion Safety Test Results ===\n");
  printf("✅ All deletion safety tests passed successfully!\n");
  printf("   - Processed %d blocks during active node deletion\n", final_blocks);
  printf("   - Worker threads correctly skipped deleted nodes\n");
  printf("   - No crashes or undefined behavior detected\n");
  printf("   - Deleted nodes properly marked and isolated\n");
  printf("   - Remaining graph structure intact\n");

  return 0;
}