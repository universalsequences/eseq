#include "graph_engine.h"
#include "graph_edit.h"
#include "graph_nodes.h"
#include <assert.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

// On Apple platforms, enable QoS hints for worker threads to reduce jitter.
#ifdef __APPLE__
#if __has_include(<pthread/qos.h>)
#include <pthread/qos.h>
#endif
#if __has_include(<os/workgroup.h>)
#include <os/workgroup.h>
#define HAVE_OS_WORKGROUP 1
#endif
#if __has_include(<mach/mach.h>)
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/thread_policy.h>
#define HAVE_MACH_RT 1
#endif
#endif

// ===================== Forward Declarations =====================

void bind_and_run_live(LiveGraph *lg, int nid, int nframes);
static void init_pending_and_seed(LiveGraph *lg);
void process_live_block(LiveGraph *lg, int nframes);
static inline void execute_and_fanout(LiveGraph *lg, int32_t nid, int nframes);
static void wait_for_block_start_or_shutdown(void);
static void rebuild_invalid_io_caches(LiveGraph *lg, int nframes);
static int choose_active_worker_count(LiveGraph *lg);

#ifndef AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
#define AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS 0
#endif

#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
#define PENDING_RUNNING_SENTINEL (-2)
#define PENDING_DONE_SENTINEL (-3)
#define COMPLETION_LOG_CAPACITY 64
#endif

// ===================== Global Engine Instance =====================

Engine g_engine;

// Worker threads should be less aggressive than the audio callback thread.
// The audio thread still actively helps drain the graph, but idle helper
// workers back off quickly instead of burning CPU polling an empty ready queue.
#ifndef AUDIOGRAPH_WORKER_EMPTY_SPINS
#define AUDIOGRAPH_WORKER_EMPTY_SPINS 8
#endif

#ifndef AUDIOGRAPH_WORKER_WAIT_TIMEOUT_US
#define AUDIOGRAPH_WORKER_WAIT_TIMEOUT_US 50
#endif

// Watchlist snapshots are useful for UI polling, but copying all watched node
// state every audio callback can be expensive. Direct process_live_block()
// callers still update every call; process_next_block() throttles snapshots.
#ifndef AUDIOGRAPH_WATCH_UPDATE_INTERVAL
#define AUDIOGRAPH_WATCH_UPDATE_INTERVAL 4
#endif

static _Atomic uint32_t g_watch_update_counter = 0;

static bool using_inline_in_cache(const RTNode *node) {
  return node->cached_inPtrs == (float **)node->cached_inInline;
}

static bool using_inline_out_cache(const RTNode *node) {
  return node->cached_outPtrs == (float **)node->cached_outInline;
}

// ===================== SUM Node Input Count Tracking =====================

// Thread-local storage for current node being processed
static __thread RTNode *g_current_processing_node = NULL;

#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
static __thread int g_current_execution_slot = 0;

#define MAX_TRACKED_EXECUTION_SLOTS 65
static _Atomic int g_inflight_node_ids[MAX_TRACKED_EXECUTION_SLOTS];
static _Atomic uint64_t g_completion_seq = 0;
static _Atomic int g_completed_jobs = 0;
static _Atomic int g_completion_log_nodes[COMPLETION_LOG_CAPACITY];
static _Atomic uint64_t g_completion_log_seq[COMPLETION_LOG_CAPACITY];

static void dump_inflight_nodes(LiveGraph *lg) {
  fprintf(stderr, "[audiograph] in-flight node dump begin\n");
  for (int slot = 0; slot < MAX_TRACKED_EXECUTION_SLOTS; slot++) {
    int nid = atomic_load_explicit(&g_inflight_node_ids[slot], memory_order_acquire);
    if (nid < 0 || !lg || nid >= lg->node_count) {
      continue;
    }
    RTNode *node = &lg->nodes[nid];
    fprintf(stderr, "[audiograph] in-flight slot=%d node=%d logical=%llu name=%s\n",
            slot, nid, (unsigned long long)node->logical_id,
            node->debug_name ? node->debug_name : "<unnamed>");
  }
  fprintf(stderr, "[audiograph] in-flight node dump end\n");
}

static void dump_completion_log(LiveGraph *lg) {
  int completed_jobs =
      atomic_load_explicit(&g_completed_jobs, memory_order_acquire);
  uint64_t completion_seq =
      atomic_load_explicit(&g_completion_seq, memory_order_acquire);
  fprintf(stderr,
          "[audiograph] completion summary completed_jobs=%d completion_seq=%llu\n",
          completed_jobs, (unsigned long long)completion_seq);
  fprintf(stderr, "[audiograph] completion log begin\n");

  uint64_t start_seq = 0;
  if (completion_seq > COMPLETION_LOG_CAPACITY) {
    start_seq = completion_seq - COMPLETION_LOG_CAPACITY;
  }
  for (uint64_t seq = start_seq; seq < completion_seq; seq++) {
    int slot = (int)(seq % COMPLETION_LOG_CAPACITY);
    uint64_t slot_seq =
        atomic_load_explicit(&g_completion_log_seq[slot], memory_order_acquire);
    if (slot_seq != seq + 1) {
      continue;
    }
    int nid =
        atomic_load_explicit(&g_completion_log_nodes[slot], memory_order_acquire);
    if (!lg || nid < 0 || nid >= lg->node_count) {
      fprintf(stderr,
              "[audiograph] completion seq=%llu nid=%d INVALID\n",
              (unsigned long long)seq, nid);
      continue;
    }
    RTNode *node = &lg->nodes[nid];
    fprintf(stderr,
            "[audiograph] completion seq=%llu nid=%d logical=%llu name=%s\n",
            (unsigned long long)seq, nid, (unsigned long long)node->logical_id,
            node->debug_name ? node->debug_name : "<unnamed>");
  }
  fprintf(stderr, "[audiograph] completion log end\n");
}

static inline bool claim_ready_node(LiveGraph *lg, int32_t nid) {
  if (!lg || nid < 0 || nid >= lg->node_count) {
    return false;
  }
  int expected = 0;
  if (atomic_compare_exchange_strong_explicit(&lg->sched.pending[nid], &expected,
                                              PENDING_RUNNING_SENTINEL,
                                              memory_order_acq_rel,
                                              memory_order_acquire)) {
    return true;
  }

  RTNode *node = &lg->nodes[nid];
  fprintf(stderr,
          "[audiograph] WARN: duplicate/spurious ready pop nid=%d logical=%llu pending=%d name=%s\n",
          nid, (unsigned long long)node->logical_id, expected,
          node->debug_name ? node->debug_name : "<unnamed>");
  return false;
}
#endif

int ap_current_node_ninputs(void) {
  if (g_current_processing_node) {
    return g_current_processing_node->nInputs;
  }
  return 0; // fallback
}

