#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <math.h>
#include "../graph_engine.h"
#include "../graph_edit.h"
#include "../graph_api.h"

int main() {
    printf("🧪 Testing Audio Dropout When Adding Unconnected Node (Using Queued Commands)\n");
    printf("==============================================================================\n");

    // Create live graph
    LiveGraph *lg = create_live_graph(16, 256, "test_graph", 1);
    if (!lg) {
        printf("❌ Failed to create live graph\n");
        return 1;
    }

    printf("✓ Created live graph\n");

    // 1. Queue adding 2 number generator nodes using add_node (which uses atomic_fetch_add)
    NodeVTable num1_vtable = create_number_vtable(10.0);
    NodeVTable num2_vtable = create_number_vtable(20.0);
    
    int num1_id = add_node(lg, num1_vtable, sizeof(float) * 8, "num1", 0, 1);
    int num2_id = add_node(lg, num2_vtable, sizeof(float) * 8, "num2", 0, 1);
    
    if (num1_id < 0 || num2_id < 0) {
        printf("❌ Failed to queue number nodes\n");
        destroy_live_graph(lg);
        return 1;
    }
    
    printf("✓ Queued two number generator nodes: num1=%d(10.0), num2=%d(20.0)\n", num1_id, num2_id);

    // 2. Queue connections to DAC using graph_connect
    bool connect1 = graph_connect(lg, num1_id, 0, lg->dac_node_id, 0);
    bool connect2 = graph_connect(lg, num2_id, 0, lg->dac_node_id, 0);
    
    if (!connect1 || !connect2) {
        printf("❌ Failed to queue connections to DAC\n");
        destroy_live_graph(lg);
        return 1;
    }
    
    printf("✓ Queued connections to DAC\n");

    // 3. Apply all queued edits (this is where the real work happens)
    bool edits_applied = apply_graph_edits(lg->graphEditQueue, lg);
    if (!edits_applied) {
        printf("❌ Failed to apply graph edits\n");
        destroy_live_graph(lg);
        return 1;
    }
    
    printf("✓ Applied all queued edits\n");

    // Update orphaned status after connections
    update_orphaned_status(lg);
    printf("✓ Updated orphaned status after connections\n");

    // Verify initial setup
    printf("\nInitial Graph State:\n");
    printf("  DAC node ID: %d\n", lg->dac_node_id);
    printf("  Node count: %d\n", lg->node_count);
    printf("  num1 orphaned: %s\n", lg->sched.is_orphaned[num1_id] ? "YES" : "NO");
    printf("  num2 orphaned: %s\n", lg->sched.is_orphaned[num2_id] ? "YES" : "NO");
    printf("  DAC orphaned: %s\n", lg->sched.is_orphaned[lg->dac_node_id] ? "YES" : "NO");

    // 3. Run process_next_block a few times and verify output
    float output_buffer[256];
    float expected_output = 10.0 + 20.0; // Should be 30.0 (sum of both generators)
    
    printf("\n=== Testing Audio Before Adding Unconnected Node ===\n");
    
    for (int block = 0; block < 3; block++) {
        memset(output_buffer, 0, sizeof(output_buffer));
        process_next_block(lg, output_buffer, 256);
        
        // Check first few samples (should all be the same for constant generators)
        float actual = output_buffer[0];
        printf("Block %d: output[0] = %.6f (expected: %.6f) - %s\n", 
               block + 1, actual, expected_output,
               fabs(actual - expected_output) < 0.001f ? "✓" : "❌");
               
        if (fabs(actual - expected_output) > 0.001f) {
            printf("❌ Audio output incorrect before adding node!\n");
            destroy_live_graph(lg);
            return 1;
        }
    }
    
    printf("✓ Audio working correctly before adding unconnected node\n");

    // 4. Add a new unconnected node using the queuing system (this should conflict with auto-SUM)
    printf("\n=== Adding Unconnected Node Using Queued Commands ===\n");
    
    // Debug DAC connections BEFORE adding node
    printf("DEBUG: DAC connections BEFORE adding node:\n");
    RTNode *dac_before = &lg->nodes[lg->dac_node_id];
    printf("  DAC has %d inputs\n", dac_before->nInputs);
    if (dac_before->inEdgeId) {
        for (int i = 0; i < dac_before->nInputs; i++) {
            int eid = dac_before->inEdgeId[i];
            printf("  DAC input[%d] = edge %d", i, eid);
            if (eid >= 0) {
                int src_node = lg->edges[eid].src_node;
                int src_port = lg->edges[eid].src_port;
                printf(" (from node %d port %d)", src_node, src_port);
            }
            printf("\n");
        }
    }
    
    printf("DEBUG: Current next_node_id: %d\n", lg->next_node_id);
    
    // Queue adding a new unconnected node (using add_node with atomic_fetch_add)
    NodeVTable num3_vtable = create_number_vtable(5.0);
    int num3_id = add_node(lg, num3_vtable, sizeof(float) * 8, "num3", 0, 1);
    
    if (num3_id < 0) {
        printf("❌ Failed to queue third number node\n");
        destroy_live_graph(lg);
        return 1;
    }
    
    printf("✓ Queued unconnected node: num3=%d(5.0)\n", num3_id);
    printf("DEBUG: next_node_id after queuing: %d\n", lg->next_node_id);
    
    // Apply the edit (this is where the conflict would occur with our fix)
    bool num3_applied = apply_graph_edits(lg->graphEditQueue, lg);
    if (!num3_applied) {
        printf("❌ Failed to apply graph edits for num3\n");
        destroy_live_graph(lg);
        return 1;
    }
    
    printf("✓ Applied num3 node creation\n");
    
    // Debug DAC connections AFTER adding node
    printf("DEBUG: DAC connections AFTER adding node:\n");
    RTNode *dac_after = &lg->nodes[lg->dac_node_id];
    printf("  DAC has %d inputs\n", dac_after->nInputs);
    if (dac_after->inEdgeId) {
        for (int i = 0; i < dac_after->nInputs; i++) {
            int eid = dac_after->inEdgeId[i];
            printf("  DAC input[%d] = edge %d", i, eid);
            if (eid >= 0 && eid < lg->edge_capacity) {
                int src_node = lg->edges[eid].src_node;
                int src_port = lg->edges[eid].src_port;
                printf(" (from node %d port %d)", src_node, src_port);
            } else if (eid >= 0) {
                printf(" (INVALID EDGE ID!)");
            }
            printf("\n");
        }
    }
    
    // Note: NOT connecting num3 to anything - it should remain orphaned
    // Note: NOT calling update_orphaned_status() yet to isolate the add_node effect
    
    printf("Graph state after adding unconnected node:\n");
    printf("  Node count: %d\n", lg->node_count);
    printf("  num1 orphaned: %s\n", lg->sched.is_orphaned[num1_id] ? "YES" : "NO");
    printf("  num2 orphaned: %s\n", lg->sched.is_orphaned[num2_id] ? "YES" : "NO");
    printf("  num3 orphaned: %s (expected: YES)\n", lg->sched.is_orphaned[num3_id] ? "YES" : "NO");
    printf("  DAC orphaned: %s\n", lg->sched.is_orphaned[lg->dac_node_id] ? "YES" : "NO");

    // 5. Run process_next_block and confirm output is still correct
    printf("\n=== Testing Audio After Adding Unconnected Node ===\n");
    
    for (int block = 0; block < 5; block++) {
        memset(output_buffer, 0, sizeof(output_buffer));
        process_next_block(lg, output_buffer, 256);
        
        float actual = output_buffer[0];
        printf("Block %d: output[0] = %.6f (expected: %.6f) - %s\n", 
               block + 1, actual, expected_output,
               fabs(actual - expected_output) < 0.001f ? "✓" : "❌");
               
        if (fabs(actual - expected_output) > 0.001f) {
            printf("❌ AUDIO DROPOUT DETECTED! Adding unconnected node broke audio processing!\n");
            printf("   This confirms the bug - unconnected nodes shouldn't affect existing audio\n");
            
            // Additional debugging
            printf("\nDEBUG: Checking scheduling state after dropout:\n");
            printf("  Total jobs scheduled: unknown\n");
            printf("  Ready queue state: unknown\n");
            
            destroy_live_graph(lg);
            return 1;
        }
    }
    
    printf("✓ Audio continues working correctly after adding unconnected node\n");

    // Additional test: Now call update_orphaned_status and verify it doesn't break things
    printf("\n=== Testing After update_orphaned_status() Call ===\n");
    
    // Debug the DAC connections before calling update_orphaned_status
    printf("DEBUG: Before update_orphaned_status():\n");
    RTNode *dac = &lg->nodes[lg->dac_node_id];
    printf("  DAC has %d inputs\n", dac->nInputs);
    if (dac->inEdgeId) {
        for (int i = 0; i < dac->nInputs; i++) {
            int eid = dac->inEdgeId[i];
            printf("  DAC input[%d] = edge %d", i, eid);
            if (eid >= 0) {
                int src_node = lg->edges[eid].src_node;
                int src_port = lg->edges[eid].src_port;
                printf(" (from node %d port %d)", src_node, src_port);
            }
            printf("\n");
        }
    }
    
    update_orphaned_status(lg);
    printf("✓ Called update_orphaned_status()\n");
    
    printf("Graph state after update_orphaned_status:\n");
    printf("  num1 orphaned: %s\n", lg->sched.is_orphaned[num1_id] ? "YES" : "NO");
    printf("  num2 orphaned: %s\n", lg->sched.is_orphaned[num2_id] ? "YES" : "NO");
    printf("  num3 orphaned: %s (expected: YES)\n", lg->sched.is_orphaned[num3_id] ? "YES" : "NO");
    printf("  DAC orphaned: %s\n", lg->sched.is_orphaned[lg->dac_node_id] ? "YES" : "NO");
    
    for (int block = 0; block < 2; block++) {
        memset(output_buffer, 0, sizeof(output_buffer));
        process_next_block(lg, output_buffer, 256);
        
        float actual = output_buffer[0];
        printf("Block %d: output[0] = %.6f (expected: %.6f) - %s\n", 
               block + 1, actual, expected_output,
               fabs(actual - expected_output) < 0.001f ? "✓" : "❌");
               
        if (fabs(actual - expected_output) > 0.001f) {
            printf("❌ AUDIO DROPOUT DETECTED after update_orphaned_status()!\n");
            destroy_live_graph(lg);
            return 1;
        }
    }
    
    printf("✓ Audio still working after update_orphaned_status()\n");

    // Cleanup
    destroy_live_graph(lg);
    
    printf("\n🎉 Test completed successfully - no audio dropout detected!\n");
    printf("   (If you're seeing dropouts in your patch editor, the bug may be elsewhere)\n");
    
    return 0;
}