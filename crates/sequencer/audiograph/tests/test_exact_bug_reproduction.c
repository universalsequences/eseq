#include <assert.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"

// Exact reproduction of the first fuzz test failure:
// Perm_617_Initial-N1→N4-N4→DAC-N2→N3-N3→DAC
// Expected DAC: 3.000, Actual: 0.000
//
// This uses the EXACT same node types and topology as the fuzz test

// Custom node states (identical to fuzz test)
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

// Custom node process functions (identical to fuzz test)
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

// Custom node init functions (identical to fuzz test)
static void number_gen_init(void *state, int sr, int mb, const void *initial_state) {
    (void)sr; (void)mb;
    NumberGenState *s = (NumberGenState *)state;
    s->value = 1.0f;
}

static void dual_output_init(void *state, int sr, int mb, const void *initial_state) {
    (void)sr; (void)mb;
    DualOutputState *s = (DualOutputState *)state;
    s->value1 = 2.0f;
    s->value2 = 3.0f;
}

static void multiplier_init(void *state, int sr, int mb, const void *initial_state) {
    (void)sr; (void)mb; (void)state;
}

// Custom VTables (identical to fuzz test)
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

// Global variables for debugging
static LiveGraph *lg = NULL;
static int node1_id, node2_id, node3_id, node4_id;
static float expected_dac_output = 3.0f;

// Calculate expected output based on active topology
static float calculate_expected_output() {
    // After the disconnection sequence: Initial-N1→N4-N4→DAC-N2→N3-N3→DAC
    // Active edges should be: N1→N3, N2→DAC
    //
    // Node 1 generates: 1.0
    // Node 2 generates: 2.0 (port 0) and 3.0 (port 1)  
    // Node 3 receives: 1.0 * 2.0 = 2.0 (but N3→DAC is disconnected)
    // Node 4 receives: nothing (N1→N4 disconnected)
    //
    // DAC receives: 3.0 (from Node 2 port 1)
    // 
    // Wait - let me check the fuzz test calculation more carefully...
    return 3.0f; // This is what the fuzz test expected
}

// Validation function with detailed debugging
static bool validate_final_state() {
    const int block_size = 256;
    float output_buffer[block_size];
    memset(output_buffer, 0, sizeof(output_buffer));
    
    printf("🔍 Processing audio to check final state...\n");
    process_next_block(lg, output_buffer, block_size);
    
    float actual_output = output_buffer[0];
    printf("   Actual DAC output: %.6f\n", actual_output);
    printf("   Expected DAC output: %.6f\n", expected_dac_output);
    printf("   Difference: %.6f\n", actual_output - expected_dac_output);
    
    // Check indegrees for anomalies
    printf("\n📊 Indegree Analysis:\n");
    printf("   Node 1 indegree: %d (should be 0)\n", lg->sched.indegree[node1_id]);
    printf("   Node 2 indegree: %d (should be 0)\n", lg->sched.indegree[node2_id]);
    printf("   Node 3 indegree: %d (should be 1)\n", lg->sched.indegree[node3_id]);
    printf("   Node 4 indegree: %d (should be 0)\n", lg->sched.indegree[node4_id]);
    printf("   DAC indegree: %d (should be 1)\n", lg->sched.indegree[lg->dac_node_id]);
    
    // Check for orphaned or invalid nodes
    printf("\n🔍 Node State Analysis:\n");
    for (int i = 0; i <= 4; i++) {
        if (i <= lg->node_count) {
            printf("   Node %d: inputs=%d, outputs=%d\n",
                   i, lg->nodes[i].nInputs, lg->nodes[i].nOutputs);
        }
    }
    
    // The bug condition
    bool has_bug = (fabs(actual_output) < 0.001f) && (expected_dac_output > 0.001f);
    
    if (has_bug) {
        printf("\n🐛 BUG CONFIRMED: DAC output is near zero but should be %.3f\n", expected_dac_output);
        printf("   This indicates a disconnection cleanup bug in the auto-sum system.\n");
    } else if (fabs(actual_output - expected_dac_output) > 0.001f) {
        printf("\n⚠️  OUTPUT MISMATCH: Got %.3f, expected %.3f\n", actual_output, expected_dac_output);
        printf("   This may indicate a calculation error or different bug.\n");
    } else {
        printf("\n✅ Output matches expectation: %.3f\n", actual_output);
    }
    
    return has_bug;
}