void initialize_engine(int block_Size, int sample_rate) {
  g_engine.blockSize = block_Size;
  g_engine.sampleRate = sample_rate;
  atomic_store_explicit(&g_engine.oswg, NULL, memory_order_relaxed);
  atomic_store_explicit(&g_engine.oswg_join_pending, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.oswg_join_remaining, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.oswg_version, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.rt_time_constraint, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.activeWorkerLimit, 0, memory_order_relaxed);
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
  for (int i = 0; i < MAX_TRACKED_EXECUTION_SLOTS; i++) {
    atomic_store_explicit(&g_inflight_node_ids[i], -1, memory_order_relaxed);
  }
  atomic_store_explicit(&g_completion_seq, 0, memory_order_relaxed);
  atomic_store_explicit(&g_completed_jobs, 0, memory_order_relaxed);
  for (int i = 0; i < COMPLETION_LOG_CAPACITY; i++) {
    atomic_store_explicit(&g_completion_log_nodes[i], -1, memory_order_relaxed);
    atomic_store_explicit(&g_completion_log_seq[i], 0, memory_order_relaxed);
  }
#endif
}

// ===================== Graph Management =====================

// ===================== Parameter Application =====================

#define DGEN_HEADER_SLOTS 4
#define DGEN_CANARY_INDEX 2
#define DGEN_HEADER_CANARY_BITS 0x4cd35a1dU
#define DGEN_STATE_REDZONE_SLOTS 256

static inline bool has_dgen_header_canary(const float *memory) {
  union {
    float f;
    uint32_t u;
  } bits = {.f = memory[DGEN_CANARY_INDEX]};
  return bits.u == DGEN_HEADER_CANARY_BITS;
}

void apply_params(LiveGraph *g) {
  if (!g || !g->params)
    return;
  ParamMsg m;
  while (params_pop(g->params, &m)) {
    // O(1) direct lookup: logical_id is used as the array index in apply_add_node
    int node_id = (int)m.logical_id;
    if (node_id >= 0 && node_id < g->node_count) {
      RTNode *node = &g->nodes[node_id];
      // Verify logical_id matches (safety check for deleted/reused slots)
      if (node->state && node->logical_id == m.logical_id) {
        float *memory = (float *)node->state;
        int state_slots = (int)(node->state_size / sizeof(float));
        assert(m.idx < (uint64_t)state_slots);
        memory[m.idx] = m.fvalue;
        if (m.idx >= DGEN_HEADER_SLOTS && has_dgen_header_canary(memory)) {
          int total_slots = (int)memory[1];
          if (total_slots > 0) {
            int write_base = DGEN_HEADER_SLOTS + total_slots + DGEN_STATE_REDZONE_SLOTS;
            int mirrored_idx = write_base + (m.idx - DGEN_HEADER_SLOTS);
            if (mirrored_idx >= 0 && mirrored_idx < state_slots) {
              memory[mirrored_idx] = m.fvalue;
            }
          }
        }
      }
    }
  }
}

// ===================== Block Processing =====================

// Legacy bind_and_run function removed - using port-based bind_and_run_live
// only

static void wait_for_block_start_or_shutdown(void) {
  pthread_mutex_lock(&g_engine.sess_mtx);
  for (;;) {
    if (!atomic_load_explicit(&g_engine.runFlag, memory_order_acquire))
      break;
    // Also wake if workgroup join is pending
    if (atomic_load_explicit(&g_engine.oswg_join_pending, memory_order_acquire))
      break;
    LiveGraph *lg =
        atomic_load_explicit(&g_engine.workSession, memory_order_acquire);
    if (lg && atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire) > 0)
      break;
    pthread_cond_wait(&g_engine.sess_cv, &g_engine.sess_mtx);
  }
  pthread_mutex_unlock(&g_engine.sess_mtx);
}

