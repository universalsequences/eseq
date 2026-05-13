#include "../graph_api.h"
#include "../graph_edit.h"
#include "../graph_engine.h"
#include "../graph_nodes.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

int main(void) {
  LiveGraph *lg = create_live_graph(64, 128, "custom_engine_growth", 1);
  if (!lg) {
    fprintf(stderr, "failed to create graph\n");
    return 1;
  }

  for (int i = 0; i < 61; i++) {
    if (live_add_gain(lg, 1.0f, "filler") < 0) {
      fprintf(stderr, "failed to queue filler node %d\n", i);
      destroy_live_graph(lg);
      return 1;
    }
  }
  if (!apply_graph_edits(lg->graphEditQueue, lg)) {
    fprintf(stderr, "failed to apply filler nodes\n");
    destroy_live_graph(lg);
    return 1;
  }

  int node_ids[12];
  for (int i = 0; i < 12; i++) {
    float init = 0.25f;
    node_ids[i] = add_node(lg, NUMBER_VTABLE, NUMBER_MEMORY_SIZE * sizeof(float),
                           "engine_node", 0, 1, &init, sizeof(init));
    if (node_ids[i] < 0) {
      fprintf(stderr, "failed to queue engine node %d\n", i);
      destroy_live_graph(lg);
      return 1;
    }
    if (!graph_connect(lg, node_ids[i], 0, lg->dac_node_id, 0)) {
      fprintf(stderr, "failed to queue connect for engine node %d\n", i);
      destroy_live_graph(lg);
      return 1;
    }
  }

  if (!apply_graph_edits(lg->graphEditQueue, lg)) {
    fprintf(stderr, "failed to apply engine nodes\n");
    destroy_live_graph(lg);
    return 1;
  }

  float out[128];
  memset(out, 0, sizeof(out));
  process_next_block(lg, out, 128);

  if (fabsf(out[0] - 3.0f) > 0.001f) {
    fprintf(stderr, "unexpected output after growth: got %.3f expected 3.000\n",
            out[0]);
    destroy_live_graph(lg);
    return 1;
  }

  if (lg->node_capacity < 128) {
    fprintf(stderr, "node capacity did not grow as expected: %d\n",
            lg->node_capacity);
    destroy_live_graph(lg);
    return 1;
  }

  destroy_live_graph(lg);
  return 0;
}
