#include "../graph_edit.h"
#include "../graph_engine.h"
#include "../graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>

#define BLOCK_SIZE 64

int main(void) {
  LiveGraph *lg = create_live_graph(16, BLOCK_SIZE, "param_update_bounds", 1);
  assert(lg != NULL);

  int gain = live_add_gain(lg, 1.0f, "gain");
  int src = live_add_number(lg, 2.0f, "src");
  assert(gain >= 0 && src >= 0);
  assert(apply_graph_edits(lg->graphEditQueue, lg));
  assert(apply_connect(lg, src, 0, gain, 0));
  assert(apply_connect(lg, gain, 0, lg->dac_node_id, 0));

  ParamMsg oob = {
      .idx = 999999,
      .logical_id = (uint64_t)gain,
      .fvalue = 4.0f,
  };
  assert(params_push(lg->params, oob));

  ParamMsg nonfinite = {
      .idx = 0,
      .logical_id = (uint64_t)gain,
      .fvalue = NAN,
  };
  assert(params_push(lg->params, nonfinite));

  float output[BLOCK_SIZE];
  process_next_block(lg, output, BLOCK_SIZE);

  for (int i = 0; i < BLOCK_SIZE; i++) {
    assert(fabsf(output[i] - 2.0f) < 0.001f);
  }

  ParamMsg valid = {
      .idx = 0,
      .logical_id = (uint64_t)gain,
      .fvalue = 3.0f,
  };
  assert(params_push(lg->params, valid));
  process_next_block(lg, output, BLOCK_SIZE);
  for (int i = 0; i < BLOCK_SIZE; i++) {
    assert(fabsf(output[i] - 6.0f) < 0.001f);
  }

  destroy_live_graph(lg);
  printf("param update bounds test passed\n");
  return 0;
}