int main() {
    printf("🐛 Exact Bug Reproduction - Fuzz Test Case #617\n");
    printf("=================================================\n\n");
    printf("Reproducing exact failure: Perm_617_Initial-N1→N4-N4→DAC-N2→N3-N3→DAC\n");
    printf("Expected behavior: DAC should output 3.000\n");
    printf("Observed bug: DAC outputs 0.000 instead\n\n");
    
    // Setup identical to fuzz test
    const int block_size = 256;
    lg = create_live_graph(32, block_size, "exact_bug_reproduction", 1);
    assert(lg != NULL);
    
    printf("🏗️  Creating exact 4-node topology from fuzz test...\n");
    
    // Node 1: Simple number generator (generates 1.0)
    node1_id = add_node(lg, NUMBER_GEN_VTABLE, sizeof(NumberGenState), 
                       "number_gen", 0, 1);
    printf("   Node 1 (number gen=1.0): id=%d\n", node1_id);
    assert(node1_id >= 0);
    
    // Node 2: Dual output number generator (generates 2.0 and 3.0)  
    node2_id = add_node(lg, DUAL_OUTPUT_VTABLE, sizeof(DualOutputState),
                       "dual_output", 0, 2);
    printf("   Node 2 (dual output=2.0,3.0): id=%d\n", node2_id);
    assert(node2_id >= 0);
    
    // Node 3: 2-input/1-output multiplier
    node3_id = add_node(lg, MULTIPLIER_VTABLE, sizeof(MultiplierState),
                       "multiplier", 2, 1);
    printf("   Node 3 (2-input multiplier): id=%d\n", node3_id);
    assert(node3_id >= 0);
    
    // Node 4: Gain node (gain = 0.5)
    node4_id = live_add_gain(lg, 0.5f, "gain");
    printf("   Node 4 (gain=0.5): id=%d\n", node4_id);
    assert(node4_id >= 0);
    
    // Apply node creation edits
    apply_graph_edits(lg->graphEditQueue, lg);
    printf("✅ Nodes created and applied\n");
    
    printf("\n🔗 Creating ALL initial connections (exactly as fuzz test)...\n");
    
    // Create all 6 edges exactly as in fuzz test
    printf("   Creating N1→N3 (Node 1 output 0 -> Node 3 input 0)\n");
    assert(graph_connect(lg, node1_id, 0, node3_id, 0));
    
    printf("   Creating N1→N4 (Node 1 output 0 -> Node 4 input 0)\n");
    assert(graph_connect(lg, node1_id, 0, node4_id, 0));
    
    printf("   Creating N2→N3 (Node 2 output 0 -> Node 3 input 1)\n");
    assert(graph_connect(lg, node2_id, 0, node3_id, 1));
    
    printf("   Creating N2→DAC (Node 2 output 1 -> DAC)\n");
    assert(apply_connect(lg, node2_id, 1, lg->dac_node_id, 0));
    
    printf("   Creating N3→DAC (Node 3 output 0 -> DAC)\n");
    assert(apply_connect(lg, node3_id, 0, lg->dac_node_id, 0));
    
    printf("   Creating N4→DAC (Node 4 output 0 -> DAC)\n");
    assert(apply_connect(lg, node4_id, 0, lg->dac_node_id, 0));
    
    apply_graph_edits(lg->graphEditQueue, lg);
    printf("✅ All 6 connections created and applied\n");
    
    // Verify initial state
    float output_buffer[block_size];
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    printf("\n🎵 Initial DAC output (all connected): %.3f\n", output_buffer[0]);
    
    // Now execute the EXACT disconnection sequence that failed:
    // "Initial-N1→N4-N4→DAC-N2→N3-N3→DAC"
    
    printf("\n🔌 Executing exact disconnection sequence that triggers bug...\n");
    
    printf("\nStep 1: Disconnect N1→N4\n");
    assert(graph_disconnect(lg, node1_id, 0, node4_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    printf("   DAC after N1→N4 disconnect: %.6f\n", output_buffer[0]);
    
    printf("\nStep 2: Disconnect N4→DAC\n");
    assert(apply_disconnect(lg, node4_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    memset(output_buffer, 0, sizeof(output_buffer));  
    process_next_block(lg, output_buffer, block_size);
    printf("   DAC after N4→DAC disconnect: %.6f\n", output_buffer[0]);
    
    printf("\nStep 3: Disconnect N2→N3\n");
    assert(graph_disconnect(lg, node2_id, 0, node3_id, 1));
    apply_graph_edits(lg->graphEditQueue, lg);
    memset(output_buffer, 0, sizeof(output_buffer));
    process_next_block(lg, output_buffer, block_size);
    printf("   DAC after N2→N3 disconnect: %.6f\n", output_buffer[0]);
    
    printf("\nStep 4: Disconnect N3→DAC [CRITICAL STEP]\n");
    printf("   This is where the bug likely manifests...\n");
    assert(apply_disconnect(lg, node3_id, 0, lg->dac_node_id, 0));
    apply_graph_edits(lg->graphEditQueue, lg);
    
    printf("\n🎯 FINAL STATE - Bug Check:\n");
    printf("==========================================\n");
    printf("Remaining active edges should be: N1→N3, N2→DAC\n");
    printf("Expected topology:\n");
    printf("  - Node 1 (1.0) → Node 3 → [disconnected from DAC]\n");
    printf("  - Node 2 port 1 (3.0) → DAC\n");
    printf("  - Expected DAC output: 3.0\n\n");
    
    bool bug_found = validate_final_state();
    
    if (bug_found) {
        printf("\n🚨 SUCCESS: Bug reproduced! Ready for lldb debugging.\n");
        printf("   Set breakpoints in apply_disconnect and related functions.\n");
        printf("   Focus on auto-sum cleanup logic when N3→DAC is disconnected.\n");
    } else {
        printf("\n❓ Bug not reproduced in this run.\n");
        printf("   The bug might be non-deterministic or environment-specific.\n");
    }
    
    printf("\n💡 lldb Debugging Suggestions:\n");
    printf("   lldb ./tests/test_exact_bug_reproduction\n");
    printf("   (lldb) break set -n apply_disconnect\n");
    printf("   (lldb) break set -n apply_connect\n");
    printf("   (lldb) break set -n process_next_block\n");
    printf("   (lldb) run\n");
    printf("   Focus on Step 4 where N3→DAC gets disconnected\n");
    
    destroy_live_graph(lg);
    return bug_found ? 0 : 1; // Return 0 if bug found (success for debugging)
}