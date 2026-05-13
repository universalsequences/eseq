#ifndef GRAPH_TYPES_H
#define GRAPH_TYPES_H

#define _GNU_SOURCE
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

// ===================== Constants =====================
#define MAX_IO 32 // Initial per-thread IO buffer chunk (grows dynamically)

// ===================== Helpers =====================
static inline void *alloc_aligned(size_t alignment, size_t size) {
  void *p = NULL;
  if (posix_memalign(&p, alignment, size) != 0)
    return NULL;
  return p;
}

static inline float *alloc_state_f32(size_t n_floats, size_t alignment) {
  // guard overflow
  size_t bytes = n_floats * sizeof(float);
#if defined(__has_builtin) && __has_builtin(__builtin_mul_overflow)
  if (__builtin_mul_overflow(n_floats, sizeof(float), &bytes))
    return NULL;
#endif
  void *p = NULL;
  int rc = posix_memalign(&p, alignment, bytes);
  if (rc != 0)
    return NULL;
  memset(p, 0, bytes);
  return (float *)p;
}

static inline uint64_t nsec_now(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}

// ===================== Kernel ABI =====================
typedef void (*KernelFn)(float *const *in, float *const *out, int nframes,
                         void *state, void *buffers);
typedef void (*InitFn)(void *state, int sampleRate, int maxBlock,
                       const void *initial_state);
typedef void (*ResetFn)(void *state);
typedef void (*MigrateFn)(void *newState, const void *oldState);

typedef struct {
  KernelFn process;
  InitFn init;       // optional
  ResetFn reset;     // optional
  MigrateFn migrate; // optional: copy persistent state on graph swap
} NodeVTable;

// ===================== Parameter Mailbox =====================
typedef enum { PARAM_SET_GAIN = 1 } ParamKind;

typedef struct {
  uint64_t idx;        // target parameter index
  uint64_t logical_id; // target node
  float fvalue;        // e.g., new gain
} ParamMsg;

#define PARAM_RING_CAP 2048

typedef struct ParamRing {
  ParamMsg buf[PARAM_RING_CAP];
  _Atomic uint32_t head; // producer writes
  _Atomic uint32_t tail; // consumer reads
} ParamRing;

// ===================== MPMC Work Queue =====================

#include "mpmc_queue.h"

// Parameter ring operations
static inline bool params_push(ParamRing *r, ParamMsg m) {
  uint32_t h = atomic_load_explicit(&r->head, memory_order_relaxed);
  uint32_t t = atomic_load_explicit(&r->tail, memory_order_acquire);
  if ((h - t) >= PARAM_RING_CAP)
    return false; // full
  r->buf[h % PARAM_RING_CAP] = m;
  atomic_store_explicit(&r->head, h + 1, memory_order_release);
  return true;
}

static inline bool params_pop(ParamRing *r, ParamMsg *out) {
  uint32_t t = atomic_load_explicit(&r->tail, memory_order_relaxed);
  uint32_t h = atomic_load_explicit(&r->head, memory_order_acquire);
  if (t == h)
    return false; // empty
  *out = r->buf[t % PARAM_RING_CAP];
  atomic_store_explicit(&r->tail, t + 1, memory_order_release);
  return true;
}

typedef enum {
  GE_ADD_NODE,
  GE_REMOVE_NODE,
  GE_CONNECT,
  GE_DISCONNECT,
  GE_HOT_SWAP_NODE, // port compatible: swap vtable+state (option xfade/migrate)
  GE_REPLACE_KEEP_EDGES, // incompatible signature: remap/auto-disconnect, keep
                         // slot
  GE_ADD_WATCH,
  GE_REMOVE_WATCH,

  GE_CREATE_BUFFER,
  GE_HOTSWAP_BUFFER

} GraphEditOp;

typedef struct {
  NodeVTable vt;
  size_t state_size; // size of memory to allocate for state
  int node_id;       // target slot to swap
  int new_nInputs;
  int new_nOutputs;
  void *initial_state;       // Optional initial state data
  size_t initial_state_size; // Size of initial_state data
} GEHotSwapNode;

