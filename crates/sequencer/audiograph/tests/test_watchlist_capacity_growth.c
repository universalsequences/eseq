#include "graph_api.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <stdio.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

int main(void) {
  const int initial_capacity = 4;
  const int block_size = 128;
  LiveGraph *lg = create_live_graph(initial_capacity, block_size,
                                    "watchlist_capacity_growth", 1);
  if (!lg) {
    printf("FAILED: create_live_graph returned NULL\n");
    return 1;
  }

  const int total_nodes = 150; // ensure multiple growth operations

  for (int i = 1; i <= total_nodes; i++) {
    float init = (float)i;
    int node_id =
        apply_add_node(lg, NUMBER_VTABLE,
                       NUMBER_MEMORY_SIZE * sizeof(float), (uint64_t)i,
                       "number", 0, 1, &init);
    if (node_id < 0) {
      printf("FAILED: apply_add_node returned %d at iteration %d\n", node_id,
             i);
      destroy_live_graph(lg);
      return 1;
    }

    if (i % 30 == 0) {
      if (!add_node_to_watchlist(lg, node_id)) {
        printf("FAILED: add_node_to_watchlist for node %d\n", node_id);
        destroy_live_graph(lg);
        return 1;
      }
    }
  }

  const int watched_nodes[] = {30, 60, 90, 120, 150};
  const size_t expected_size = NUMBER_MEMORY_SIZE * sizeof(float);

  // Run a few blocks to let watchlist copy state snapshots.
  float tmp_output[block_size];
  for (int i = 0; i < 3; i++) {
    process_next_block(lg, tmp_output, block_size);
  }

  for (size_t i = 0; i < sizeof(watched_nodes) / sizeof(watched_nodes[0]); i++) {
    int node_id = watched_nodes[i];
    size_t snapshot_size = 0;
    float *snapshot = (float *)get_node_state(lg, node_id, &snapshot_size);
    if (!snapshot) {
      printf("FAILED: snapshot missing for node %d\n", node_id);
      destroy_live_graph(lg);
      return 1;
    }
    if (snapshot_size != expected_size) {
      printf("FAILED: snapshot size mismatch for node %d (expected %zu, got %zu)\n",
             node_id, expected_size, snapshot_size);
      free(snapshot);
      destroy_live_graph(lg);
      return 1;
    }
    float expected = (float)node_id;
    if (fabsf(snapshot[0] - expected) > 0.0001f) {
      printf("FAILED: snapshot value mismatch for node %d (expected %.1f, got %.6f)\n",
             node_id, expected, snapshot[0]);
      free(snapshot);
      destroy_live_graph(lg);
      return 1;
    }
    free(snapshot);
  }

  destroy_live_graph(lg);
  printf("SUCCESS: watchlist snapshots survive node capacity growth\n");
  return 0;
}
