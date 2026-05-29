#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_types.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

// Node whose process() outputs state[0], so we can observe a write end-to-end.
#define STATE_SLOTS 16
static void state_output_process(float *const *in, float *const *out, int n,
                                 void *memory, void *buffers) {
  (void)in;
  (void)buffers;
  float *mem = (float *)memory;
  float v = mem[0];
  for (int i = 0; i < n; i++)
    out[0][i] = v;
}

static const NodeVTable STATE_VTABLE = {
    .process = state_output_process, .init = NULL, .reset = NULL, .migrate = NULL};

void test_write_node_state_basic() {
  printf("=== Testing write_node_state (basic) ===\n");
  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "wns_test", 1);
  assert(lg != NULL);

  int nid = add_node(lg, STATE_VTABLE, STATE_SLOTS * sizeof(float), "state", 0, 1,
                     NULL, 0);
  assert(nid >= 0);
  assert(apply_graph_edits(lg->graphEditQueue, lg));

  // Queue a bulk write into the middle of the node's state.
  float payload[4] = {10.0f, 20.0f, 30.0f, 40.0f};
  bool queued = write_node_state(lg, nid, 4, payload, 4);
  assert(queued);
  // Not applied yet.
  float *state = (float *)lg->nodes[nid].state;
  assert(state[4] == 0.0f);

  assert(apply_graph_edits(lg->graphEditQueue, lg));
  assert(state[4] == 10.0f && state[5] == 20.0f && state[6] == 30.0f &&
         state[7] == 40.0f);
  // Neighbors untouched.
  assert(state[3] == 0.0f && state[8] == 0.0f);
  printf("✓ Bulk write landed at the correct offset, neighbors intact\n");

  destroy_live_graph(lg);
}

void test_write_node_state_end_to_end() {
  printf("=== Testing write_node_state (end-to-end through process) ===\n");
  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "wns_e2e", 1);
  assert(lg != NULL);

  int nid = add_node(lg, STATE_VTABLE, STATE_SLOTS * sizeof(float), "state", 0, 1,
                     NULL, 0);
  assert(nid >= 0);
  assert(apply_graph_edits(lg->graphEditQueue, lg));
  assert(apply_connect(lg, nid, 0, lg->dac_node_id, 0));

  // Write state[0] = 7.0 and let process_next_block drain + apply the edit.
  float v = 7.0f;
  assert(write_node_state(lg, nid, 0, &v, 1));

  float out[64];
  memset(out, 0, sizeof(out));
  process_next_block(lg, out, block_size);
  for (int i = 0; i < block_size; i++) {
    assert(fabsf(out[i] - 7.0f) < 0.001f);
  }
  printf("✓ Queued write observed by the audio thread after process\n");

  destroy_live_graph(lg);
}

void test_write_node_state_bounds() {
  printf("=== Testing write_node_state (out-of-bounds rejected) ===\n");
  const int block_size = 64;
  LiveGraph *lg = create_live_graph(16, block_size, "wns_oob", 1);
  assert(lg != NULL);

  int nid = add_node(lg, STATE_VTABLE, STATE_SLOTS * sizeof(float), "state", 0, 1,
                     NULL, 0);
  assert(nid >= 0);
  assert(apply_graph_edits(lg->graphEditQueue, lg));

  float *state = (float *)lg->nodes[nid].state;
  state[STATE_SLOTS - 1] = 123.0f; // canary

  // Write that would overrun the state allocation.
  float big[8] = {1, 2, 3, 4, 5, 6, 7, 8};
  assert(write_node_state(lg, nid, STATE_SLOTS - 2, big, 8));
  // apply_graph_edits returns false because the apply was rejected,
  // but it must not corrupt memory.
  bool ok = apply_graph_edits(lg->graphEditQueue, lg);
  assert(!ok);
  assert(state[STATE_SLOTS - 1] == 123.0f); // canary intact
  printf("✓ Out-of-bounds write rejected without corruption\n");

  destroy_live_graph(lg);
}

int main() {
  test_write_node_state_basic();
  test_write_node_state_end_to_end();
  test_write_node_state_bounds();
  printf("\n✅ All write_node_state tests passed!\n");
  return 0;
}