static void *worker_main(void *arg) {
  intptr_t worker_slot = (intptr_t)arg;
  int worker_index = (int)worker_slot - 1;
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
  g_current_execution_slot = (int)worker_slot;
#endif
  // Elevate worker thread QoS on Apple platforms for better scheduling.
#ifdef __APPLE__
#ifdef QOS_CLASS_USER_INTERACTIVE
  (void)pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
#endif
#endif

#ifdef HAVE_MACH_RT
  // Optionally promote to Mach time-constraint scheduling
  if (atomic_load_explicit(&g_engine.rt_time_constraint,
                           memory_order_acquire)) {
    // Compute period from engine config
    double sr =
        (g_engine.sampleRate > 0) ? (double)g_engine.sampleRate : 48000.0;
    double bs = (g_engine.blockSize > 0) ? (double)g_engine.blockSize : 512.0;
    double period_ns_d = (bs / sr) * 1e9; // block duration in ns
    uint64_t period_ns = (uint64_t)(period_ns_d + 0.5);
    // Budget ~75% of period, constraint = period
    uint64_t comp_ns = (period_ns * 3) / 4;
    uint64_t cons_ns = period_ns;

    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    uint64_t period_abs = (period_ns * tb.denom) / tb.numer;
    uint64_t comp_abs = (comp_ns * tb.denom) / tb.numer;
    uint64_t cons_abs = (cons_ns * tb.denom) / tb.numer;

    thread_time_constraint_policy_data_t pol;
    pol.period = (uint32_t)period_abs;
    pol.computation = (uint32_t)comp_abs;
    pol.constraint = (uint32_t)cons_abs;
    pol.preemptible = TRUE;

    kern_return_t kr = thread_policy_set(
        mach_thread_self(), THREAD_TIME_CONSTRAINT_POLICY,
        (thread_policy_t)&pol, THREAD_TIME_CONSTRAINT_POLICY_COUNT);
    if (kr != KERN_SUCCESS) {
      fprintf(stderr,
              "[audiograph] WARN: thread_policy_set RT failed (kr=%d)\n", kr);
    } else if (atomic_load_explicit(&g_engine.rt_log, memory_order_relaxed)) {
      fprintf(stderr,
              "[audiograph] worker %p set Mach RT TC (period=%.2f ms)\n",
              (void *)pthread_self(), period_ns_d / 1e6);
    }
  }
#endif

#ifdef HAVE_OS_WORKGROUP
  os_workgroup_t oswg = NULL;
  os_workgroup_join_token_s oswg_token;  // Stack-allocated token struct (not pointer)
  memset(&oswg_token, 0, sizeof(oswg_token));
  bool oswg_joined = false;
  int oswg_local_version = 0; // Track which version we've joined
#endif
  for (;;) {
    if (!atomic_load_explicit(&g_engine.runFlag, memory_order_acquire))
      break;

    // Park until a block is published
    wait_for_block_start_or_shutdown();
    if (!atomic_load_explicit(&g_engine.runFlag, memory_order_acquire))
      break;

    // Handle OS workgroup joining/re-joining when version changes
    // This allows workers to switch workgroups without being recreated
#ifdef HAVE_OS_WORKGROUP
    int global_version = atomic_load_explicit(&g_engine.oswg_version, memory_order_acquire);
    if (global_version != oswg_local_version) {
      // Version changed - need to leave old workgroup and join new one
      // IMPORTANT: We must leave using our saved oswg pointer and token,
      // not the global one (which may have changed or been freed)
      if (oswg_joined) {
        // We have a valid join - leave using our saved references
        os_workgroup_leave(oswg, &oswg_token);
        if (atomic_load_explicit(&g_engine.rt_log, memory_order_relaxed))
          fprintf(stderr, "[audiograph] worker %p left os_workgroup %p (version %d -> %d)\n",
                  (void *)pthread_self(), (void *)oswg, oswg_local_version, global_version);
        oswg_joined = false;
        oswg = NULL;
        memset(&oswg_token, 0, sizeof(oswg_token));
      }

      // Update local version before attempting join
      oswg_local_version = global_version;

      void *w = atomic_load_explicit(&g_engine.oswg, memory_order_acquire);
      if (w) {
        oswg = (os_workgroup_t)w;
        int ok = os_workgroup_join(oswg, &oswg_token);
        oswg_joined = (ok == 0);
        if (oswg_joined) {
          if (atomic_load_explicit(&g_engine.rt_log, memory_order_relaxed))
            fprintf(stderr, "[audiograph] worker %p joined os_workgroup %p (version %d)\n",
                    (void *)pthread_self(), (void *)oswg, global_version);
        } else {
          fprintf(stderr, "[audiograph] worker %p FAILED to join os_workgroup %p (err=%d)\n",
                  (void *)pthread_self(), (void *)oswg, ok);
          oswg = NULL;  // Don't keep stale pointer on failure
        }
      }
      // Decrement remaining counter; last worker clears the pending flag
      int remaining = atomic_fetch_sub_explicit(&g_engine.oswg_join_remaining, 1,
                                                memory_order_acq_rel) - 1;
      if (remaining == 0) {
        atomic_store_explicit(&g_engine.oswg_join_pending, 0, memory_order_release);
      }
    }
#endif

    LiveGraph *lg =
        atomic_load_explicit(&g_engine.workSession, memory_order_acquire);
    if (!lg)
      continue; // spurious wake or no work - but workgroup joining is done

    // Adaptive worker limit: workers above the per-block limit stay out of the
    // ready queue. Hosts can keep a high max worker count for complex graphs
    // without paying wake/steal jitter on tiny or mostly-serial graphs.
    int active_limit = atomic_load_explicit(&g_engine.activeWorkerLimit,
                                            memory_order_acquire);
    if (worker_index >= active_limit) {
      while (atomic_load_explicit(&g_engine.runFlag, memory_order_acquire)) {
        LiveGraph *cur =
            atomic_load_explicit(&g_engine.workSession, memory_order_acquire);
        if (cur != lg)
          break;
        if (atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire) == 0)
          break;
        usleep(50);
      }
      continue;
    }

    // Hot loop: run until this block is complete
    for (;;) {
      // If the session ended or graph pointer changed, exit the hot loop.
      LiveGraph *cur =
          atomic_load_explicit(&g_engine.workSession, memory_order_acquire);
      if (cur != lg)
        break;
      if (atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire) == 0)
        break;

      int32_t nid;

      // Modest spin to catch bursts without kernel call, then short timed wait.
      bool got = false;
      for (int s = 0; s < AUDIOGRAPH_WORKER_EMPTY_SPINS; s++) {
        if ((got = rq_try_pop(lg->sched.readyQueue, &nid)))
          break;
        cpu_relax(); // brief pause
      }
      if (!got) {
        (void)rq_wait_nonempty(lg->sched.readyQueue,
                               /*timeout_us=*/AUDIOGRAPH_WORKER_WAIT_TIMEOUT_US);
        continue;
      }

      // Validate job ID to avoid crashes if queue is corrupted under load
      if (nid < 0 || nid >= lg->node_count) {
        fprintf(stderr,
                "[audiograph] WARN: invalid job id %d (node_count=%d)\n", nid,
                lg->node_count);
        continue;
      }

      int nf =
          atomic_load_explicit(&g_engine.sessionFrames, memory_order_acquire);
      if (nf <= 0 || nf > lg->block_size) {
        nf = lg->block_size; // Clamp to graph's internal block size for safety
      }
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
      if (!claim_ready_node(lg, nid)) {
        continue;
      }
#endif
      execute_and_fanout(lg, nid, nf);
    }

    // Loop back: will go to sleep on sess_cv until next block
  }

  // Thread exiting: leave workgroup if still joined
#ifdef HAVE_OS_WORKGROUP
  if (oswg_joined && oswg) {
    os_workgroup_leave(oswg, &oswg_token);
    oswg_joined = false;
  }
#endif

  return NULL;
}

// ===================== Worker Pool Management =====================

void engine_start_workers(int workers) {
  g_engine.workerCount = workers;
  g_engine.threads = (pthread_t *)calloc(workers, sizeof(pthread_t));

  // Initialize mutex and condition variable for block-start wake
  pthread_mutex_init(&g_engine.sess_mtx, NULL);
  pthread_cond_init(&g_engine.sess_cv, NULL);

  atomic_store(&g_engine.runFlag, 1);
  for (int i = 0; i < workers; i++) {
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    // Hint a high QoS class on Apple platforms; no-ops elsewhere.
#ifdef __APPLE__
#ifdef QOS_CLASS_USER_INTERACTIVE
    (void)pthread_attr_set_qos_class_np(&attr, QOS_CLASS_USER_INTERACTIVE, 0);
#endif
#endif
    pthread_create(&g_engine.threads[i], &attr, worker_main,
                   (void *)(intptr_t)(i + 1));
    pthread_attr_destroy(&attr);
  }
}

void engine_set_os_workgroup(void *oswg_ptr) {
#ifdef HAVE_OS_WORKGROUP
  // Store opaque pointer; Swift side retains it.
  atomic_store_explicit(&g_engine.oswg, oswg_ptr, memory_order_release);

  // Increment version to signal workers to re-join
  int new_version = atomic_fetch_add_explicit(&g_engine.oswg_version, 1,
                                              memory_order_acq_rel) + 1;

  // Set counter to number of workers, then set flag and broadcast
  // This ensures all workers see the flag before it's cleared
  atomic_store_explicit(&g_engine.oswg_join_remaining, g_engine.workerCount,
                        memory_order_release);
  atomic_store_explicit(&g_engine.oswg_join_pending, 1, memory_order_release);
  pthread_mutex_lock(&g_engine.sess_mtx);
  pthread_cond_broadcast(&g_engine.sess_cv);
  pthread_mutex_unlock(&g_engine.sess_mtx);

  if (atomic_load_explicit(&g_engine.rt_log, memory_order_relaxed))
    fprintf(stderr,
            "[audiograph] set os_workgroup=%p (version %d, notifying %d existing workers)\n",
            oswg_ptr, new_version, g_engine.workerCount);
#else
  (void)oswg_ptr;
#endif
}

