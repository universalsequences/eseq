#include <assert.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"

// Custom node states (same as 4-node topology test)
typedef struct {
    float value;
} NumberGenState;

typedef struct {
    float value1;
    float value2;
} DualOutputState;

typedef struct {
    float dummy;
} MultiplierState;

// Custom node process functions
static void number_gen_process(float *const *inputs, float *const *outputs,
                              int block_size, void *state) {
    (void)inputs;
    NumberGenState *s = (NumberGenState *)state;
    
    for (int i = 0; i < block_size; i++) {
        outputs[0][i] = s->value;
    }
}

static void dual_output_process(float *const *inputs, float *const *outputs,
                               int block_size, void *state) {
    (void)inputs;
    DualOutputState *s = (DualOutputState *)state;
    
    for (int i = 0; i < block_size; i++) {
        outputs[0][i] = s->value1;
        outputs[1][i] = s->value2;
    }
}

static void multiplier_process(float *const *inputs, float *const *outputs,
                              int block_size, void *state) {
    (void)state;
    
    for (int i = 0; i < block_size; i++) {
        outputs[0][i] = inputs[0][i] * inputs[1][i];
    }
}

// Custom node init functions
static void number_gen_init(void *state, int sr, int mb, const void *initial_state) {
    (void)sr; (void)mb;
    NumberGenState *s = (NumberGenState *)state;
    s->value = 1.0f;
}

static void dual_output_init(void *state, int sr, int mb, const void *initial_state) {
    (void)sr; (void)mb;
    DualOutputState *s = (DualOutputState *)state;
    s->value1 = 2.0f;
    s->value2 = 3.0f;  // This should appear at DAC when N2→DAC is connected
}

static void multiplier_init(void *state, int sr, int mb, const void *initial_state) {
    (void)sr; (void)mb; (void)state;
}

// Custom VTables
static const NodeVTable NUMBER_GEN_VTABLE = {
    .init = number_gen_init,
    .process = number_gen_process,
    .reset = NULL,
    .migrate = NULL
};

static const NodeVTable DUAL_OUTPUT_VTABLE = {
    .init = dual_output_init,
    .process = dual_output_process,
    .reset = NULL,
    .migrate = NULL
};

static const NodeVTable MULTIPLIER_VTABLE = {
    .init = multiplier_init,
    .process = multiplier_process,
    .reset = NULL,
    .migrate = NULL
};

// Global state (like the fuzz test)
static LiveGraph *lg = NULL;
static int node1_id, node2_id, node3_id, node4_id;

// Helper function to reset all edges to connected state
static bool reset_to_full_topology() {
    printf("🔄 Resetting to full topology...\n");
    
    // Connect all 6 edges
    bool success = true;
    success &= graph_connect(lg, node1_id, 0, node3_id, 0);     // N1→N3
    success &= graph_connect(lg, node1_id, 0, node4_id, 0);     // N1→N4
    success &= graph_connect(lg, node2_id, 0, node3_id, 1);     // N2→N3
    success &= apply_connect(lg, node2_id, 1, lg->dac_node_id, 0); // N2→DAC
    success &= graph_connect(lg, node3_id, 0, lg->dac_node_id, 0); // N3→DAC
    success &= graph_connect(lg, node4_id, 0, lg->dac_node_id, 0); // N4→DAC
    
    if (success) {
        apply_graph_edits(lg->graphEditQueue, lg);
    }
    
    return success;
}

// Helper function to check DAC state
static void check_dac_state(const char* context) {
    printf("🔍 %s - DAC indegree: %d\n", context, lg->sched.indegree[lg->dac_node_id]);
    
    // Check DAC input connections
    RTNode *dac = &lg->nodes[lg->dac_node_id];
    int active_connections = 0;
    for (int i = 0; i < dac->nInputs; i++) {
        if (dac->inEdgeId[i] >= 0) {
            active_connections++;
        }
    }
    printf("     Active DAC input connections: %d\n", active_connections);
    
    // Process audio and check output
    const int block_size = 256;
    float output_buffer[block_size];
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    printf("     DAC output: %.3f\n", output_buffer[0]);
}

// Perform a disconnection/reconnection cycle that might corrupt state
static bool perform_corruption_cycle(int cycle_num) {
    printf("\n--- CORRUPTION CYCLE %d ---\n", cycle_num);
    
    // Disconnect and reconnect edges in a pattern that might cause corruption
    bool success = true;
    
    // Pattern 1: Disconnect N3→DAC, then reconnect
    success &= graph_disconnect(lg, node3_id, 0, lg->dac_node_id, 0);
    apply_graph_edits(lg->graphEditQueue, lg);
    check_dac_state("After N3→DAC disconnect");
    
    success &= graph_connect(lg, node3_id, 0, lg->dac_node_id, 0);
    apply_graph_edits(lg->graphEditQueue, lg);
    check_dac_state("After N3→DAC reconnect");
    
    // Pattern 2: Disconnect N1→N3, then reconnect
    success &= graph_disconnect(lg, node1_id, 0, node3_id, 0);
    apply_graph_edits(lg->graphEditQueue, lg);
    check_dac_state("After N1→N3 disconnect");
    
    success &= graph_connect(lg, node1_id, 0, node3_id, 0);
    apply_graph_edits(lg->graphEditQueue, lg);
    check_dac_state("After N1→N3 reconnect");
    
    // Pattern 3: Multiple edge disconnect/reconnect
    success &= graph_disconnect(lg, node2_id, 0, node3_id, 1);
    success &= graph_disconnect(lg, node4_id, 0, lg->dac_node_id, 0);
    apply_graph_edits(lg->graphEditQueue, lg);
    check_dac_state("After double disconnect");
    
    success &= graph_connect(lg, node2_id, 0, node3_id, 1);
    success &= graph_connect(lg, node4_id, 0, lg->dac_node_id, 0);
    apply_graph_edits(lg->graphEditQueue, lg);
    check_dac_state("After double reconnect");
    
    return success;
}

