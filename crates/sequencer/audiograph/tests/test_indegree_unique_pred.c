#include "graph_edit.h"
#include "graph_engine.h"
#include "graph_nodes.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

// Custom nodes: 0-in/2-out source (A=1.0, B=2.0), 1x1 pass, 2x2 dup
static const float A = 1.0f, B = 2.0f;

static void src2(float *const *in, float *const *out, int n, void *m, void *b) {
  (void)in;
  (void)m;
  for (int i = 0; i < n; i++) {
    out[0][i] = A;
    out[1][i] = B;
  }
}
static const NodeVTable SRC2 = {.process = src2};

static void passt(float *const *in, float *const *out, int n, void *m,
                  void *b) {
  (void)m;
  if (in[0])
    memcpy(out[0], in[0], (size_t)n * sizeof(float));
  else
    memset(out[0], 0, (size_t)n * sizeof(float));
}
static const NodeVTable PASS = {.process = passt};

static void dup22(float *const *in, float *const *out, int n, void *m,
                  void *b) {
  (void)m;
  if (in[0])
    memcpy(out[0], in[0], (size_t)n * sizeof(float));
  else
    memset(out[0], 0, (size_t)n * sizeof(float));
  if (in[1])
    memcpy(out[1], in[1], (size_t)n * sizeof(float));
  else
    memset(out[1], 0, (size_t)n * sizeof(float));
}
static const NodeVTable DUP22 = {.process = dup22};

static int count_unique_preds(LiveGraph *lg, int dst) {
  if (dst < 0 || dst >= lg->node_count)
    return -1;
  RTNode *D = &lg->nodes[dst];
  int uniq = 0;
  for (int s = 0; s < lg->node_count; s++) {
    int uses = 0;
    for (int di = 0; di < D->nInputs; di++) {
      int eid = D->inEdgeId ? D->inEdgeId[di] : -1;
      if (eid >= 0 && eid < lg->edge_capacity && lg->edges[eid].in_use &&
          lg->edges[eid].src_node == s) {
        uses = 1;
        break;
      }
    }
    uniq += uses;
  }
  return uniq;
}

int main() {
  initialize_engine(128, 48000);
  LiveGraph *lg = create_live_graph(16, 128, "indegree_unique_pred", 2);
  assert(lg);
  int dac = lg->dac_node_id;

  int src = add_node(lg, SRC2, 0, "src2", 0, 2, NULL, 0);
  int pass = add_node(lg, PASS, 0, "pass", 1, 1, NULL, 0);
  int dup = add_node(lg, DUP22, 0, "dup22", 2, 2, NULL, 0);
  assert(src >= 0 && pass >= 0 && dup >= 0);
  apply_graph_edits(lg->graphEditQueue, lg);

  // Wiring: src:0 -> pass -> DAC[0]; src:0 -> dup:0; src:1 -> dup:1;
  // dup:0->DAC[0], dup:1->DAC[1]
  assert(apply_connect(lg, src, 0, pass, 0));
  assert(apply_connect(lg, pass, 0, dac, 0));
  assert(apply_connect(lg, src, 0, dup, 0));
  assert(apply_connect(lg, src, 1, dup, 1));
  assert(apply_connect(lg, dup, 0, dac, 0));
  assert(apply_connect(lg, dup, 1, dac, 1));

  int uniq_before = count_unique_preds(lg, dac);
  printf("Unique preds before disconnect = %d, indegree=%d\n", uniq_before,
         lg->sched.indegree[dac]);
  assert(uniq_before == lg->sched.indegree[dac]);

  // Disconnect pass -> DAC[0], expect DAC unique preds becomes 1
  assert(apply_disconnect(lg, pass, 0, dac, 0));
  int uniq_after = count_unique_preds(lg, dac);
  printf("Unique preds after disconnect = %d, indegree=%d\n", uniq_after,
         lg->sched.indegree[dac]);
  assert(uniq_after == lg->sched.indegree[dac]);

  // Process a block to ensure no scheduling deadlock (single-thread run)
  float out[128 * 2];
  process_next_block(lg, out, 128);

  destroy_live_graph(lg);
  printf("✓ Indegree matches unique predecessors before/after disconnect\n");
  return 0;
}
