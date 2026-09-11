#include "graph_types.h"
#include <assert.h>
#include <errno.h>

#ifdef AUDIOGRAPH_EXPERIMENTS
// Immutable after experiment initialization, before any worker starts.
int audiograph_experiment_queue_hint = 0;
#endif

// Allows worker threads to "sleep" and be "awaken" when a new block is needed
// This lowers CPU utilization as we don't waste spins when theres no work to do
// ===================== ReadyQ Implementation =====================

ReadyQ *rq_create(int capacity) {
  ReadyQ *q = (ReadyQ *)malloc(sizeof(ReadyQ));
  if (!q)
    return NULL;

  // Create underlying MPMC queue
  q->ring = mpmc_create(capacity);
  if (!q->ring) {
    free(q);
    return NULL;
  }

  // Initialize logical length counter
  atomic_store_explicit(&q->qlen, 0, memory_order_relaxed);
  // Initialize waiter count
  atomic_store_explicit(&q->waiters, 0, memory_order_relaxed);
  atomic_init(&q->finished, false);

  // Initialize semaphore (starts at 0 - no items)
#ifdef __APPLE__
  q->items = dispatch_semaphore_create(0);
  if (!q->items) {
    mpmc_destroy(q->ring);
    free(q);
    return NULL;
  }
#else
  if (sem_init(&q->items, 0, 0) != 0) {
    mpmc_destroy(q->ring);
    free(q);
    return NULL;
  }
#endif

  return q;
}

void rq_destroy(ReadyQ *q) {
  if (!q)
    return;
  assert(atomic_load_explicit(&q->waiters, memory_order_relaxed) == 0);

#ifdef __APPLE__
  if (q->items) {
    dispatch_release(q->items);
  }
#else
  sem_destroy(&q->items);
#endif

  mpmc_destroy(q->ring);
  free(q);
}

static void rq_signal(ReadyQ *q) {
#ifdef __APPLE__
  dispatch_semaphore_signal(q->items);
#else
  sem_post(&q->items);
#endif
}

bool rq_push(ReadyQ *q, int32_t nid) {
  if (!q)
    return false;

  // First, try to enqueue the item
  if (!mpmc_push(q->ring, nid)) {
    return false; // Queue is full
  }

  // Item was successfully enqueued, now update length and signal if needed
  // Publication and waiter registration share a sequentially consistent order:
  // either a waiter sees the work, or the publisher sees the registered waiter.
  atomic_fetch_add_explicit(&q->qlen, 1, memory_order_seq_cst);

  // A future waiter will see qlen; it does not need a saved notification.
  // Avoid accumulating credits while all executors are busy doing DSP.
  if (atomic_load_explicit(&q->waiters, memory_order_seq_cst) > 0)
    rq_signal(q);

  return true;
}

bool rq_try_pop(ReadyQ *q, int32_t *out) {
  if (!q || !out)
    return false;

#ifdef AUDIOGRAPH_EXPERIMENTS
  // Advisory only. A concurrent publisher may not yet have incremented qlen;
  // callers retry or use the existing timed wait in that case.
  if (audiograph_experiment_queue_hint &&
      atomic_load_explicit(&q->qlen, memory_order_acquire) <= 0)
    return false;
#endif

  // Try to dequeue an item (non-blocking)
  if (!mpmc_pop(q->ring, out)) {
    return false; // Queue is empty
  }

  // Item was successfully dequeued, decrement length
  // Use acq_rel to ensure dequeue happens before length decrement
  atomic_fetch_sub_explicit(&q->qlen, 1, memory_order_acq_rel);

  return true;
}

void rq_finish(ReadyQ *q) {
  if (atomic_exchange_explicit(&q->finished, true, memory_order_seq_cst))
    return;
  int waiters = atomic_load_explicit(&q->waiters, memory_order_seq_cst);
  for (int i = 0; i < waiters; i++)
    rq_signal(q);
}

void rq_wait_for_work(ReadyQ *q, int timeout_us) {
  if (!q)
    return;

  // Register BEFORE checking both predicates. A wake published between the
  // checks and the kernel wait leaves a semaphore credit for this waiter.
  atomic_fetch_add_explicit(&q->waiters, 1, memory_order_seq_cst);
  if (atomic_load_explicit(&q->finished, memory_order_seq_cst) ||
      atomic_load_explicit(&q->qlen, memory_order_seq_cst) > 0) {
    atomic_fetch_sub_explicit(&q->waiters, 1, memory_order_seq_cst);
    return;
  }

#ifdef __APPLE__
  // macOS does not support unnamed POSIX semaphores.
  dispatch_time_t timeout =
      dispatch_time(DISPATCH_TIME_NOW,
                    (int64_t)timeout_us * 1000L); // Convert us to ns
  (void)dispatch_semaphore_wait(q->items, timeout);
#else
  // Linux: Use sem_timedwait
  struct timespec ts;
  clock_gettime(CLOCK_REALTIME, &ts);

  // Add timeout_us microseconds
  long nsec = ts.tv_nsec + (timeout_us * 1000L);
  ts.tv_sec += nsec / 1000000000L;
  ts.tv_nsec = nsec % 1000000000L;

  (void)sem_timedwait(&q->items, &ts);
#endif
  atomic_fetch_sub_explicit(&q->waiters, 1, memory_order_seq_cst);
}

void rq_reset(ReadyQ *q) {
  if (!q)
    return;
  assert(atomic_load_explicit(&q->waiters, memory_order_relaxed) == 0);

  // Drain any remaining items from the underlying MPMC queue
  int32_t dummy;
  while (rq_try_pop(q, &dummy)) {
    // Discard stale items
  }

  // Reset logical length counter
  atomic_store_explicit(&q->qlen, 0, memory_order_relaxed);
  atomic_store_explicit(&q->finished, false, memory_order_relaxed);

  // Only drain with exclusive ownership. Previous-block waiters must fully
  // leave before their notification state can be recycled for the next block.
#ifdef __APPLE__
  // For dispatch_semaphore, we need to consume any pending signals
  // Use a timeout of 0 to make it non-blocking
  while (dispatch_semaphore_wait(q->items, DISPATCH_TIME_NOW) == 0) {
    // Consumed one pending signal
  }
#else
  // For POSIX semaphores, drain using sem_trywait
  while (sem_trywait(&q->items) == 0) {
    // Consumed one pending signal
  }
#endif
}

void rq_push_or_spin(ReadyQ *q, int32_t nid) {
  if (!q)
    return;

  // CRITICAL FIX: Spin until enqueue succeeds to prevent dropped work
  // This was the main cause of audio artifacts - lost nodes = stale buffers
  for (;;) {
    if (rq_push(q, nid))
      break;
    cpu_relax(); // Brief pause to reduce contention
  }
}

// Publish the batch before waking up to one waiter per item.
void rq_push_batch(ReadyQ *q, const int32_t *nids, int count) {
  if (!q || !nids || count <= 0)
    return;

  int pushed = 0;
  for (int i = 0; i < count; i++) {
    // Push directly to MPMC without signaling
    for (;;) {
      if (mpmc_push(q->ring, nids[i])) {
        atomic_fetch_add_explicit(&q->qlen, 1, memory_order_seq_cst);
        pushed++;
        break;
      }
      cpu_relax();
    }
  }

  if (pushed > 0) {
    int waiters = atomic_load_explicit(&q->waiters, memory_order_seq_cst);
    int signals = waiters < pushed ? waiters : pushed;
    for (int i = 0; i < signals; i++)
      rq_signal(q);
  }
}