// Test that reuses same graph instance like the fuzz test
int main() {
    printf("🐛 Graph Reuse State Corruption Test\n");
    printf("=====================================\n");
    printf("This test reuses the same LiveGraph instance across many operations\n");
    printf("to reproduce the cumulative state corruption that causes the bug.\n\n");
    
    const int block_size = 256;
    lg = create_live_graph(32, block_size, "graph_reuse_bug_test", 1);
    if (!lg) {
        printf("❌ Failed to create live graph\n");
        return 1;
    }
    
    // Create nodes (same as fuzz test)
    node1_id = add_node(lg, NUMBER_GEN_VTABLE, sizeof(NumberGenState), "number_gen", 0, 1);
    node2_id = add_node(lg, DUAL_OUTPUT_VTABLE, sizeof(DualOutputState), "dual_output", 0, 2);
    node3_id = add_node(lg, MULTIPLIER_VTABLE, sizeof(MultiplierState), "multiplier", 2, 1);
    node4_id = live_add_gain(lg, 0.5f, "gain");
    
    if (node1_id < 0 || node2_id < 0 || node3_id < 0 || node4_id < 0) {
        printf("❌ Failed to create nodes\n");
        return 1;
    }
    
    apply_graph_edits(lg->graphEditQueue, lg);
    
    printf("✅ Nodes created: N1=%d, N2=%d, N3=%d, N4=%d, DAC=%d\n",
           node1_id, node2_id, node3_id, node4_id, lg->dac_node_id);
    
    // Set up initial topology
    if (!reset_to_full_topology()) {
        printf("❌ Failed to create initial topology\n");
        return 1;
    }
    
    check_dac_state("Initial state");
    
    // Perform many corruption cycles to build up state corruption
    const int NUM_CYCLES = 100;  // Simulate many operations like in fuzz test
    
    for (int i = 1; i <= NUM_CYCLES; i++) {
        if (!perform_corruption_cycle(i)) {
            printf("❌ Corruption cycle %d failed\n", i);
            break;
        }
        
        // Every 20 cycles, check if we've corrupted the state
        if (i % 20 == 0) {
            printf("\n🔬 CORRUPTION CHECK AFTER %d CYCLES:\n", i);
            check_dac_state("After corruption cycles");
            
            // Test the exact failing pattern from the fuzz test
            // Disconnect N3→DAC to simulate the state that should cause the bug
            bool success = graph_disconnect(lg, node3_id, 0, lg->dac_node_id, 0);
            if (success) {
                apply_graph_edits(lg->graphEditQueue, lg);
                check_dac_state("Test pattern - N3→DAC disconnected");
                
                // Check if we have the bug: DAC indegree=0 but N2→DAC still connected
                RTNode *dac = &lg->nodes[lg->dac_node_id];
                int active_connections = 0;
                for (int j = 0; j < dac->nInputs; j++) {
                    if (dac->inEdgeId[j] >= 0) {
                        active_connections++;
                    }
                }
                
                if (lg->sched.indegree[lg->dac_node_id] == 0 && active_connections > 0) {
                    printf("🐛 BUG REPRODUCED! DAC indegree=0 but has %d active connections!\n", active_connections);
                    
                    // Process audio to see if output is wrong
                    float output_buffer[block_size];
                    memset(output_buffer, 0, sizeof(output_buffer));
                    process_next_block(lg, output_buffer, block_size);
                    printf("   Expected output: 3.000 (from N2→DAC)\n");
                    printf("   Actual output: %.3f\n", output_buffer[0]);
                    
                    if (fabs(output_buffer[0] - 3.0f) > 0.001f) {
                        printf("🎯 OUTPUT BUG CONFIRMED! Incorrect audio output due to indegree corruption.\n");
                        destroy_live_graph(lg);
                        return 1; // Bug reproduced successfully
                    }
                }
                
                // Reconnect for next cycle
                graph_connect(lg, node3_id, 0, lg->dac_node_id, 0);
                apply_graph_edits(lg->graphEditQueue, lg);
            }
        }
    }
    
    printf("\n🏁 Test completed after %d corruption cycles\n", NUM_CYCLES);
    printf("✅ No state corruption detected - indegree tracking appears robust\n");
    
    destroy_live_graph(lg);
    return 0;
}