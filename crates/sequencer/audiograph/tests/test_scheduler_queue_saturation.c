#include "../graph_edit.h"
#include "../graph_engine.h"
#include "../graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#define GROUPS 5
#define SOURCES_PER_GROUP 840
#define SOURCE_COUNT (GROUPS * SOURCES_PER_GROUP)
#define BRANCH_COUNT 4200
#define BLOCK_SIZE 64

static void timeout_handler(int sig) {
  (void)sig;
  fprintf(stderr, "scheduler queue saturation test timed out\n");
  exit(2);
}

static void test_source_seed_saturation(void) {
  LiveGraph *lg = create_live_graph(SOURCE_COUNT + 32, BLOCK_SIZE,
                                    "scheduler_source_seed_saturation", 1);
  assert(lg != NULL);

  int groups[GROUPS];
  for (int g = 0; g < GROUPS; g++) {
    groups[g] = live_add_gain(lg, 1.0f, "group");
    assert(groups[g] >= 0);
  }

  int master = live_add_gain(lg, 1.0f, "master");
  assert(master >= 0);

  for (int g = 0; g < GROUPS; g++) {
    for (int i = 0; i < SOURCES_PER_GROUP; i++) {
      int src = live_add_number(lg, 1.0f, "src");
      assert(src >= 0);
      assert(apply_connect_internal(lg, src, 0, groups[g], 0));
    }
    assert(apply_connect_internal(lg, groups[g], 0, master, 0));
  }
  assert(apply_connect_internal(lg, master, 0, lg->dac_node_id, 0));
  update_orphaned_status(lg);

  float output[BLOCK_SIZE];
  process_next_block(lg, output, BLOCK_SIZE);

  printf("seed source_count=%d output=%.1f\n", lg->sched.source_count,
         output[0]);
  assert(lg->sched.source_count == SOURCE_COUNT);
  assert(fabsf(output[0] - (float)SOURCE_COUNT) < 0.01f);

  destroy_live_graph(lg);
}

static void test_mid_block_fanout_saturation(void) {
  LiveGraph *lg = create_live_graph(BRANCH_COUNT + 32, BLOCK_SIZE,
                                    "scheduler_fanout_saturation", 1);
  assert(lg != NULL);

  int src = live_add_number(lg, 1.0f, "src");
  int master = live_add_gain(lg, 1.0f, "master");
  assert(src >= 0);
  assert(master >= 0);

  int groups[GROUPS];
  for (int g = 0; g < GROUPS; g++) {
    groups[g] = live_add_gain(lg, 1.0f, "group");
    assert(groups[g] >= 0);
  }

  for (int g = 0; g < GROUPS; g++) {
    for (int i = 0; i < SOURCES_PER_GROUP; i++) {
      int branch = live_add_gain(lg, 1.0f, "branch");
      assert(branch >= 0);
      assert(apply_connect_internal(lg, src, 0, branch, 0));
      assert(apply_connect_internal(lg, branch, 0, groups[g], 0));
    }
    assert(apply_connect_internal(lg, groups[g], 0, master, 0));
  }
  assert(apply_connect_internal(lg, master, 0, lg->dac_node_id, 0));
  update_orphaned_status(lg);

  float output[BLOCK_SIZE];
  process_next_block(lg, output, BLOCK_SIZE);

  printf("fanout source_count=%d output=%.1f\n", lg->sched.source_count,
         output[0]);
  assert(lg->sched.source_count == 1);
  assert(fabsf(output[0] - (float)BRANCH_COUNT) < 0.01f);

  destroy_live_graph(lg);
}

int main(void) {
  signal(SIGALRM, timeout_handler);
  alarm(5);

  initialize_engine(BLOCK_SIZE, 48000);
  engine_start_workers(3);

  test_source_seed_saturation();
  test_mid_block_fanout_saturation();

  engine_stop_workers();
  alarm(0);
  printf("scheduler queue saturation test passed\n");
  return 0;
}
