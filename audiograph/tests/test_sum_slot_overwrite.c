#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <stdio.h>

/*
 * Regression test for: SUM node overwritten by queued GE_ADD_NODE.
 *
 * Bug scenario:
 *   1. Create a graph with capacity C, fill slots 0..(C-2) immediately.
 *   2. Queue add_node(), which pre-allocates ID (C-1) but does not populate
 *      the slot yet.
 *   3. Immediately create another node via live_add_*(), forcing growth and
 *      moving node_count past the queued slot.
 *   4. Immediately apply_connect() a second source to an already-fed port,
 *      forcing a hidden SUM node.
 *   5. Drain the queued GE_ADD_NODE.
 *
 * Expected: hidden SUM allocation must use a fresh atomic node ID, not scan for
 * an apparently-empty slot that has already been reserved by queued add_node().
 */
int main(void) {
  printf("=== SUM Slot Overwrite Regression Test ===\n");

  LiveGraph *lg = create_live_graph(5, 128, "sum_overwrite_test", 1);
  assert(lg != NULL);
  int dac = lg->dac_node_id;
  printf("  DAC at slot %d, node_count=%d, capacity=%d\n", dac, lg->node_count,
         lg->node_capacity);

  int src1 = live_add_oscillator(lg, 440.0f, "src1");
  int bus = live_add_gain(lg, 1.0f, "bus");
  int src2 = live_add_oscillator(lg, 880.0f, "src2");

  printf("  src1=%d, bus=%d, src2=%d  (node_count=%d)\n", src1, bus, src2,
         lg->node_count);
  assert(src1 == 1 && bus == 2 && src2 == 3);
  assert(lg->node_count == 4);

  bool c1 = apply_connect(lg, src1, 0, bus, 0);
  assert(c1);

  int queued_id =
      add_node(lg, GAIN_VTABLE, GAIN_MEMORY_SIZE * sizeof(float),
               "queued_gain", 1, 1, NULL, 0);
  printf("  Queued add_node -> pre-allocated ID %d\n", queued_id);
  assert(queued_id == 4);

  assert(lg->nodes[queued_id].vtable.process == NULL);
  assert(lg->nodes[queued_id].nInputs == 0);
  assert(lg->nodes[queued_id].nOutputs == 0);

  int extra = live_add_oscillator(lg, 220.0f, "extra");
  printf("  Immediate live_add -> slot %d  (node_count=%d, capacity=%d)\n",
         extra, lg->node_count, lg->node_capacity);
  assert(extra == 5);
  assert(lg->node_count >= 6);
  assert(lg->nodes[queued_id].vtable.process == NULL);

  bool c2 = apply_connect(lg, src2, 0, bus, 0);
  assert(c2);

  RTNode *bus_node = &lg->nodes[bus];
  int sum_id = bus_node->fanin_sum_node_id[0];
  printf("  bus->fanin_sum_node_id[0] = %d\n", sum_id);
  assert(sum_id >= 0);

  RTNode *sum_node = &lg->nodes[sum_id];
  assert(sum_node->vtable.process != NULL);

  bool drain_ok = apply_graph_edits(lg->graphEditQueue, lg);
  assert(drain_ok);

  bus_node = &lg->nodes[bus];
  sum_id = bus_node->fanin_sum_node_id[0];
  sum_node = &lg->nodes[sum_id];

  assert(sum_node->vtable.process != NULL &&
         "SUM node was destroyed by queued GE_ADD_NODE");
  assert(sum_node->nInputs >= 2 && "SUM node lost its input ports");

  RTNode *gain_node = &lg->nodes[queued_id];
  assert(gain_node->vtable.process != NULL &&
         "Queued gain node should exist after drain");

  destroy_live_graph(lg);
  printf("=== SUM Slot Overwrite Test PASSED ===\n");
  return 0;
}
