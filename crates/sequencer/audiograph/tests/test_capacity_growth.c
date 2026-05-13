#include "graph_engine.h"
#include "graph_edit.h"
#include "graph_nodes.h"
#include <stdio.h>

int main() {
  printf("=== Capacity Growth Test ===\n");
  
  // Create a small graph that will need to grow
  LiveGraph *lg = create_live_graph(4, 128, "growth_test", 1);
  printf("Initial capacity: %d nodes\n", lg->node_capacity);
  
  // Add 3 nodes within capacity first (DAC is at index 0, so we have room for indexes 1,2,3)
  printf("Adding 3 nodes (within initial capacity of 4, since DAC is at index 0)...\n");
  
  int node_ids[6];
  for (int i = 0; i < 3; i++) {
    node_ids[i] = live_add_gain(lg, 0.5f, "test_gain");
    printf("  Node %d: ID = %d\n", i, node_ids[i]);
    if (node_ids[i] < 0) {
      printf("ERROR: Failed to create node %d\n", i);
      return 1;
    }
  }
  
  // Process the first 3 nodes (no capacity growth needed)
  apply_graph_edits(lg->graphEditQueue, lg);
  
  
  // Now add the nodes that will trigger capacity growth
  printf("\nAdding 3 more nodes (will exceed capacity and trigger growth)...\n");
  for (int i = 3; i < 6; i++) {
    node_ids[i] = live_add_gain(lg, 0.5f, "test_gain");
    printf("  Node %d: ID = %d\n", i, node_ids[i]);
    if (node_ids[i] < 0) {
      printf("ERROR: Failed to create node %d\n", i);
      return 1;
    }
  }
  
  // Process the growth-triggering operations
  apply_graph_edits(lg->graphEditQueue, lg);
  
  printf("Final capacity: %d nodes\n", lg->node_capacity);
  printf("Final node count: %d nodes\n", lg->node_count);
  
  if (lg->node_capacity > 4) {
    printf("✓ Capacity growth worked!\n");
  } else {
    printf("✗ Capacity growth failed!\n");
    return 1;
  }
  
  // Systematically verify all LiveGraph fields after growth
  printf("\n=== Post-Growth Field Verification ===\n");
  printf("nodes pointer: %p\n", (void*)lg->nodes);
  printf("pending pointer: %p\n", (void*)lg->sched.pending);
  printf("indegree pointer: %p\n", (void*)lg->sched.indegree);
  printf("is_orphaned pointer: %p\n", (void*)lg->sched.is_orphaned);
  printf("edges pointer: %p\n", (void*)lg->edges);
  printf("silence_buf pointer: %p\n", (void*)lg->silence_buf);
  printf("scratch_null pointer: %p\n", (void*)lg->scratch_null);
  printf("readyQueue pointer: %p\n", (void*)lg->sched.readyQueue);
  printf("params pointer: %p\n", (void*)lg->params);
  printf("graphEditQueue pointer: %p\n", (void*)lg->graphEditQueue);
  printf("failed_ids pointer: %p\n", (void*)lg->failed_ids);
  
  // Check if any pointers look suspicious (very low values indicate corruption)
  bool corruption_detected = false;
  if ((uintptr_t)lg->nodes < 1000) { printf("WARNING: nodes pointer looks corrupted!\n"); corruption_detected = true; }
  if ((uintptr_t)lg->sched.pending < 1000) { printf("WARNING: pending pointer looks corrupted!\n"); corruption_detected = true; }
  if ((uintptr_t)lg->sched.indegree < 1000) { printf("WARNING: indegree pointer looks corrupted!\n"); corruption_detected = true; }
  if ((uintptr_t)lg->sched.is_orphaned < 1000) { printf("WARNING: is_orphaned pointer looks corrupted!\n"); corruption_detected = true; }
  
  if (!corruption_detected) {
    printf("✓ All major pointers look valid\n");
  }
  
  // Test teardown
  printf("Testing destroy_live_graph...\n");
  destroy_live_graph(lg);
  printf("✓ Graph destroyed successfully\n");
  
  return 0;
}