#include "graph_engine.h"
#include "graph_edit.h"
#include "graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>

static void test_sum_high_fanin(void) {
  printf("=== test_sum_high_fanin ===\n");

  const int block_size = 64;
  const int fanin = 64; // Exceeds previous MAX_IO limit

  LiveGraph *lg = create_live_graph(8, block_size, "sum_high_fanin", 1);
  assert(lg != NULL);

  int node_ids[fanin];
  for (int i = 0; i < fanin; i++) {
    int node_id = live_add_number(lg, 1.0f, NULL);
    assert(node_id >= 0);
    node_ids[i] = node_id;
  }
  assert(apply_graph_edits(lg->graphEditQueue, lg));

  for (int i = 0; i < fanin; i++) {
    assert(apply_connect(lg, node_ids[i], 0, lg->dac_node_id, 0));
  }

  int sum_id = lg->nodes[lg->dac_node_id].fanin_sum_node_id[0];
  assert(sum_id >= 0);
  RTNode *sum = &lg->nodes[sum_id];
  assert(sum->nInputs == fanin);

  float output[block_size];
  process_next_block(lg, output, block_size);
  process_next_block(lg, output, block_size);

  const float expected = (float)fanin;
  for (int i = 0; i < block_size; i++) {
    assert(fabsf(output[i] - expected) < 1e-3f);
  }

  destroy_live_graph(lg);
  printf("✓ processed %d-input SUM without crash\n", fanin);
}

int main(void) {
  test_sum_high_fanin();
  return 0;
}
