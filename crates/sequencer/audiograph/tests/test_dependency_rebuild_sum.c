#include "../graph_edit.h"
#include "../graph_engine.h"
#include "../graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#define BLOCK_SIZE 64

static bool has_successor_id(const RTNode *node, int succ_id) {
  for (int i = 0; i < node->succCount; i++) {
    if (node->succ[i] == succ_id) {
      return true;
    }
  }
  return false;
}

static void corrupt_successors(RTNode *node) {
  free(node->succ);
  node->succ = NULL;
  node->succCount = 0;
}

int main(void) {
  LiveGraph *lg = create_live_graph(32, BLOCK_SIZE, "dependency_rebuild_sum", 1);
  assert(lg != NULL);

  int src_a = live_add_number(lg, 1.0f, "src_a");
  int src_b = live_add_number(lg, 2.0f, "src_b");
  int src_c = live_add_number(lg, 4.0f, "src_c");
  int master = live_add_gain(lg, 1.0f, "master");
  assert(src_a >= 0 && src_b >= 0 && src_c >= 0 && master >= 0);

  assert(apply_graph_edits(lg->graphEditQueue, lg));
  assert(apply_connect_internal(lg, src_a, 0, master, 0));
  assert(apply_connect_internal(lg, src_b, 0, master, 0));
  assert(apply_connect_internal(lg, src_c, 0, master, 0));
  assert(apply_connect_internal(lg, master, 0, lg->dac_node_id, 0));
  update_orphaned_status(lg);

  int sum_id = lg->nodes[master].fanin_sum_node_id[0];
  assert(sum_id >= 0);
  assert(lg->sched.indegree[sum_id] == 3);
  assert(has_successor_id(&lg->nodes[src_a], sum_id));
  assert(has_successor_id(&lg->nodes[src_b], sum_id));
  assert(has_successor_id(&lg->nodes[src_c], sum_id));

  // Simulate the stale dependency state seen in the live failure: the edge
  // graph is still correct, but scheduler predecessor/successor metadata has
  // drifted. A topology refresh must repair this from node input edges.
  corrupt_successors(&lg->nodes[src_a]);
  corrupt_successors(&lg->nodes[src_b]);
  corrupt_successors(&lg->nodes[src_c]);
  lg->sched.indegree[sum_id] = 0;

  update_orphaned_status(lg);

  assert(lg->sched.indegree[sum_id] == 3);
  assert(has_successor_id(&lg->nodes[src_a], sum_id));
  assert(has_successor_id(&lg->nodes[src_b], sum_id));
  assert(has_successor_id(&lg->nodes[src_c], sum_id));

  float output[BLOCK_SIZE];
  process_next_block(lg, output, BLOCK_SIZE);
  assert(fabsf(output[0] - 7.0f) < 0.001f);

  destroy_live_graph(lg);
  printf("dependency rebuild SUM test passed\n");
  return 0;
}
