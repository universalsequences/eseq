#include <assert.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"

// Reproduce the first bug found by the fuzz test:
// Perm_617_Initial-N1→N4-N4→DAC-N2→N3-N3→DAC
// Expected: 3.000, Actual: 0.000

int main() {
    printf("🐛 Minimal Disconnection Bug Reproduction\n");
    printf("==========================================\n\n");
    printf("Reproducing: Perm_617_Initial-N1→N4-N4→DAC-N2→N3-N3→DAC\n");
    printf("Expected DAC: 3.000, Fuzz test got: 0.000\n\n");
    
    const int block_size = 256;
    LiveGraph *lg = create_live_graph(32, block_size, "bug_reproduction", 1);
    assert(lg != NULL);
    
    // Create the same 4-node topology
    int node1_id = live_add_number(lg, 1.0f, "number_gen");
    int node2_id = live_add_number(lg, 2.0f, "dual_output"); // Simplified to single output for this test
    int node3_id = live_add_gain(lg, 1.0f, "multiplier"); // Simplified to gain node
    int node4_id = live_add_gain(lg, 0.5f, "gain");
    
    apply_graph_edits(lg->graphEditQueue, lg);
    printf("✅ Created nodes: N1=%d, N2=%d, N3=%d, N4=%d, DAC=%d\n",
           node1_id, node2_id, node3_id, node4_id, lg->dac_node_id);
    
    // Create ALL edges initially (same as fuzz test)
    printf("\n🔗 Creating initial connections...\n");
    assert(graph_connect(lg, node1_id, 0, node3_id, 0)); // N1→N3
    assert(graph_connect(lg, node1_id, 0, node4_id, 0)); // N1→N4  
    assert(graph_connect(lg, node2_id, 0, node3_id, 0)); // N2→N3 (simplified)
    assert(apply_connect(lg, node2_id, 0, lg->dac_node_id, 0)); // N2→DAC
    assert(apply_connect(lg, node3_id, 0, lg->dac_node_id, 0)); // N3→DAC
    assert(apply_connect(lg, node4_id, 0, lg->dac_node_id, 0)); // N4→DAC
    
    apply_graph_edits(lg->graphEditQueue, lg);
    printf("✅ All connections created\n");
    
    // Test initial state
    float output_buffer[block_size];
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    
    float initial_output = output_buffer[0];
    printf("🎵 Initial DAC output: %.3f\n", initial_output);
    
    // Now follow the exact disconnection sequence from the failed test:
    // "Initial-N1→N4-N4→DAC-N2→N3-N3→DAC"
    
    printf("\n🔌 Step 1: Disconnect N1→N4\n");
    assert(graph_disconnect(lg, node1_id, 0, node4_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    printf("   DAC output after N1→N4 disconnect: %.3f\n", output_buffer[0]);
    
    printf("🔌 Step 2: Disconnect N4→DAC\n");
    assert(apply_disconnect(lg, node4_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    printf("   DAC output after N4→DAC disconnect: %.3f\n", output_buffer[0]);
    
    printf("🔌 Step 3: Disconnect N2→N3\n");
    assert(graph_disconnect(lg, node2_id, 0, node3_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    printf("   DAC output after N2→N3 disconnect: %.3f\n", output_buffer[0]);
    
    printf("🔌 Step 4: Disconnect N3→DAC\n");
    assert(apply_disconnect(lg, node3_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    printf("   DAC output after N3→DAC disconnect: %.3f\n", output_buffer[0]);
    
    // Final state analysis
    printf("\n📊 Final State Analysis:\n");
    printf("   Remaining active edges: N1→N3, N2→DAC\n");
    printf("   Expected calculation:\n");
    printf("   - Node 1 generates: 1.0\n"); 
    printf("   - Node 2 generates: 2.0 → DAC\n");
    printf("   - Node 3 receives: 1.0 from N1, outputs 1.0 × 1.0 = 1.0 (but N3→DAC disconnected)\n");
    printf("   - Expected DAC total: 2.0 (from N2) + 0.0 (N3 disconnected) = 2.0\n");
    printf("   - But fuzz test expected: 3.000 (this suggests a bug in expectation or actual calculation)\n");
    printf("   - Actual DAC output: %.3f\n", output_buffer[0]);
    
    // Check indegrees
    printf("\n🔍 Indegree Analysis:\n");
    printf("   Node 1 indegree: %d (expected: 0)\n", lg->sched.indegree[node1_id]);
    printf("   Node 2 indegree: %d (expected: 0)\n", lg->sched.indegree[node2_id]);  
    printf("   Node 3 indegree: %d (expected: 1, from N1)\n", lg->sched.indegree[node3_id]);
    printf("   Node 4 indegree: %d (expected: 0, disconnected)\n", lg->sched.indegree[node4_id]);
    printf("   DAC indegree: %d (expected: 1, from N2)\n", lg->sched.indegree[lg->dac_node_id]);
    
    // Verify if the bug is reproduced
    bool bug_reproduced = (fabs(output_buffer[0]) < 0.001f) && (output_buffer[0] != 2.0f);
    
    if (bug_reproduced) {
        printf("\n🐛 BUG REPRODUCED! DAC output is %.3f but should be 2.0\n", output_buffer[0]);
        printf("   This confirms the disconnection cleanup bug found by fuzz testing.\n");
    } else {
        printf("\n✅ Bug not reproduced. DAC output: %.3f matches expected behavior.\n", output_buffer[0]);
    }
    
    destroy_live_graph(lg);
    return bug_reproduced ? 1 : 0;
}