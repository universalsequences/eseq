#include "../graph_api.h"
#include "../graph_edit.h"
#include "../graph_engine.h"
#include "../graph_nodes.h"
#include <assert.h>
#include <stdio.h>
#include <unistd.h>

#define WAITERS 6
#define FRAMES 64
#define SOURCES 96

static void *wait_for_completion(void *arg) {
  // Deliberately much longer than the test watchdog: completion must wake us.
  rq_wait_for_work((ReadyQ *)arg, 60000000);
  return NULL;
}

static void queue_completion_wakes_all(void) {
  ReadyQ *q = rq_create(16);
  assert(q);
  for (int block = 0; block < 100; block++) {
    pthread_t threads[WAITERS];
    for (int i = 0; i < WAITERS; i++)
      assert(pthread_create(&threads[i], NULL, wait_for_completion, q) == 0);
    while (atomic_load_explicit(&q->waiters, memory_order_seq_cst) < WAITERS)
      cpu_relax();
    // Covers registration-before-park as well as already parked waiters.
    rq_finish(q);
    for (int i = 0; i < WAITERS; i++)
      assert(pthread_join(threads[i], NULL) == 0);
    assert(atomic_load(&q->waiters) == 0);
    // Entering after completion must not consume a credit or park.
    rq_wait_for_work(q, 60000000);
    rq_reset(q);
    assert(atomic_load(&q->qlen) == 0);
    assert(!atomic_load(&q->finished));
  }
  rq_destroy(q);
}

static void queue_publication_wakes_waiter(void) {
  ReadyQ *q = rq_create(16);
  assert(q);
  for (int i = 0; i < 100; i++) {
    pthread_t waiter;
    assert(pthread_create(&waiter, NULL, wait_for_completion, q) == 0);
    // Alternate publication after registration and racing registration. The
    // latter also covers a waiter entering when work is already available.
    if (i % 2 == 0) {
      while (atomic_load_explicit(&q->waiters, memory_order_seq_cst) == 0)
        cpu_relax();
    }
    assert(rq_push(q, i));
    assert(pthread_join(waiter, NULL) == 0);
    int32_t value;
    assert(rq_try_pop(q, &value) && value == i);
    rq_reset(q);
  }
  rq_destroy(q);
}

typedef struct { ReadyQ *queue; } TailState;

static void tail_process(float *const *in, float *const *out, int frames,
                         void *state, void *buffers) {
  (void)buffers;
  TailState *tail = state;
  int active = atomic_load(&g_engine.activeWorkerLimit);
  // All parallel source work has joined here. Hold this node until the idle
  // helpers have entered their queue waits, without depending on a sleep delay.
  // The executor may itself be one of the workers, hence active - 1.
  while (atomic_load(&tail->queue->waiters) < active - 1)
    cpu_relax();
  for (int i = 0; i < frames; i++)
    out[0][i] = in[0][i];
}

static void graph_return_releases_waiters(void) {
  const NodeVTable tail_vtable = {.process = tail_process};
  initialize_engine(FRAMES, 48000);
  const int pools[] = {0, 1, 3, 6};
  for (unsigned p = 0; p < sizeof(pools) / sizeof(pools[0]); p++) {
    engine_start_workers(pools[p]);
    for (int graph = 0; graph < 16; graph++) {
      LiveGraph *lg = create_live_graph(256, FRAMES, "completion", 1);
      assert(lg);
      int tail_id = atomic_fetch_add(&lg->next_node_id, 1);
      int tail = apply_add_node(lg, tail_vtable, sizeof(TailState), tail_id,
                                 "tail", 1, 1, NULL);
      assert(tail >= 0);
      ((TailState *)lg->nodes[tail].state)->queue = lg->sched.readyQueue;
      for (int i = 0; i < SOURCES; i++) {
        int source = live_add_number(lg, 1.0f, "source");
        assert(source >= 0);
        assert(apply_connect_internal(lg, source, 0, tail, 0));
      }
      assert(apply_connect_internal(lg, tail, 0, lg->dac_node_id, 0));
      update_orphaned_status(lg);
      for (int block = 0; block < 16; block++) {
        float output[FRAMES];
        process_next_block(lg, output, FRAMES);
        for (int i = 0; i < FRAMES; i++)
          assert(output[i] == (float)SOURCES);
        assert(atomic_load(&lg->sched.jobsInFlight) == 0);
        assert(atomic_load(&lg->sched.readyQueue->waiters) == 0);
        assert(atomic_load(&g_engine.workSession) == NULL);
      }
      // Destroy/recreate while the pool remains alive, including immediate
      // reuse of graph allocations and many blocks on the same graph pointer.
      destroy_live_graph(lg);
    }
    engine_clear_os_workgroup();
    engine_stop_workers();
  }
}

int main(void) {
  alarm(15);
  queue_completion_wakes_all();
  queue_publication_wakes_waiter();
  graph_return_releases_waiters();
  alarm(0);
  puts("scheduler completion and lifetime tests passed");
  return 0;
}