void engine_clear_os_workgroup(void) {
#ifdef HAVE_OS_WORKGROUP
  // First, signal workers to leave by setting NULL and incrementing version
  atomic_store_explicit(&g_engine.oswg, NULL, memory_order_release);
  int new_version = atomic_fetch_add_explicit(&g_engine.oswg_version, 1,
                                              memory_order_acq_rel) + 1;

  atomic_store_explicit(&g_engine.oswg_join_remaining, g_engine.workerCount,
                        memory_order_release);
  atomic_store_explicit(&g_engine.oswg_join_pending, 1, memory_order_release);
  pthread_mutex_lock(&g_engine.sess_mtx);
  pthread_cond_broadcast(&g_engine.sess_cv);
  pthread_mutex_unlock(&g_engine.sess_mtx);

  if (atomic_load_explicit(&g_engine.rt_log, memory_order_relaxed))
    fprintf(stderr,
            "[audiograph] clearing os_workgroup (version %d, waiting for %d workers to leave)\n",
            new_version, g_engine.workerCount);

  // IMPORTANT: Wait for all workers to leave before returning
  // This ensures Swift can safely release the old workgroup
  int timeout_ms = 1000;  // 1 second timeout
  int waited_ms = 0;
  while (atomic_load_explicit(&g_engine.oswg_join_pending, memory_order_acquire) != 0) {
    usleep(1000);  // 1ms
    waited_ms++;
    if (waited_ms >= timeout_ms) {
      int remaining = atomic_load_explicit(&g_engine.oswg_join_remaining, memory_order_acquire);
      fprintf(stderr,
              "[audiograph] WARNING: timeout waiting for workers to leave workgroup (%d remaining)\n",
              remaining);
      break;
    }
  }

  if (atomic_load_explicit(&g_engine.rt_log, memory_order_relaxed))
    fprintf(stderr, "[audiograph] os_workgroup cleared (workers left in %d ms)\n", waited_ms);
#else
  (void)0; // os_workgroup unsupported
#endif
}

void engine_enable_rt_logging(int enable) {
  atomic_store_explicit(&g_engine.rt_log, enable ? 1 : 0, memory_order_release);
}

void engine_enable_rt_time_constraint(int enable) {
  atomic_store_explicit(&g_engine.rt_time_constraint, enable ? 1 : 0,
                        memory_order_release);
}

void engine_stop_workers(void) {
  atomic_store(&g_engine.runFlag, 0);

  // Wake sleepers on both wait sites
  pthread_mutex_lock(&g_engine.sess_mtx);
  pthread_cond_broadcast(&g_engine.sess_cv);
  pthread_mutex_unlock(&g_engine.sess_mtx);

  // Also wake any workers blocked in rq_wait_nonempty during a block
  // We'll iterate through all potential live graphs, but since we're shutting
  // down, we can just wait for threads to exit naturally

  for (int i = 0; i < g_engine.workerCount; i++) {
    pthread_join(g_engine.threads[i], NULL);
  }

  // Clean up synchronization primitives
  pthread_mutex_destroy(&g_engine.sess_mtx);
  pthread_cond_destroy(&g_engine.sess_cv);

  free(g_engine.threads);
  g_engine.threads = NULL;
  g_engine.workerCount = 0;
}

// ===================== Live Graph Operations =====================

// Rebuild IO cache for a single node (called lazily when cache is invalid)
static void rebuild_node_io_cache(LiveGraph *lg, RTNode *node, int nframes) {
  (void)nframes;  // Currently unused but might be needed later

  // Reallocate cached pointer arrays if size changed
  // This handles cases where SUM nodes grow their input count
  if (node->nInputs > 0) {
    if (node->cached_inPtrs && !using_inline_in_cache(node)) {
      free(node->cached_inPtrs);
    }
    if (node->nInputs <= MAX_IO) {
      node->cached_inPtrs = (float **)node->cached_inInline;
    } else {
      node->cached_inPtrs = malloc(node->nInputs * sizeof(float *));
    }
  }
  if (node->nOutputs > 0) {
    if (node->cached_outPtrs && !using_inline_out_cache(node)) {
      free(node->cached_outPtrs);
    }
    if (node->nOutputs <= MAX_IO) {
      node->cached_outPtrs = (float **)node->cached_outInline;
    } else {
      node->cached_outPtrs = malloc(node->nOutputs * sizeof(float *));
    }
  }

  // Resolve input pointers
  if (node->cached_inPtrs) {
    for (int i = 0; i < node->nInputs; i++) {
      int eid = node->inEdgeId ? node->inEdgeId[i] : -1;
      if (eid >= 0 && eid < lg->edge_capacity && lg->edges[eid].buf) {
        node->cached_inPtrs[i] = lg->edges[eid].buf;
      } else {
        node->cached_inPtrs[i] = lg->silence_buf;
      }
    }
  }

  // Resolve output pointers
  if (node->cached_outPtrs) {
    for (int i = 0; i < node->nOutputs; i++) {
      int eid = node->outEdgeId ? node->outEdgeId[i] : -1;
      if (eid >= 0 && eid < lg->edge_capacity && lg->edges[eid].buf) {
        node->cached_outPtrs[i] = lg->edges[eid].buf;
      } else {
        node->cached_outPtrs[i] = lg->scratch_null;
      }
    }
  }

  node->io_cache_valid = true;
}

static void rebuild_invalid_io_caches(LiveGraph *lg, int nframes) {
  for (int i = 0; i < lg->node_count; i++) {
    RTNode *node = &lg->nodes[i];
    bool deleted = (node->vtable.process == NULL && node->nInputs == 0 &&
                    node->nOutputs == 0);
    if (deleted)
      continue;
    if (!node->io_cache_valid) {
      rebuild_node_io_cache(lg, node, nframes);
    }
  }
}

void bind_and_run_live(LiveGraph *lg, int nid, int nframes) {
  RTNode *node = &lg->nodes[nid];

  // treat deleted nodes as: no process fn AND no ports
  if (node->vtable.process == NULL && node->nInputs == 0 && node->nOutputs == 0)
    return;
  if (lg->sched.is_orphaned[nid]) // Node is orphaned
    return;
  if (node->nInputs < 0 || node->nOutputs < 0) // Invalid port counts
    return;

  // Set thread-local context for SUM nodes to access input count
  g_current_processing_node = node;

  // === Use pre-cached IO pointers ===
  // Rebuild lazily if cache is invalid (topology changed). This is the
  // per-node guarantee; process_next_block also does an eager full-graph
  // pass, but we keep the lazy check here so any entry point (tests,
  // direct process_live_block calls, etc.) stays correct.
  if (!node->io_cache_valid) {
    rebuild_node_io_cache(lg, node, nframes);
  }

  // Pass the cached pointer arrays straight to the kernel — same contract
  // the kernels were written against pre-e44d655. No stack copy, no live
  // output re-resolution: the kernel sees exactly what the cache says.
  float **inPtrs = node->cached_inPtrs;
  float **outPtrs = node->cached_outPtrs;

  // Fallback to silence/scratch if no cached pointers (shouldn't happen)
  if (!inPtrs && node->nInputs > 0) {
    inPtrs = &lg->silence_buf; // Single pointer fallback
  }
  if (!outPtrs && node->nOutputs > 0) {
    outPtrs = &lg->scratch_null;
  }

  if (node->vtable.process) {
    node->vtable.process((float *const *)inPtrs, (float *const *)outPtrs,
                         nframes, node->state, lg->buffers);
  }

  // Clear thread-local context
  g_current_processing_node = NULL;
}


