#ifndef MPMC_QUEUE_H
#define MPMC_QUEUE_H

// Vyukov-style MPMC bounded queue with per-cell sequence numbers.
// Lock-free, cache-line aligned, ABA-safe.

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdatomic.h>

// Forward-declare alloc_aligned (defined in graph_types.h or provide inline)
static inline void *mpmc_alloc_aligned(size_t alignment, size_t size) {
  void *p = NULL;
  if (posix_memalign(&p, alignment, size) != 0)
    return NULL;
  return p;
}

typedef struct {
  _Atomic uint64_t sequence; // Per-cell sequence number for ABA protection
  int32_t value;             // Node ID to process
  char padding[64 - sizeof(_Atomic uint64_t) -
               sizeof(int32_t)]; // Cache line align
} __attribute__((aligned(64))) MPMCCell;

typedef struct {
  MPMCCell *buffer; // Ring buffer of cells
  uint32_t mask;    // Capacity - 1 (must be power of 2)
  char padding1[64 - sizeof(MPMCCell *) - sizeof(uint32_t)];
  _Atomic uint64_t head; // Producer cursor
  char padding2[64 - sizeof(_Atomic uint64_t)];
  _Atomic uint64_t tail; // Consumer cursor
  char padding3[64 - sizeof(_Atomic uint64_t)];
} __attribute__((aligned(64))) MPMCQueue;

static inline MPMCQueue *mpmc_create(uint32_t capacity) {
  // Capacity must be power of 2
  if ((capacity & (capacity - 1)) != 0)
    return NULL;

  MPMCQueue *q = (MPMCQueue *)mpmc_alloc_aligned(64, sizeof(MPMCQueue));
  if (!q)
    return NULL;

  q->buffer = (MPMCCell *)mpmc_alloc_aligned(64, capacity * sizeof(MPMCCell));
  if (!q->buffer) {
    free(q);
    return NULL;
  }

  q->mask = capacity - 1;
  atomic_store_explicit(&q->head, 0, memory_order_relaxed);
  atomic_store_explicit(&q->tail, 0, memory_order_relaxed);

  for (uint32_t i = 0; i < capacity; ++i) {
    atomic_store_explicit(&q->buffer[i].sequence, i, memory_order_relaxed);
  }

  return q;
}

static inline void mpmc_destroy(MPMCQueue *q) {
  if (q) {
    free(q->buffer);
    free(q);
  }
}

static inline bool mpmc_push(MPMCQueue *q, int32_t value) {
  MPMCCell *cell;
  uint64_t pos = atomic_load_explicit(&q->head, memory_order_relaxed);

  for (;;) {
    cell = &q->buffer[pos & q->mask];
    uint64_t seq = atomic_load_explicit(&cell->sequence, memory_order_acquire);
    intptr_t dif = (intptr_t)seq - (intptr_t)pos;

    if (dif == 0) {
      if (atomic_compare_exchange_weak_explicit(&q->head, &pos, pos + 1,
                                                memory_order_relaxed,
                                                memory_order_relaxed))
        break;
    } else if (dif < 0) {
      return false; // full
    } else {
      pos = atomic_load_explicit(&q->head, memory_order_relaxed);
    }
  }

  cell->value = value;
  atomic_store_explicit(&cell->sequence, pos + 1, memory_order_release);
  return true;
}

static inline bool mpmc_pop(MPMCQueue *q, int32_t *value) {
  MPMCCell *cell;
  uint64_t pos = atomic_load_explicit(&q->tail, memory_order_relaxed);

  for (;;) {
    cell = &q->buffer[pos & q->mask];
    uint64_t seq = atomic_load_explicit(&cell->sequence, memory_order_acquire);
    intptr_t dif = (intptr_t)seq - (intptr_t)(pos + 1);

    if (dif == 0) {
      if (atomic_compare_exchange_weak_explicit(&q->tail, &pos, pos + 1,
                                                memory_order_relaxed,
                                                memory_order_relaxed))
        break;
    } else if (dif < 0) {
      return false; // empty
    } else {
      pos = atomic_load_explicit(&q->tail, memory_order_relaxed);
    }
  }

  *value = cell->value;
  atomic_store_explicit(&cell->sequence, pos + q->mask + 1,
                        memory_order_release);
  return true;
}

#endif // MPMC_QUEUE_H