typedef struct {
  NodeVTable vt;
  size_t state_size; // size of memory to allocate for state
  int node_id;       // target slot to replace
  int new_nInputs;
  int new_nOutputs;
  void *initial_state;       // Optional initial state data
  size_t initial_state_size; // Size of initial_state data
  // Policy flags:
  // - if shrinking inputs/outputs, auto-disconnect excess ports
  // deterministically
  // - if growing, new ports initialize to -1
} GEReplaceKeepEdges;

typedef struct {
  uint64_t buffer_id;
  int size;
  int channel_count;
  float *source_data;      // Optional: data to copy into buffer (caller-owned, freed after apply)
  size_t source_data_size; // Size in bytes of source_data
} GECreateBuffer;

typedef struct {
  uint64_t buffer_id;
  int size;                // New size (samples per channel)
  int channel_count;       // New channel count
  float *source_data;      // New data to copy into buffer (caller-owned, freed after apply)
  size_t source_data_size; // Size in bytes of source_data
} GEHotSwapBuffer;

typedef struct {
  GraphEditOp op;
  uint64_t batch_serial;
  union {
    struct {
      NodeVTable vt;
      size_t state_size;
      uint64_t logical_id;
      char *name;
      int nInputs;
      int nOutputs;
      void *initial_state;       // Optional initial state data
      size_t initial_state_size; // Size of initial_state data
    } add_node;
    struct {
      int node_id;
    } remove_node;
    struct {
      int src_id, src_port, dst_id, dst_port;
    } connect;
    struct {
      int src_id, src_port, dst_id, dst_port;
    } disconnect;
    GEHotSwapNode hot_swap_node;
    GEReplaceKeepEdges replace_keep_edges;
    struct {
      int node_id;
    } add_watch;
    struct {
      int node_id;
    } remove_watch;
    GECreateBuffer create_buffer;
    GEHotSwapBuffer hotswap_buffer;
  } u;
} GraphEditCmd;

typedef struct GraphEditQueue {
  GraphEditCmd *buf;
  uint32_t cap;  // power-of-two
  uint32_t mask; // cap - 1

  _Atomic uint32_t head; // producer writes
  _Atomic uint32_t tail; // consumer reads
} GraphEditQueue;

// ===================== Ready Queue (MPMC + length + semaphore)
// =====================

#ifdef __APPLE__
#include <dispatch/dispatch.h>
// macOS doesn't support POSIX unnamed semaphores, use dispatch_semaphore
// instead
typedef struct {
  MPMCQueue *ring;            // Thread-safe node storage
  _Atomic int qlen;           // Logical queue length
  _Atomic int waiters;        // Number of threads waiting on items
  dispatch_semaphore_t items; // Semaphore used for wakeups
} ReadyQ;
#else
#include <semaphore.h>
typedef struct {
  MPMCQueue *ring;     // Thread-safe node storage
  _Atomic int qlen;    // Logical queue length
  _Atomic int waiters; // Number of threads waiting on items
  sem_t items;         // Semaphore used for wakeups
} ReadyQ;
#endif

// CPU relaxation (platform specific)
#if defined(__x86_64__) || defined(__i386__)
#define cpu_relax() __builtin_ia32_pause()
#elif defined(__aarch64__) || defined(__arm__)
#define cpu_relax() __asm__ __volatile__("yield" ::: "memory")
#else
#define cpu_relax() __asm__ __volatile__("" ::: "memory")
#endif

// ReadyQ operations
ReadyQ *rq_create(int capacity);
void rq_destroy(ReadyQ *q);
bool rq_push(ReadyQ *q, int32_t nid);
bool rq_try_pop(ReadyQ *q, int32_t *out);
bool rq_wait_nonempty(ReadyQ *q, int timeout_us);
void rq_reset(ReadyQ *q); // Reset/drain queue for clean block start
void rq_push_or_spin(ReadyQ *q, int32_t nid); // Retry until enqueue succeeds
void rq_push_batch(ReadyQ *q, const int32_t *nids, int count); // Batch push with single signal

#endif // GRAPH_TYPES_H