static inline void execute_and_fanout(LiveGraph *lg, int32_t nid, int nframes) {
  if (nid < 0 || nid >= lg->node_count) {
    fprintf(stderr,
            "[audiograph] WARN: execute_and_fanout skipping invalid nid=%d "
            "(count=%d)\n",
            nid, lg->node_count);
    return;
  }
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
  atomic_store_explicit(&g_inflight_node_ids[g_current_execution_slot], nid,
                        memory_order_release);
#endif
  bind_and_run_live(lg, nid, nframes); // uses silence/scratch for missing ports

  RTNode *node = &lg->nodes[nid];

  // OPTIMIZATION: Check if we have any successors before the loop
  // This avoids cache misses on is_orphaned for leaf nodes
  if (node->succCount > 0) {
    // Notify successors (node-level)
    // Use release semantics to ensure our output buffer writes are visible
    for (int i = 0; i < node->succCount; i++) {
      int succ = node->succ[i];
      if (succ < 0 || succ >= lg->node_count) {
        continue;
      }
      if (lg->sched.is_orphaned[succ]) {
        continue;
      }
      // Use release on decrement to ensure buffer writes are visible to successor
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
      int prev_pending =
          atomic_fetch_sub_explicit(&lg->sched.pending[succ], 1, memory_order_release);
      if (prev_pending == 1) {
        rq_push_or_spin(lg->sched.readyQueue, succ);
      } else if (prev_pending <= 0) {
        RTNode *succ_node = &lg->nodes[succ];
        fprintf(stderr,
                "[audiograph] WARN: pending underflow/duplicate fanout src=%d(%s) succ=%d(%s) prev_pending=%d indegree=%d\n",
                nid, node->debug_name ? node->debug_name : "<unnamed>", succ,
                succ_node->debug_name ? succ_node->debug_name : "<unnamed>",
                prev_pending, lg->sched.indegree[succ]);
      }
#else
      if (atomic_fetch_sub_explicit(&lg->sched.pending[succ], 1,
                                    memory_order_release) == 1) {
        rq_push_or_spin(lg->sched.readyQueue, succ);
      }
#endif
    }
  }

#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
  // Log completion and decrement counters BEFORE marking node as done.
  // This ensures that if a stall diagnostic sees pending=-3, the matching
  // jobsInFlight-- has already happened. Previously, pending was set first,
  // which created a window where a node appeared "done" in diagnostics
  // without its jobsInFlight decrement being visible.
  uint64_t completion_seq =
      atomic_fetch_add_explicit(&g_completion_seq, 1, memory_order_acq_rel);
  int completion_slot = (int)(completion_seq % COMPLETION_LOG_CAPACITY);
  atomic_store_explicit(&g_completion_log_nodes[completion_slot], nid,
                        memory_order_release);
  atomic_store_explicit(&g_completion_log_seq[completion_slot], completion_seq + 1,
                        memory_order_release);
  lg->sched.completed_this_block[nid] = true;
  atomic_fetch_add_explicit(&g_completed_jobs, 1, memory_order_release);
  atomic_fetch_sub_explicit(&lg->sched.jobsInFlight, 1, memory_order_release);
  // Mark done AFTER accounting — a node with pending=-3 is now guaranteed
  // to have been fully counted in completed_jobs and jobsInFlight.
  atomic_store_explicit(&lg->sched.pending[nid], PENDING_DONE_SENTINEL,
                        memory_order_release);
  atomic_store_explicit(&g_inflight_node_ids[g_current_execution_slot], -1,
                        memory_order_release);
#else
  // Publish this node's output writes before signaling global block completion.
  // The audio thread waits on jobsInFlight with acquire loads, so the final
  // transition to zero must carry release semantics.
  atomic_fetch_sub_explicit(&lg->sched.jobsInFlight, 1, memory_order_release);
#endif
}

// Check if a node has any connected outputs (for scheduling)
static inline bool node_has_any_output_connected(LiveGraph *lg, int node_id) {
  RTNode *node = &lg->nodes[node_id];
  if (!node->outEdgeId)
    return false;

  for (int i = 0; i < node->nOutputs; i++) {
    if (node->outEdgeId[i] >= 0)
      return true;
  }
  return false;
}

#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
static void dump_stalled_nodes(LiveGraph *lg) {
  if (!lg) {
    return;
  }

  int blocked_count = 0;
  int ready_count = 0;
  int running_count = 0;
  int done_count = 0;
  int orphaned_count = 0;
  int deleted_count = 0;

  for (int i = 0; i < lg->node_count; i++) {
    RTNode *node = &lg->nodes[i];
    bool deleted = (node->vtable.process == NULL && node->nInputs == 0 &&
                    node->nOutputs == 0);
    bool orphaned = lg->sched.is_orphaned[i];
    int pending = atomic_load_explicit(&lg->sched.pending[i], memory_order_acquire);

    if (deleted) {
      deleted_count++;
      continue;
    }
    if (orphaned) {
      orphaned_count++;
      continue;
    }

    if (pending > 0) {
      blocked_count++;
    } else if (pending == 0) {
      ready_count++;
    } else if (pending == PENDING_RUNNING_SENTINEL) {
      running_count++;
    } else if (pending == PENDING_DONE_SENTINEL) {
      done_count++;
    }
  }

  fprintf(stderr,
          "[audiograph] stalled-node summary blocked=%d ready=%d running=%d done=%d orphaned=%d deleted=%d\n",
          blocked_count, ready_count, running_count, done_count, orphaned_count,
          deleted_count);
  fprintf(stderr, "[audiograph] stalled-node dump begin\n");
  for (int i = 0; i < lg->node_count; i++) {
    RTNode *node = &lg->nodes[i];
    bool deleted = (node->vtable.process == NULL && node->nInputs == 0 &&
                    node->nOutputs == 0);
    int pending = atomic_load_explicit(&lg->sched.pending[i], memory_order_acquire);
    int indegree = lg->sched.indegree[i];
    bool orphaned = lg->sched.is_orphaned[i];
    bool has_out = node_has_any_output_connected(lg, i);

    bool is_active_negative = !orphaned && !deleted &&
                              (pending == PENDING_RUNNING_SENTINEL ||
                               pending == PENDING_DONE_SENTINEL);
    if (pending < 0 && !orphaned && !deleted && !is_active_negative) {
      continue;
    }

    fprintf(stderr,
            "[audiograph] stalled-node id=%d logical=%llu pending=%d indegree=%d orphaned=%d deleted=%d succCount=%d nIn=%d nOut=%d hasOut=%d name=%s\n",
            i, (unsigned long long)node->logical_id, pending, indegree,
            orphaned ? 1 : 0, deleted ? 1 : 0, node->succCount, node->nInputs,
            node->nOutputs, has_out ? 1 : 0,
            node->debug_name ? node->debug_name : "<unnamed>");
  }
  fprintf(stderr, "[audiograph] stalled-node dump end\n");

  // Identify "ghost" nodes: pending=-3 but never went through completion this block.
  // This is the primary diagnostic for the 1-off jobsInFlight stall.
  int ghost_count = 0;
  for (int i = 0; i < lg->node_count; i++) {
    RTNode *node = &lg->nodes[i];
    int pending = atomic_load_explicit(&lg->sched.pending[i], memory_order_acquire);
    bool deleted = (node->vtable.process == NULL && node->nInputs == 0 &&
                    node->nOutputs == 0);
    bool orphaned = lg->sched.is_orphaned[i];
    if (deleted || orphaned) continue;
    if (pending == PENDING_DONE_SENTINEL && !lg->sched.completed_this_block[i]) {
      fprintf(stderr,
              "[audiograph] GHOST NODE id=%d logical=%llu indegree=%d succCount=%d nIn=%d nOut=%d name=%s\n",
              i, (unsigned long long)node->logical_id, lg->sched.indegree[i],
              node->succCount, node->nInputs, node->nOutputs,
              node->debug_name ? node->debug_name : "<unnamed>");
      ghost_count++;
    }
  }
  if (ghost_count > 0) {
    fprintf(stderr, "[audiograph] ghost node count=%d (pending=-3 but not completed this block)\n", ghost_count);
  }
}

