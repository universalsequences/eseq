#include "graph_api.h"
#include "graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

static void expect_watch_snapshot(LiveGraph *lg, int node_id, float expected) {
  float block[128];
  for (int i = 0; i < 4; i++) {
    process_next_block(lg, block, 128);
  }

  size_t snapshot_size = 0;
  float *snapshot = (float *)get_node_state(lg, node_id, &snapshot_size);
  assert(snapshot && "watchlist snapshot missing");
  assert(snapshot_size == NUMBER_MEMORY_SIZE * sizeof(float));
  assert(fabsf(snapshot[0] - expected) < 1e-6f);
  free(snapshot);
}

int main(void) {
  const int initial_capacity = 8;
  LiveGraph *lg =
      create_live_graph(initial_capacity, 128, "watchlist_initial_cap", 1);
  assert(lg);

  // Seed nodes up to the initial capacity and track their snapshots.
  for (int i = 1; i <= 7; ++i) {
    float init = (float)i;
    int node_id = apply_add_node(
        lg, NUMBER_VTABLE, NUMBER_MEMORY_SIZE * sizeof(float), (uint64_t)i,
        "number", 0, 1, &init);
    assert(node_id == i);
    assert(add_node_to_watchlist(lg, node_id));
    expect_watch_snapshot(lg, node_id, init);
  }

  // This node triggers the first capacity growth (from 8 → 16).
  float init8 = 8.0f;
  int node8 = apply_add_node(lg, NUMBER_VTABLE,
                             NUMBER_MEMORY_SIZE * sizeof(float), 8, "number", 0,
                             1, &init8);
  assert(node8 == 8);
  assert(add_node_to_watchlist(lg, node8));
  expect_watch_snapshot(lg, node8, init8);

  // Verify previously watched nodes still report correct snapshots post-growth.
  expect_watch_snapshot(lg, 3, 3.0f);
  expect_watch_snapshot(lg, 7, 7.0f);

  destroy_live_graph(lg);
  printf("SUCCESS: watchlist survives initial-capacity growth edge case\n");
  return 0;
}
