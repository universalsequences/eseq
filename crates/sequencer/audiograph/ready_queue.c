#include "graph_types.h"
#include <errno.h>

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

bool rq_push(ReadyQ *q, int32_t nid) {
  if (!q)
    return false;

  // First, try to enqueue the item
  if (!mpmc_push(q->ring, nid)) {
    return false; // Queue is full
  }

  // Item was successfully enqueued, now update length and signal if needed
  // Use acq_rel to ensure the enqueue is visible before length increment
  int old_len = atomic_fetch_add_explicit(&q->qlen, 1, memory_order_acq_rel);

  // Wake strategy:
  // - Always signal on 0→1 transition (classic behavior)
  // - Additionally, if there are waiters, signal to wake more workers
  if (old_len == 0 || atomic_load_explicit(&q->waiters, memory_order_acquire) > 0) {
#ifdef __APPLE__
    dispatch_semaphore_signal(q->items);
#else
    sem_post(&q->items);
#endif
  }

  return true;
}

bool rq_try_pop(ReadyQ *q, int32_t *out) {
  if (!q || !out)
    return false;

  // Try to dequeue an item (non-blocking)
  if (!mpmc_pop(q->ring, out)) {
    return false; // Queue is empty
  }

  // Item was successfully dequeued, decrement length
  // Use acq_rel to ensure dequeue happens before length decrement
  atomic_fetch_sub_explicit(&q->qlen, 1, memory_order_acq_rel);

  return true;
}

bool rq_wait_nonempty(ReadyQ *q, int timeout_us) {
  if (!q)
    return false;

  // Quick check - if queue has items, return immediately
  if (atomic_load_explicit(&q->qlen, memory_order_acquire) > 0) {
    return true;
  }

#ifdef __APPLE__
  // macOS: Use dispatch_semaphore with timeout
  // Track waiters to enable push-side aggressive wakeups when needed
  atomic_fetch_add_explicit(&q->waiters, 1, memory_order_acq_rel);
  dispatch_time_t timeout =
      dispatch_time(DISPATCH_TIME_NOW,
                    (int64_t)timeout_us * 1000L); // Convert us to ns
  int rc = dispatch_semaphore_wait(q->items, timeout) == 0;
  atomic_fetch_sub_explicit(&q->waiters, 1, memory_order_acq_rel);
  return rc;
#else
  // Linux: Use sem_timedwait
  struct timespec ts;
  clock_gettime(CLOCK_REALTIME, &ts);

  // Add timeout_us microseconds
  long nsec = ts.tv_nsec + (timeout_us * 1000L);
  ts.tv_sec += nsec / 1000000000L;
  ts.tv_nsec = nsec % 1000000000L;

  atomic_fetch_add_explicit(&q->waiters, 1, memory_order_acq_rel);
  int result = sem_timedwait(&q->items, &ts);
  atomic_fetch_sub_explicit(&q->waiters, 1, memory_order_acq_rel);
  return (result == 0);
#endif
}

void rq_reset(ReadyQ *q) {
  if (!q)
    return;

  // Drain any remaining items from the underlying MPMC queue
  int32_t dummy;
  while (rq_try_pop(q, &dummy)) {
    // Discard stale items
  }

  // Reset logical length counter
  atomic_store_explicit(&q->qlen, 0, memory_order_relaxed);

  // Drain semaphore - consume any pending signals
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

// Batch push multiple items with a single semaphore signal at the end
// This dramatically reduces kernel calls when seeding many source nodes
void rq_push_batch(ReadyQ *q, const int32_t *nids, int count) {
  if (!q || !nids || count <= 0)
    return;

  int pushed = 0;
  for (int i = 0; i < count; i++) {
    // Push directly to MPMC without signaling
    for (;;) {
      if (mpmc_push(q->ring, nids[i])) {
        atomic_fetch_add_explicit(&q->qlen, 1, memory_order_acq_rel);
        pushed++;
        break;
      }
      cpu_relax();
    }
  }

  // Single signal after all items are pushed - wakes one worker
  // which will then wake others as needed through work stealing
  if (pushed > 0) {
#ifdef __APPLE__
    dispatch_semaphore_signal(q->items);
#else
    sem_post(&q->items);
#endif
  }
}