typedef struct {
  int total_jobs;
  int source_count;
} SchedulingCounts;

static SchedulingCounts recount_scheduling_counts(LiveGraph *lg, bool dump_nodes,
                                                  const char *reason) {
  SchedulingCounts counts = {0, 0};
  if (!lg) {
    return counts;
  }

  if (dump_nodes) {
    fprintf(stderr, "[audiograph] scheduling recount begin reason=%s\n",
            reason ? reason : "<none>");
  }

  for (int i = 0; i < lg->node_count; i++) {
    RTNode *node = &lg->nodes[i];
    bool deleted = (node->vtable.process == NULL && node->nInputs == 0 &&
                    node->nOutputs == 0);
    bool orphaned = lg->sched.is_orphaned[i];
    bool has_out = node_has_any_output_connected(lg, i);
    bool is_sink = !has_out && lg->sched.indegree[i] > 0;

    if (deleted || orphaned) {
      continue;
    }

    if (has_out || is_sink) {
      counts.total_jobs++;
      if (dump_nodes) {
        fprintf(stderr,
                "[audiograph] recount-job id=%d logical=%llu indegree=%d hasOut=%d isSink=%d succCount=%d nIn=%d nOut=%d name=%s\n",
                i, (unsigned long long)node->logical_id, lg->sched.indegree[i],
                has_out ? 1 : 0, is_sink ? 1 : 0, node->succCount, node->nInputs,
                node->nOutputs, node->debug_name ? node->debug_name : "<unnamed>");
      }
      if (lg->sched.indegree[i] == 0 && has_out) {
        counts.source_count++;
      }
    }
  }

  if (dump_nodes) {
    fprintf(stderr,
            "[audiograph] scheduling recount end reason=%s total_jobs=%d source_count=%d cached_total_jobs=%d cached_source_count=%d\n",
            reason ? reason : "<none>", counts.total_jobs, counts.source_count,
            lg->sched.cached_total_jobs, lg->sched.source_count);
  }

  return counts;
}

static void dump_cached_source_nodes(LiveGraph *lg) {
  if (!lg) {
    return;
  }
  fprintf(stderr, "[audiograph] cached source nodes begin\n");
  for (int idx = 0; idx < lg->sched.source_count; idx++) {
    int nid = lg->sched.source_nodes[idx];
    if (nid < 0 || nid >= lg->node_count) {
      fprintf(stderr, "[audiograph] cached-source idx=%d nid=%d INVALID\n", idx, nid);
      continue;
    }
    RTNode *node = &lg->nodes[nid];
    fprintf(stderr,
            "[audiograph] cached-source idx=%d nid=%d logical=%llu indegree=%d succCount=%d nIn=%d nOut=%d name=%s\n",
            idx, nid, (unsigned long long)node->logical_id, lg->sched.indegree[nid],
            node->succCount, node->nInputs, node->nOutputs,
            node->debug_name ? node->debug_name : "<unnamed>");
  }
  fprintf(stderr, "[audiograph] cached source nodes end\n");
}

static void dump_ready_queue_state(LiveGraph *lg) {
  if (!lg || !lg->sched.readyQueue || !lg->sched.readyQueue->ring) {
    return;
  }
  ReadyQ *q = lg->sched.readyQueue;
  MPMCQueue *ring = q->ring;
  int qlen = atomic_load_explicit(&q->qlen, memory_order_acquire);
  int waiters = atomic_load_explicit(&q->waiters, memory_order_acquire);
  uint64_t head = atomic_load_explicit(&ring->head, memory_order_acquire);
  uint64_t tail = atomic_load_explicit(&ring->tail, memory_order_acquire);
  fprintf(stderr,
          "[audiograph] readyq state qlen=%d waiters=%d head=%llu tail=%llu mask=%u approx_ring_count=%llu\n",
          qlen, waiters, (unsigned long long)head, (unsigned long long)tail,
          ring->mask, (unsigned long long)(head - tail));
}
#endif

// ===================== OPTIMIZATION: Scheduling Cache =====================
// Instead of O(n) scans every block, we cache source nodes and job counts.
// The cache is rebuilt only when topology changes (scheduling_dirty flag).

static void rebuild_scheduling_cache(LiveGraph *lg) {
  int totalJobs = 0;
  int sourceCount = 0;

  // Count sources first to check capacity
  for (int i = 0; i < lg->node_count; i++) {
    bool deleted = (lg->nodes[i].vtable.process == NULL &&
                    lg->nodes[i].nInputs == 0 && lg->nodes[i].nOutputs == 0);
    if (deleted || lg->sched.is_orphaned[i])
      continue;

    bool hasOut = node_has_any_output_connected(lg, i);
    bool isSink = !hasOut && lg->sched.indegree[i] > 0;

    if (hasOut || isSink) {
      totalJobs++;
      if (lg->sched.indegree[i] == 0 && hasOut) {
        sourceCount++;
      }
    }
  }

  // Grow source_nodes array if needed
  if (sourceCount > lg->sched.source_capacity) {
    int new_cap = lg->sched.source_capacity;
    while (new_cap < sourceCount)
      new_cap *= 2;
    int32_t *new_sources = realloc(lg->sched.source_nodes, new_cap * sizeof(int32_t));
    if (new_sources) {
      lg->sched.source_nodes = new_sources;
      lg->sched.source_capacity = new_cap;
    }
  }

  // Build source list
  lg->sched.source_count = 0;
  for (int i = 0; i < lg->node_count; i++) {
    bool deleted = (lg->nodes[i].vtable.process == NULL &&
                    lg->nodes[i].nInputs == 0 && lg->nodes[i].nOutputs == 0);
    if (deleted || lg->sched.is_orphaned[i])
      continue;

    if (lg->sched.indegree[i] == 0 && node_has_any_output_connected(lg, i)) {
      if (lg->sched.source_count < lg->sched.source_capacity) {
        lg->sched.source_nodes[lg->sched.source_count++] = i;
      }
    }
  }

  // Detect cycles at topology-change time (not every block!)
  lg->sched.has_cycle = (totalJobs > 0 && lg->sched.source_count == 0);

  lg->sched.cached_total_jobs = totalJobs;
  lg->sched.dirty = false;
}

static int choose_active_worker_count(LiveGraph *lg) {
  int max_workers = g_engine.workerCount;
  if (max_workers <= 0 || !lg)
    return 0;

  int jobs = lg->sched.cached_total_jobs;
  int sources = lg->sched.source_count;

  // Worker wake/sync overhead dominates tiny and mostly-serial graphs.
  if (jobs < 24)
    return 0;

  // Scale up conservatively with available work. Initial source width is only a
  // soft cap: many useful audio graphs start at one source and fan out later.
  int active = jobs / 24; // 24..47 jobs => 1 worker, 48..71 => 2, ...
  if (sources >= 2 && active > sources)
    active = sources;
  if (active < 1)
    active = 1;
  if (active > max_workers)
    active = max_workers;
  return active;
}

static void init_pending_and_seed(LiveGraph *lg) {
  // Rebuild cache if topology changed
  if (lg->sched.dirty) {
    rebuild_scheduling_cache(lg);
  }

  // CRITICAL FIX: Properly reset/drain the ready queue to prevent stale node
  // IDs. Only drain if there might be stale items.
  int32_t dummy;
  while (rq_try_pop(lg->sched.readyQueue, &dummy)) {
    // Discard any stale items
  }

  // Reset pending counts to indegree for all active nodes
  // This is O(n) but uses relaxed stores which are fast
  // Workers will use atomic decrements on these values
  for (int i = 0; i < lg->node_count; i++) {
    bool deleted = (lg->nodes[i].vtable.process == NULL &&
                    lg->nodes[i].nInputs == 0 && lg->nodes[i].nOutputs == 0);
    if (deleted || lg->sched.is_orphaned[i]) {
      atomic_store_explicit(&lg->sched.pending[i], -1, memory_order_relaxed);
    } else {
      atomic_store_explicit(&lg->sched.pending[i], lg->sched.indegree[i], memory_order_relaxed);
    }
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
    lg->sched.completed_this_block[i] = false;
#endif
  }

  // Memory barrier to ensure all pending stores are visible before workers start
  atomic_thread_fence(memory_order_release);

  // Seed ready queue from cached source list - O(sources) instead of O(n)
  // Use batch push to reduce semaphore signals from O(sources) to O(1)
  rq_push_batch(lg->sched.readyQueue, lg->sched.source_nodes, lg->sched.source_count);

#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
  atomic_store_explicit(&g_completed_jobs, 0, memory_order_release);
  atomic_store_explicit(&g_completion_seq, 0, memory_order_release);
  for (int i = 0; i < COMPLETION_LOG_CAPACITY; i++) {
    atomic_store_explicit(&g_completion_log_nodes[i], -1, memory_order_relaxed);
    atomic_store_explicit(&g_completion_log_seq[i], 0, memory_order_relaxed);
  }
#endif
  atomic_store_explicit(&lg->sched.jobsInFlight, lg->sched.cached_total_jobs,
                        memory_order_release);
}

bool detect_cycle(LiveGraph *lg) {
  // Use cached result if available
  if (!lg->sched.dirty) {
    return lg->sched.has_cycle;
  }
  // Fallback to full computation (shouldn't happen in hot path)
  int reachable = 0, zero_in = 0;
  for (int i = 0; i < lg->node_count; i++) {
    if (atomic_load_explicit(&lg->sched.pending[i], memory_order_relaxed) < 0)
      continue; // orphan/deleted
    reachable++;
    if (lg->sched.indegree[i] == 0 && node_has_any_output_connected(lg, i))
      zero_in++;
  }
  return (reachable > 0 && zero_in == 0);
}

// Call at the end of process_live_block (after all work done)
static void drain_retire_list(LiveGraph *lg) {
  for (int i = 0; i < lg->retire.count; i++) {
    lg->retire.list[i].deleter(lg->retire.list[i].ptr);
  }
  lg->retire.count = 0;
}

static void update_watched_node_states(LiveGraph *lg);

static void process_live_block_internal(LiveGraph *lg, int nframes, bool update_watch) {
  // Initialize pending counts and seed ready queue
  init_pending_and_seed(lg);

  // Check for cycles that would cause silent deadlocks
  if (detect_cycle(lg)) {
    // Clear output buffer to silence
    if (lg->dac_node_id >= 0 && lg->nodes[lg->dac_node_id].inEdgeId) {
      int master_edge_id = lg->nodes[lg->dac_node_id].inEdgeId[0];
      if (master_edge_id >= 0 && master_edge_id < lg->edge_capacity &&
          lg->edges[master_edge_id].buf != NULL) {
        memset(lg->edges[master_edge_id].buf, 0, nframes * sizeof(float));
      }
    }
    if (update_watch)
      update_watched_node_states(lg);
    return;
  }

  // check if no work to be done
  if (atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire) <= 0) {
    if (update_watch)
      update_watched_node_states(lg);
    return;
  }

  int active_workers = choose_active_worker_count(lg);
  atomic_store_explicit(&g_engine.activeWorkerLimit, active_workers,
                        memory_order_release);

  if (active_workers > 0) {
    // Publish session frames and graph
    atomic_store_explicit(&g_engine.sessionFrames, nframes,
                          memory_order_release);
    atomic_store_explicit(&g_engine.workSession, lg, memory_order_release);

    // wake workers
    pthread_mutex_lock(&g_engine.sess_mtx);
    pthread_cond_broadcast(&g_engine.sess_cv);
    pthread_mutex_unlock(&g_engine.sess_mtx);

    // Audio thread helps do some work
    int32_t nid;
    int empty_spins = 0;
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
    bool stall_logged = false;
    struct timespec wait_started;
    clock_gettime(CLOCK_MONOTONIC, &wait_started);
#endif
    while (atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire) > 0) {
      if (rq_try_pop(lg->sched.readyQueue, &nid)) {
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
        if (!claim_ready_node(lg, nid)) {
          continue;
        }
#endif
        execute_and_fanout(lg, nid, nframes);
        empty_spins = 0; // Reset on successful work
      } else {
        // Queue empty but work in flight - workers processing
        // Check again if work completed (avoids unnecessary spins)
        if (atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire) == 0)
          break;
        cpu_relax();
        // This is the realtime callback thread; keep it runnable and poll
        // lightly until workers publish more ready jobs or finish.
        if (++empty_spins > 4096) {
          empty_spins = 0;
        }
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
        if (!stall_logged) {
          struct timespec now;
          clock_gettime(CLOCK_MONOTONIC, &now);
          long waited_ms =
              (now.tv_sec - wait_started.tv_sec) * 1000L +
              (now.tv_nsec - wait_started.tv_nsec) / 1000000L;
          if (waited_ms >= 10) {
            int jobs = atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire);
            int qlen = atomic_load_explicit(&lg->sched.readyQueue->qlen, memory_order_acquire);
            SchedulingCounts recounted =
                recount_scheduling_counts(lg, /*dump_nodes=*/true, "stall");
            fprintf(stderr,
                    "[audiograph] process_live_block stall: waited=%ldms jobsInFlight=%d readyQ=%d node_count=%d source_count=%d cached_total_jobs=%d recounted_total_jobs=%d recounted_source_count=%d dirty=%d\n",
                    waited_ms, jobs, qlen, lg->node_count, lg->sched.source_count,
                    lg->sched.cached_total_jobs, recounted.total_jobs,
                    recounted.source_count, lg->sched.dirty ? 1 : 0);
            dump_ready_queue_state(lg);
            dump_cached_source_nodes(lg);
            dump_stalled_nodes(lg);
            dump_inflight_nodes(lg);
            dump_completion_log(lg);
            stall_logged = true;

            // STALL RECOVERY: Force jobsInFlight to 0 so the audio callback
            // can return and the next block can start fresh. Without this,
            // a single accounting bug causes permanent audio death.
            fprintf(stderr,
                    "[audiograph] STALL RECOVERY: forcing jobsInFlight from %d to 0\n",
                    jobs);
            atomic_store_explicit(&lg->sched.jobsInFlight, 0, memory_order_release);
          }
        }
#endif
      }
    }

    // Paired with the release decrement in execute_and_fanout so downstream
    // consumers (DAC copy, watch snapshots, retire list) see completed node
    // writes from all worker threads.
    atomic_thread_fence(memory_order_acquire);

    // Clear session
    atomic_store_explicit(&g_engine.workSession, NULL, memory_order_release);
  } else {
    // Single-thread fallback
    int32_t nid;
    while (rq_try_pop(lg->sched.readyQueue, &nid)) {
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
      if (!claim_ready_node(lg, nid)) {
        continue;
      }
#endif
      execute_and_fanout(lg, nid, nframes);
    }
  }

  drain_retire_list(lg);

  if (update_watch)
    update_watched_node_states(lg);
}

void process_live_block(LiveGraph *lg, int nframes) {
  process_live_block_internal(lg, nframes, true);
}

int find_live_output(LiveGraph *lg) {
  return lg->dac_node_id; // Simply return the DAC node - no searching needed
}

// ===================== Live Engine Implementation =====================
void process_next_block(LiveGraph *lg, float *output_buffer, int nframes) {
  if (!lg || !output_buffer || nframes <= 0) {
    // Clear output buffer if invalid input
    if (output_buffer && nframes > 0) {
      memset(output_buffer, 0,
             (size_t)nframes * (size_t)lg->num_channels * sizeof(float));
    }
    return;
  }

  if (atomic_load_explicit(&lg->edit_batch_depth, memory_order_acquire) == 0) {
    apply_graph_edits(lg->graphEditQueue, lg);
    if (lg->sched.dirty) {
      rebuild_invalid_io_caches(lg, lg->block_size);
    }
  }

  apply_params(lg);

  // Process in slices if callback frames exceed internal block size.
  int remaining = nframes;
  int out_offset = 0; // in frames
  while (remaining > 0) {
    int slice = remaining;
    if (slice > lg->block_size)
      slice = lg->block_size;

    process_live_block_internal(lg, slice, false);

    // Get the DAC node (final output)
    int output_node = find_live_output(lg);

    if (output_node >= 0 && lg->nodes[output_node].nInputs > 0) {
      RTNode *dac = &lg->nodes[output_node];

      // Copy each channel from DAC inputs to interleaved output buffer
      for (int ch = 0; ch < lg->num_channels; ch++) {
        float *src = NULL;

        // Get the input edge for this channel
        if (ch < dac->nInputs) {
          int edge_id = dac->inEdgeId[ch];
          if (edge_id >= 0 && edge_id < lg->edge_capacity) {
            src = lg->edges[edge_id].buf;
          }
        }

        // Interleave this channel into the output buffer with offset
        float *dst = output_buffer +
                     ((size_t)out_offset * (size_t)lg->num_channels) + ch;
        if (src) {
          for (int i = 0; i < slice; i++) {
            dst[i * lg->num_channels] = src[i];
          }
        } else {
          for (int i = 0; i < slice; i++) {
            dst[i * lg->num_channels] = 0.0f;
          }
        }
      }
    } else {
      // No output node - silence for this slice
      for (int i = 0; i < slice; i++) {
        for (int ch = 0; ch < lg->num_channels; ch++) {
          output_buffer[((size_t)out_offset + i) * (size_t)lg->num_channels +
                        ch] = 0.0f;
        }
      }
    }

    remaining -= slice;
    out_offset += slice;
  }

  // Throttle watchlist snapshots in the audio render path. A full snapshot can
  // be expensive when many UI/polling operators are watched, and most consumers
  // don't need audio-block-rate state updates.
#if AUDIOGRAPH_WATCH_UPDATE_INTERVAL <= 1
  update_watched_node_states(lg);
#else
  uint32_t watch_tick = atomic_fetch_add_explicit(&g_watch_update_counter, 1,
                                                  memory_order_relaxed);
  if ((watch_tick % AUDIOGRAPH_WATCH_UPDATE_INTERVAL) == 0) {
    update_watched_node_states(lg);
  }
#endif
}

static void update_watched_node_states(LiveGraph *lg) {
  if (!lg || lg->watch.count == 0) {
    return;
  }

  // This can be called from process_next_block(), so never block the audio
  // thread behind UI watchlist readers/writers and avoid per-callback mallocs.
  // Loaded sequencer projects can watch one pan meter plus many sampler voices
  // per track, so this must be comfortably above the common project size.
  enum { WATCH_STACK_CAP = 2048 };
  int watch_nodes_stack[WATCH_STACK_CAP];

  if (pthread_mutex_trylock(&lg->watch.mutex) != 0) {
    return;
  }

  int watch_count = lg->watch.count;
  if (watch_count > WATCH_STACK_CAP) {
    watch_count = WATCH_STACK_CAP;
  }
  if (watch_count <= 0) {
    pthread_mutex_unlock(&lg->watch.mutex);
    return;
  }

  memcpy(watch_nodes_stack, lg->watch.list, watch_count * sizeof(int));
  pthread_mutex_unlock(&lg->watch.mutex);

  if (pthread_rwlock_trywrlock(&lg->watch.lock) != 0) {
    return;
  }

  for (int i = 0; i < watch_count; i++) {
    int node_id = watch_nodes_stack[i];

    // Validate node_id and check if node exists
    if (node_id < 0 || node_id >= lg->node_count) {
      continue;
    }

    RTNode *node = &lg->nodes[node_id];
    if (!node->state || node->state_size == 0) {
      continue; // No state to copy
    }

    // Reuse existing snapshot buffer if size matches. Allocation only happens
    // when a watch is first added or a hot-swap changes state size.
    if (lg->watch.snapshots[node_id] &&
        lg->watch.sizes[node_id] == node->state_size) {
      memcpy(lg->watch.snapshots[node_id], node->state, node->state_size);
    } else {
      if (lg->watch.snapshots[node_id]) {
        free(lg->watch.snapshots[node_id]);
        lg->watch.snapshots[node_id] = NULL;
        lg->watch.sizes[node_id] = 0;
      }
      void *snapshot = malloc(node->state_size);
      if (snapshot) {
        memcpy(snapshot, node->state, node->state_size);
        lg->watch.snapshots[node_id] = snapshot;
        lg->watch.sizes[node_id] = node->state_size;
      }
    }
  }

  pthread_rwlock_unlock(&lg->watch.lock);
}
