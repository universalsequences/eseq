#include "graph_engine.h"
#include "graph_edit.h"
#include "graph_nodes.h"
#include <assert.h>
#include <errno.h>
#include <math.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#ifdef __linux__
#include <sys/syscall.h>
#endif

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
static void init_pending_and_seed(LiveGraph *lg, int nframes);
void process_live_block(LiveGraph *lg, int nframes);
static inline void execute_and_fanout(LiveGraph *lg, int32_t nid, int nframes);
static inline bool try_execute_ready_node(LiveGraph *lg, int32_t nid,
                                          int nframes);
static inline void schedule_ready_node(LiveGraph *lg, int32_t nid, int nframes);
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
_Atomic uint64_t g_param_push_count = 0;
_Atomic uint64_t g_param_push_fail_count = 0;
_Atomic uint64_t g_block_event_push_count = 0;
_Atomic uint64_t g_block_event_push_fail_count = 0;

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

#ifndef AUDIOGRAPH_STALL_RECOVERY_TIMEOUT_NS
#define AUDIOGRAPH_STALL_RECOVERY_TIMEOUT_NS 50000000ull
#endif

static _Atomic int g_active_job_count = 0;
#ifdef __linux__
static EngineRtKitWorkerHook g_rtkit_worker_hook = NULL;
static EngineRtKitCallbackHook g_rtkit_callback_hook = NULL;
static _Atomic int g_rt_permission_warning_logged = 0;
#endif
static _Atomic uint64_t g_stall_recovery_count = 0;
static _Atomic uint64_t g_graph_trace_block_counter = 0;
static _Atomic uint32_t g_graph_trace_silent_streak = 0;
static _Atomic uint64_t g_param_apply_count = 0;
static _Atomic uint64_t g_param_drop_oob_count = 0;
static _Atomic uint64_t g_param_drop_nonfinite_count = 0;
static _Atomic uint64_t g_block_event_apply_count = 0;
static _Atomic uint64_t g_block_event_drop_stale_count = 0;
static _Atomic uint64_t g_block_event_drop_unsupported_count = 0;
static _Atomic uint64_t g_block_event_drop_invalid_count = 0;
static _Atomic uint64_t g_block_event_schedule_reject_count = 0;

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
  atomic_store_explicit(&g_engine.rt_scheduling, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.rt_priority, 20, memory_order_relaxed);
  g_engine.workerPolicies = NULL;
  g_engine.workerPriorities = NULL;
  atomic_store_explicit(&g_engine.workerStartupCount, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackPolicy, ENGINE_SCHED_POLICY_UNKNOWN,
                        memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackPriority, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackReported, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.graph_log, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.activeWorkerLimit, 0, memory_order_relaxed);
  atomic_store_explicit(&g_active_job_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_stall_recovery_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_graph_trace_block_counter, 0, memory_order_relaxed);
  atomic_store_explicit(&g_graph_trace_silent_streak, 0, memory_order_relaxed);
  atomic_store_explicit(&g_param_push_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_param_push_fail_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_block_event_push_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_block_event_push_fail_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_param_apply_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_param_drop_oob_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_param_drop_nonfinite_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_block_event_apply_count, 0, memory_order_relaxed);
  atomic_store_explicit(&g_block_event_drop_stale_count, 0,
                        memory_order_relaxed);
  atomic_store_explicit(&g_block_event_drop_unsupported_count, 0,
                        memory_order_relaxed);
  atomic_store_explicit(&g_block_event_drop_invalid_count, 0,
                        memory_order_relaxed);
  atomic_store_explicit(&g_block_event_schedule_reject_count, 0,
                        memory_order_relaxed);
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

#define DGEN_HEADER_SLOTS 5
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
  uint64_t applied = 0;
  while (params_pop(g->params, &m)) {
    // O(1) direct lookup: logical_id is used as the array index in apply_add_node
    int node_id = (int)m.logical_id;
    if (node_id >= 0 && node_id < g->node_count) {
      RTNode *node = &g->nodes[node_id];
      // Verify logical_id matches (safety check for deleted/reused slots)
      if (node->state && node->logical_id == m.logical_id) {
        float *memory = (float *)node->state;
        int state_slots = (int)(node->state_size / sizeof(float));
        if (m.idx >= (uint64_t)state_slots) {
          uint64_t dropped = atomic_fetch_add_explicit(
                                 &g_param_drop_oob_count, 1, memory_order_acq_rel) +
                             1;
          if (dropped <= 16 || (dropped % 128u) == 0) {
            fprintf(stderr,
                    "[audiograph] dropped out-of-range param logical=%llu idx=%llu state_slots=%d drops=%llu\n",
                    (unsigned long long)m.logical_id, (unsigned long long)m.idx,
                    state_slots, (unsigned long long)dropped);
          }
          continue;
        }
        if (!isfinite(m.fvalue)) {
          uint64_t dropped = atomic_fetch_add_explicit(
                                 &g_param_drop_nonfinite_count, 1,
                                 memory_order_acq_rel) +
                             1;
          if (dropped <= 16 || (dropped % 128u) == 0) {
            fprintf(stderr,
                    "[audiograph] dropped non-finite param logical=%llu idx=%llu value=%f drops=%llu\n",
                    (unsigned long long)m.logical_id, (unsigned long long)m.idx,
                    m.fvalue, (unsigned long long)dropped);
          }
          continue;
        }
        memory[m.idx] = m.fvalue;
        applied++;
        if (m.idx >= DGEN_HEADER_SLOTS && has_dgen_header_canary(memory)) {
          int total_slots = (int)memory[1];
          if (total_slots > 0) {
            int write_base = DGEN_HEADER_SLOTS + total_slots + DGEN_STATE_REDZONE_SLOTS;
            int dgen_idx = (int)(m.idx - DGEN_HEADER_SLOTS);
            int mirrored_idx = write_base + dgen_idx;
            if (dgen_idx >= 0 && dgen_idx < total_slots &&
                mirrored_idx >= 0 && mirrored_idx < state_slots) {
              memory[mirrored_idx] = m.fvalue;
            }
          }
        }
      }
    }
  }
  if (applied > 0) {
    atomic_fetch_add_explicit(&g_param_apply_count, applied, memory_order_acq_rel);
  }
}

static inline bool graph_block_event_less_or_equal(const GraphBlockEvent *a,
                                                   const GraphBlockEvent *b) {
  if (a->frame_offset != b->frame_offset)
    return a->frame_offset < b->frame_offset;
  return a->sequence <= b->sequence;
}

static void stable_sort_block_events(GraphBlockEvent *events,
                                     GraphBlockEvent *scratch, int count) {
  if (!events || !scratch || count <= 1)
    return;

  GraphBlockEvent *src = events;
  GraphBlockEvent *dst = scratch;
  bool src_is_events = true;
  for (int width = 1; width < count; width *= 2) {
    for (int left = 0; left < count; left += width * 2) {
      int mid = left + width;
      int right = left + width * 2;
      if (mid > count)
        mid = count;
      if (right > count)
        right = count;

      int i = left;
      int j = mid;
      int k = left;
      while (i < mid && j < right) {
        if (graph_block_event_less_or_equal(&src[i], &src[j])) {
          dst[k++] = src[i++];
        } else {
          dst[k++] = src[j++];
        }
      }
      while (i < mid)
        dst[k++] = src[i++];
      while (j < right)
        dst[k++] = src[j++];
    }

    GraphBlockEvent *tmp = src;
    src = dst;
    dst = tmp;
    src_is_events = !src_is_events;
  }

  if (!src_is_events) {
    memcpy(events, src, (size_t)count * sizeof(GraphBlockEvent));
  }
}

static bool block_event_target_is_valid(LiveGraph *lg,
                                        const GraphBlockEvent *event) {
  int node_id = (int)event->logical_id;
  if (node_id < 0 || node_id >= lg->node_count)
    return false;
  RTNode *node = &lg->nodes[node_id];
  if (!node->state || node->logical_id != event->logical_id)
    return false;
  return true;
}

static int drain_block_events_for_callback(LiveGraph *lg, int nframes) {
  if (!lg || !lg->block_events || !lg->block_event_scratch ||
      !lg->block_event_sort_scratch) {
    return 0;
  }

  int count = 0;
  GraphBlockEvent event;
  while (block_events_pop(lg->block_events, &event)) {
    if (event.aux_count > GBE_AUX_CAP)
      event.aux_count = GBE_AUX_CAP;
    if (event.frame_offset >= (uint32_t)nframes) {
      uint64_t dropped = atomic_fetch_add_explicit(
                             &g_block_event_drop_invalid_count, 1,
                             memory_order_acq_rel) +
                         1;
      if (dropped <= 16 || (dropped % 128u) == 0) {
        fprintf(stderr,
                "[audiograph] dropped out-of-callback block event logical=%llu frame=%u nframes=%d drops=%llu\n",
                (unsigned long long)event.logical_id, event.frame_offset,
                nframes, (unsigned long long)dropped);
      }
      continue;
    }
    if (!block_event_target_is_valid(lg, &event)) {
      uint64_t dropped = atomic_fetch_add_explicit(
                             &g_block_event_drop_stale_count, 1,
                             memory_order_acq_rel) +
                         1;
      if (dropped <= 16 || (dropped % 128u) == 0) {
        fprintf(stderr,
                "[audiograph] dropped stale block event logical=%llu frame=%u kind=%u drops=%llu\n",
                (unsigned long long)event.logical_id, event.frame_offset,
                event.kind, (unsigned long long)dropped);
      }
      continue;
    }

    RTNode *node = &lg->nodes[(int)event.logical_id];
    if (!node->vtable.schedule_event) {
      uint64_t dropped = atomic_fetch_add_explicit(
                             &g_block_event_drop_unsupported_count, 1,
                             memory_order_acq_rel) +
                         1;
      if (dropped <= 16 || (dropped % 128u) == 0) {
        fprintf(stderr,
                "[audiograph] dropped unsupported block event logical=%llu frame=%u kind=%u drops=%llu\n",
                (unsigned long long)event.logical_id, event.frame_offset,
                event.kind, (unsigned long long)dropped);
      }
      continue;
    }

    if (count >= lg->block_event_scratch_capacity) {
      uint64_t dropped = atomic_fetch_add_explicit(
                             &g_block_event_drop_invalid_count, 1,
                             memory_order_acq_rel) +
                         1;
      if (dropped <= 16 || (dropped % 128u) == 0) {
        fprintf(stderr,
                "[audiograph] dropped block event scratch overflow logical=%llu frame=%u kind=%u drops=%llu\n",
                (unsigned long long)event.logical_id, event.frame_offset,
                event.kind, (unsigned long long)dropped);
      }
      continue;
    }

    lg->block_event_scratch[count++] = event;
  }

  stable_sort_block_events(lg->block_event_scratch, lg->block_event_sort_scratch,
                           count);
  lg->block_event_scratch_count = count;
  return count;
}

static void begin_event_slice_for_nodes(LiveGraph *lg, uint64_t block_serial,
                                        int slice_start,
                                        int slice_nframes) {
  for (int i = 0; i < lg->node_count; i++) {
    RTNode *node = &lg->nodes[i];
    bool deleted = (node->vtable.process == NULL && node->nInputs == 0 &&
                    node->nOutputs == 0);
    if (deleted || !node->state || !node->vtable.begin_event_slice)
      continue;
    if (lg->sched.is_orphaned && lg->sched.is_orphaned[i])
      continue;
    node->vtable.begin_event_slice(node->state, block_serial, slice_start,
                                   slice_nframes);
  }
}

static int deliver_block_events_for_slice(LiveGraph *lg, int start_index,
                                          int slice_start,
                                          int slice_nframes) {
  int slice_end = slice_start + slice_nframes;
  int index = start_index;
  while (index < lg->block_event_scratch_count &&
         (int)lg->block_event_scratch[index].frame_offset < slice_start) {
    index++;
  }

  while (index < lg->block_event_scratch_count) {
    GraphBlockEvent event = lg->block_event_scratch[index];
    if ((int)event.frame_offset >= slice_end)
      break;

    int node_id = (int)event.logical_id;
    if (node_id >= 0 && node_id < lg->node_count) {
      RTNode *node = &lg->nodes[node_id];
      if (node->state && node->logical_id == event.logical_id &&
          node->vtable.schedule_event) {
        event.frame_offset -= (uint32_t)slice_start;
        bool accepted = node->vtable.schedule_event(node->state, &event);
        if (accepted) {
          atomic_fetch_add_explicit(&g_block_event_apply_count, 1,
                                    memory_order_acq_rel);
        } else {
          uint64_t rejected = atomic_fetch_add_explicit(
                                  &g_block_event_schedule_reject_count, 1,
                                  memory_order_acq_rel) +
                              1;
          if (rejected <= 16 || (rejected % 128u) == 0) {
            fprintf(stderr,
                    "[audiograph] node rejected block event logical=%llu local_frame=%u kind=%u drops=%llu\n",
                    (unsigned long long)event.logical_id, event.frame_offset,
                    event.kind, (unsigned long long)rejected);
          }
        }
      }
    }
    index++;
  }
  return index;
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

#ifdef __linux__
// Shared SCHED_FIFO promotion for the graph workers and the audio callback
// thread that drives them. `priority_boost` is added to the configured base
// priority before clamping to the platform range: workers use 0, the callback
// thread uses +1 so the thread that publishes each block (and helps drain the
// graph in process_next_block) is never preempted by its own helpers.
// Returns true only when direct promotion was permission-denied and the
// caller should ask the Rust RealtimeKit helper for SCHED_RR instead.
static bool promote_current_linux_thread_to_fifo(int priority_boost,
                                                 const char *role,
                                                 int *requested_priority) {
  if (!atomic_load_explicit(&g_engine.rt_scheduling, memory_order_acquire)) {
    return false;
  }

  int min_priority = sched_get_priority_min(SCHED_FIFO);
  int max_priority = sched_get_priority_max(SCHED_FIFO);
  if (min_priority < 0 || max_priority < 0) {
    int err = errno;
    fprintf(stderr,
            "[audiograph] WARN: cannot query SCHED_FIFO priority range: %s "
            "(%s remains normal priority)\n",
            strerror(err), role);
    return false;
  }

  int requested =
      atomic_load_explicit(&g_engine.rt_priority, memory_order_acquire) +
      priority_boost;
  int priority = requested;
  if (priority < min_priority)
    priority = min_priority;
  if (priority > max_priority)
    priority = max_priority;

  struct sched_param policy = {.sched_priority = priority};
  *requested_priority = priority;
  int rc = pthread_setschedparam(pthread_self(), SCHED_FIFO, &policy);
  if (rc != 0) {
    if (rc == EPERM || rc == EACCES) {
      return true;
    } else {
      fprintf(stderr,
              "[audiograph] WARN: pthread_setschedparam(SCHED_FIFO, %d) "
              "failed: %s (%s remains normal priority)\n",
              priority, strerror(rc), role);
    }
    return false;
  }

  if (atomic_load_explicit(&g_engine.rt_log, memory_order_relaxed)) {
    fprintf(stderr,
            "[audiograph] %s %p set SCHED_FIFO priority %d%s\n",
            role, (void *)pthread_self(), priority,
            priority == requested ? "" : " (clamped to platform range)");
  }
  return false;
}

static void read_current_linux_scheduling(int *policy, int *priority,
                                          const char *role) {
  struct sched_param param = {.sched_priority = 0};
  int observed_policy = SCHED_OTHER;
  int rc = pthread_getschedparam(pthread_self(), &observed_policy, &param);
  if (rc != 0) {
    fprintf(stderr,
            "[audiograph] WARN: cannot read achieved scheduling for %s: %s\n",
            role, strerror(rc));
    *policy = ENGINE_SCHED_POLICY_UNKNOWN;
    *priority = 0;
    return;
  }
  *policy = observed_policy;
  *priority = param.sched_priority;
}

void engine_set_rtkit_hooks(EngineRtKitWorkerHook worker_hook,
                            EngineRtKitCallbackHook callback_hook) {
  // Registered by the control thread before workers or the callback can run.
  g_rtkit_worker_hook = worker_hook;
  g_rtkit_callback_hook = callback_hook;
}

static void warn_realtime_unavailable_without_host(void) {
  int expected = 0;
  if (atomic_compare_exchange_strong_explicit(
          &g_rt_permission_warning_logged, &expected, 1, memory_order_acq_rel,
          memory_order_relaxed)) {
    fprintf(stderr,
            "[audiograph] WARN: direct SCHED_FIFO promotion was denied and "
            "no host RealtimeKit fallback is registered; audio continues at "
            "normal priority\n");
  }
}

void engine_record_rtkit_callback_result(pid_t tid) {
  struct sched_param param = {.sched_priority = 0};
  int policy = sched_getscheduler(tid);
  if (policy < 0 || sched_getparam(tid, &param) != 0) {
    policy = ENGINE_SCHED_POLICY_UNKNOWN;
    param.sched_priority = 0;
  }
  atomic_store_explicit(&g_engine.callbackPolicy, policy, memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackPriority, param.sched_priority,
                        memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackReported, 1, memory_order_release);
}

static void promote_linux_worker_to_realtime(void) {
  int requested_priority = 0;
  if (promote_current_linux_thread_to_fifo(0, "worker", &requested_priority)) {
    pid_t tid = (pid_t)syscall(SYS_gettid);
    if (g_rtkit_worker_hook)
      (void)g_rtkit_worker_hook(tid, requested_priority);
    else
      warn_realtime_unavailable_without_host();
  }
}
#endif

void engine_promote_current_thread_rt(void) {
#ifdef __linux__
  // cpal's ALSA backend spawns the callback thread with default scheduling
  // and no thread-spawn hook, so the callback promotes itself on first entry.
  // Without this the workers run SCHED_FIFO while the thread that drives the
  // block stays SCHED_OTHER — priority inversion in the audio path.
  int requested_priority = 0;
  bool rtkit_needed = promote_current_linux_thread_to_fifo(
      1, "audio callback thread", &requested_priority);
  if (rtkit_needed) {
    pid_t tid = (pid_t)syscall(SYS_gettid);
    if (g_rtkit_callback_hook) {
      // Mark the result pending BEFORE handing the TID to the helper. This
      // thread is still SCHED_OTHER here, so it can be preempted between the
      // hook call and any later store for longer than the helper's poll plus
      // D-Bus round trip; a store after the hook could then clobber the
      // helper's recorded result into a permanently-pending status.
      atomic_store_explicit(&g_engine.callbackReported, 0,
                            memory_order_release);
      if (g_rtkit_callback_hook(tid, requested_priority))
        return;
    } else {
      warn_realtime_unavailable_without_host();
    }
  }
  int policy = ENGINE_SCHED_POLICY_UNKNOWN;
  int priority = 0;
  read_current_linux_scheduling(&policy, &priority, "audio callback thread");
  atomic_store_explicit(&g_engine.callbackPolicy, policy, memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackPriority, priority,
                        memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackReported, 1, memory_order_release);
#endif
  // On Apple this is deliberately a no-op: CoreAudio already delivers the
  // callback on a THREAD_TIME_CONSTRAINT_POLICY realtime thread.
}

static void *worker_main(void *arg) {
  intptr_t worker_slot = (intptr_t)arg;
  int worker_index = (int)worker_slot - 1;
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
  g_current_execution_slot = (int)worker_slot;
#endif
  // Elevate worker scheduling before it begins participating in graph work.
#ifdef __linux__
  promote_linux_worker_to_realtime();
  int policy = ENGINE_SCHED_POLICY_UNKNOWN;
  int priority = 0;
  read_current_linux_scheduling(&policy, &priority, "worker");
  atomic_store_explicit(&g_engine.workerPolicies[worker_index], policy,
                        memory_order_relaxed);
  atomic_store_explicit(&g_engine.workerPriorities[worker_index], priority,
                        memory_order_relaxed);
#endif
  // Starting workers is synchronous with respect to this initialization point,
  // so the host can truthfully report the achieved policy before continuing.
  pthread_mutex_lock(&g_engine.workerStartupMtx);
  atomic_fetch_add_explicit(&g_engine.workerStartupCount, 1,
                            memory_order_release);
  pthread_cond_broadcast(&g_engine.workerStartupCv);
  pthread_mutex_unlock(&g_engine.workerStartupMtx);
#ifdef __APPLE__
#ifdef QOS_CLASS_USER_INTERACTIVE
  (void)pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
#endif
#endif

#ifdef HAVE_MACH_RT
  // Optionally promote to Mach time-constraint scheduling
  if (atomic_load_explicit(&g_engine.rt_scheduling, memory_order_acquire)) {
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
      (void)try_execute_ready_node(lg, nid, nf);
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
  if (workers < 0)
    workers = 0;
  g_engine.workerCount = 0;
  g_engine.threads = (pthread_t *)calloc((size_t)workers, sizeof(pthread_t));
  g_engine.workerPolicies =
      (_Atomic int *)calloc((size_t)workers, sizeof(_Atomic int));
  g_engine.workerPriorities =
      (_Atomic int *)calloc((size_t)workers, sizeof(_Atomic int));
  if (workers > 0 && (!g_engine.threads || !g_engine.workerPolicies ||
                      !g_engine.workerPriorities)) {
    fprintf(stderr,
            "[audiograph] WARN: cannot allocate worker pool for %d workers\n",
            workers);
    free(g_engine.threads);
    free(g_engine.workerPolicies);
    free(g_engine.workerPriorities);
    g_engine.threads = NULL;
    g_engine.workerPolicies = NULL;
    g_engine.workerPriorities = NULL;
    workers = 0;
  }
  for (int i = 0; i < workers; i++) {
    atomic_init(&g_engine.workerPolicies[i], ENGINE_SCHED_POLICY_UNKNOWN);
    atomic_init(&g_engine.workerPriorities[i], 0);
  }
  atomic_store_explicit(&g_engine.workerStartupCount, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackPolicy, ENGINE_SCHED_POLICY_UNKNOWN,
                        memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackPriority, 0, memory_order_relaxed);
  atomic_store_explicit(&g_engine.callbackReported, 0, memory_order_relaxed);

  // Initialize mutexes and condition variables before any worker can signal.
  pthread_mutex_init(&g_engine.sess_mtx, NULL);
  pthread_cond_init(&g_engine.sess_cv, NULL);
  pthread_mutex_init(&g_engine.workerStartupMtx, NULL);
  pthread_cond_init(&g_engine.workerStartupCv, NULL);

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
    int rc = pthread_create(&g_engine.threads[i], &attr, worker_main,
                            (void *)(intptr_t)(i + 1));
    pthread_attr_destroy(&attr);
    if (rc != 0) {
      fprintf(stderr, "[audiograph] WARN: cannot start worker %d: %s\n", i,
              strerror(rc));
      break;
    }
    g_engine.workerCount++;
  }

  pthread_mutex_lock(&g_engine.workerStartupMtx);
  while (atomic_load_explicit(&g_engine.workerStartupCount,
                              memory_order_acquire) < g_engine.workerCount) {
    pthread_cond_wait(&g_engine.workerStartupCv, &g_engine.workerStartupMtx);
  }
  pthread_mutex_unlock(&g_engine.workerStartupMtx);
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

void engine_enable_graph_logging(int enable) {
  atomic_store_explicit(&g_engine.graph_log, enable ? 1 : 0, memory_order_release);
}

void engine_enable_rt_scheduling(int enable) {
  atomic_store_explicit(&g_engine.rt_scheduling, enable ? 1 : 0,
                        memory_order_release);
}

void engine_set_rt_priority(int priority) {
  atomic_store_explicit(&g_engine.rt_priority, priority, memory_order_release);
}

void engine_get_rt_status(EngineRtStatus *status) {
  if (!status)
    return;

  status->worker_count = g_engine.workerCount;
  status->workers_reported = atomic_load_explicit(&g_engine.workerStartupCount,
                                                  memory_order_acquire);
  status->worker_policy = ENGINE_SCHED_POLICY_UNKNOWN;
  status->worker_priority = 0;
  for (int i = 0; i < status->workers_reported && i < g_engine.workerCount; i++) {
    int policy = atomic_load_explicit(&g_engine.workerPolicies[i],
                                      memory_order_relaxed);
    int priority = atomic_load_explicit(&g_engine.workerPriorities[i],
                                        memory_order_relaxed);
    if (i == 0) {
      status->worker_policy = policy;
      status->worker_priority = priority;
    } else {
      if (status->worker_policy != policy)
        status->worker_policy = ENGINE_SCHED_POLICY_MIXED;
      if (status->worker_priority != priority)
        status->worker_priority = ENGINE_SCHED_PRIORITY_MIXED;
    }
  }

  status->callback_reported = atomic_load_explicit(&g_engine.callbackReported,
                                                   memory_order_acquire);
  status->callback_policy = atomic_load_explicit(&g_engine.callbackPolicy,
                                                 memory_order_relaxed);
  status->callback_priority = atomic_load_explicit(&g_engine.callbackPriority,
                                                   memory_order_relaxed);
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
  pthread_mutex_destroy(&g_engine.workerStartupMtx);
  pthread_cond_destroy(&g_engine.workerStartupCv);

  free(g_engine.threads);
  free(g_engine.workerPolicies);
  free(g_engine.workerPriorities);
  g_engine.threads = NULL;
  g_engine.workerPolicies = NULL;
  g_engine.workerPriorities = NULL;
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
        schedule_ready_node(lg, succ, nframes);
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
        schedule_ready_node(lg, succ, nframes);
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

static inline bool try_execute_ready_node(LiveGraph *lg, int32_t nid,
                                          int nframes) {
  if (nid < 0 || nid >= lg->node_count) {
    fprintf(stderr,
            "[audiograph] WARN: invalid ready job id %d (node_count=%d)\n",
            nid, lg->node_count);
    return false;
  }
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
  if (!claim_ready_node(lg, nid)) {
    return false;
  }
#endif
  atomic_fetch_add_explicit(&g_active_job_count, 1, memory_order_acq_rel);
  execute_and_fanout(lg, nid, nframes);
  atomic_fetch_sub_explicit(&g_active_job_count, 1, memory_order_acq_rel);
  return true;
}

static inline void schedule_ready_node(LiveGraph *lg, int32_t nid, int nframes) {
  ReadyQ *q = lg->sched.readyQueue;

  for (;;) {
    if (rq_push(q, nid)) {
      return;
    }

    // A bounded ready queue can fill in very wide/heavy graphs. Spinning here
    // can deadlock if every active audio/worker thread is also trying to
    // enqueue. Help the scheduler make progress by draining an already-ready
    // job, then retry the enqueue.
    int32_t other;
    if (rq_try_pop(q, &other)) {
      (void)try_execute_ready_node(lg, other, nframes);
      continue;
    }

    // If the queue looked full but another thread drained it first, retry.
    if (rq_push(q, nid)) {
      return;
    }

    // Last-resort progress path: the node is ready now, so running it inline
    // is equivalent to popping it from the ready queue.
    (void)try_execute_ready_node(lg, nid, nframes);
    return;
  }
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

typedef struct {
  int total_jobs;
  int source_count;
  int orphaned_count;
  int deleted_count;
} GraphTraceCounts;

static GraphTraceCounts graph_trace_count_scheduler(LiveGraph *lg) {
  GraphTraceCounts counts = {0, 0, 0, 0};
  if (!lg) {
    return counts;
  }

  for (int i = 0; i < lg->node_count; i++) {
    RTNode *node = &lg->nodes[i];
    bool deleted = (node->vtable.process == NULL && node->nInputs == 0 &&
                    node->nOutputs == 0);
    if (deleted) {
      counts.deleted_count++;
      continue;
    }
    if (lg->sched.is_orphaned && lg->sched.is_orphaned[i]) {
      counts.orphaned_count++;
      continue;
    }

    bool has_out = node_has_any_output_connected(lg, i);
    bool is_sink = !has_out && lg->sched.indegree && lg->sched.indegree[i] > 0;
    if (has_out || is_sink) {
      counts.total_jobs++;
      if (lg->sched.indegree && lg->sched.indegree[i] == 0 && has_out) {
        counts.source_count++;
      }
    }
  }

  return counts;
}

static float graph_trace_peak(const float *output_buffer, int nframes, int channels) {
  float peak = 0.0f;
  if (!output_buffer || nframes <= 0 || channels <= 0) {
    return peak;
  }
  size_t total = (size_t)nframes * (size_t)channels;
  for (size_t i = 0; i < total; i++) {
    float sample = output_buffer[i];
    float mag = sample < 0.0f ? -sample : sample;
    if (mag > peak) {
      peak = mag;
    }
  }
  return peak;
}

static float graph_trace_mono_peak(const float *buffer, int nframes) {
  float peak = 0.0f;
  if (!buffer || nframes <= 0) {
    return peak;
  }
  for (int i = 0; i < nframes; i++) {
    float sample = buffer[i];
    float mag = sample < 0.0f ? -sample : sample;
    if (mag > peak) {
      peak = mag;
    }
  }
  return peak;
}

static bool graph_trace_name_matches(const char *name) {
  if (!name) {
    return false;
  }
  return strstr(name, "_pan") || strstr(name, "_filter") ||
         strstr(name, "_delay") || strstr(name, "_sum") ||
         strstr(name, "_send") || strstr(name, "bus_L") ||
         strstr(name, "bus_R") || strstr(name, "reverb") ||
         strcmp(name, "SUM") == 0;
}

static float graph_trace_edge_peak(LiveGraph *lg, int edge_id, int nframes) {
  if (!lg || edge_id < 0 || edge_id >= lg->edge_capacity) {
    return 0.0f;
  }
  LiveEdge *edge = &lg->edges[edge_id];
  if (!edge->in_use || !edge->buf) {
    return 0.0f;
  }
  return graph_trace_mono_peak(edge->buf, nframes);
}

static void graph_trace_dump_node_io(LiveGraph *lg, int nid, int nframes) {
  if (!lg || nid < 0 || nid >= lg->node_count) {
    return;
  }

  RTNode *node = &lg->nodes[nid];
  const char *name = node->debug_name ? node->debug_name : "<unnamed>";
  bool deleted =
      (node->vtable.process == NULL && node->nInputs == 0 && node->nOutputs == 0);

  fprintf(stderr,
          "[audiograph-trace] route-node id=%d logical=%llu name=%s nIn=%d nOut=%d indegree=%d succ=%d orphaned=%d deleted=%d\n",
          nid, (unsigned long long)node->logical_id, name, node->nInputs,
          node->nOutputs,
          (lg->sched.indegree && nid < lg->node_count) ? lg->sched.indegree[nid]
                                                       : -1,
          node->succCount,
          (lg->sched.is_orphaned && nid < lg->node_count &&
           lg->sched.is_orphaned[nid])
              ? 1
              : 0,
          deleted ? 1 : 0);

  for (int port = 0; port < node->nInputs; port++) {
    int edge_id = node->inEdgeId ? node->inEdgeId[port] : -1;
    int src = (edge_id >= 0 && edge_id < lg->edge_capacity)
                  ? lg->edges[edge_id].src_node
                  : -1;
    int src_port = (edge_id >= 0 && edge_id < lg->edge_capacity)
                       ? lg->edges[edge_id].src_port
                       : -1;
    const char *src_name =
        (src >= 0 && src < lg->node_count && lg->nodes[src].debug_name)
            ? lg->nodes[src].debug_name
            : "<none>";
    fprintf(stderr,
            "[audiograph-trace] route-in node=%d name=%s port=%d edge=%d peak=%0.6f src=%d src_port=%d src_name=%s\n",
            nid, name, port, edge_id, graph_trace_edge_peak(lg, edge_id, nframes),
            src, src_port, src_name);
  }

  for (int port = 0; port < node->nOutputs; port++) {
    int edge_id = node->outEdgeId ? node->outEdgeId[port] : -1;
    int refcount = (edge_id >= 0 && edge_id < lg->edge_capacity)
                       ? lg->edges[edge_id].refcount
                       : 0;
    fprintf(stderr,
            "[audiograph-trace] route-out node=%d name=%s port=%d edge=%d peak=%0.6f refcount=%d\n",
            nid, name, port, edge_id,
            graph_trace_edge_peak(lg, edge_id, nframes), refcount);
  }
}

static void graph_trace_dump_signal_path(LiveGraph *lg, int nframes,
                                         const char *reason) {
  if (!lg || nframes <= 0) {
    return;
  }

  fprintf(stderr, "[audiograph-trace] signal dump begin reason=%s nframes=%d\n",
          reason ? reason : "<none>", nframes);

  int output_node = find_live_output(lg);
  if (output_node >= 0 && output_node < lg->node_count) {
    RTNode *dac = &lg->nodes[output_node];
    for (int ch = 0; ch < dac->nInputs; ch++) {
      int eid = dac->inEdgeId ? dac->inEdgeId[ch] : -1;
      int src = (eid >= 0 && eid < lg->edge_capacity) ? lg->edges[eid].src_node : -1;
      int src_port = (eid >= 0 && eid < lg->edge_capacity) ? lg->edges[eid].src_port : -1;
      float peak = (eid >= 0 && eid < lg->edge_capacity)
                       ? graph_trace_mono_peak(lg->edges[eid].buf, nframes)
                       : 0.0f;
      const char *src_name =
          (src >= 0 && src < lg->node_count && lg->nodes[src].debug_name)
              ? lg->nodes[src].debug_name
              : "<none>";
      fprintf(stderr,
              "[audiograph-trace] dac-input ch=%d edge=%d peak=%0.6f src=%d src_port=%d src_name=%s\n",
              ch, eid, peak, src, src_port, src_name);
    }
  }

  enum { TOP_EDGE_COUNT = 12 };
  float top_peak[TOP_EDGE_COUNT] = {0};
  int top_edge[TOP_EDGE_COUNT];
  for (int i = 0; i < TOP_EDGE_COUNT; i++) {
    top_edge[i] = -1;
  }

  for (int eid = 0; eid < lg->edge_capacity; eid++) {
    LiveEdge *edge = &lg->edges[eid];
    if (!edge->in_use || !edge->buf) {
      continue;
    }
    float peak = graph_trace_mono_peak(edge->buf, nframes);
    for (int rank = 0; rank < TOP_EDGE_COUNT; rank++) {
      if (peak <= top_peak[rank]) {
        continue;
      }
      for (int move = TOP_EDGE_COUNT - 1; move > rank; move--) {
        top_peak[move] = top_peak[move - 1];
        top_edge[move] = top_edge[move - 1];
      }
      top_peak[rank] = peak;
      top_edge[rank] = eid;
      break;
    }
  }

  for (int rank = 0; rank < TOP_EDGE_COUNT; rank++) {
    int eid = top_edge[rank];
    if (eid < 0) {
      continue;
    }
    LiveEdge *edge = &lg->edges[eid];
    int src = edge->src_node;
    const char *src_name =
        (src >= 0 && src < lg->node_count && lg->nodes[src].debug_name)
            ? lg->nodes[src].debug_name
            : "<none>";
    fprintf(stderr,
            "[audiograph-trace] top-edge rank=%d edge=%d peak=%0.6f src=%d src_port=%d refcount=%d src_name=%s\n",
              rank + 1, eid, top_peak[rank], src, edge->src_port, edge->refcount,
            src_name);
  }

  fprintf(stderr, "[audiograph-trace] route dump begin\n");
  for (int nid = 0; nid < lg->node_count; nid++) {
    RTNode *node = &lg->nodes[nid];
    if (!graph_trace_name_matches(node->debug_name)) {
      continue;
    }
    graph_trace_dump_node_io(lg, nid, nframes);
  }
  fprintf(stderr, "[audiograph-trace] route dump end\n");

  fprintf(stderr, "[audiograph-trace] signal dump end\n");
}

static void graph_trace_log_block(LiveGraph *lg, int nframes, float peak,
                                  bool topology_event, bool edits_ok) {
  if (!lg || !atomic_load_explicit(&g_engine.graph_log, memory_order_acquire)) {
    return;
  }

  uint64_t block = atomic_fetch_add_explicit(&g_graph_trace_block_counter, 1,
                                             memory_order_acq_rel) +
                   1;
  bool silent = peak <= 0.000001f;
  uint32_t silent_streak = 0;
  if (silent) {
    silent_streak =
        atomic_fetch_add_explicit(&g_graph_trace_silent_streak, 1,
                                  memory_order_acq_rel) +
        1;
  } else {
    atomic_store_explicit(&g_graph_trace_silent_streak, 0, memory_order_release);
  }

  bool periodic = (block % 86u) == 0;
  bool silent_checkpoint =
      silent && (silent_streak == 1 || silent_streak == 16 ||
                 (silent_streak % 128u) == 0);
  if (!topology_event && edits_ok && !periodic && !silent_checkpoint) {
    return;
  }

  int ready_qlen = 0;
  int ready_waiters = 0;
  uint64_t ready_head = 0;
  uint64_t ready_tail = 0;
  uint32_t ready_mask = 0;
  if (lg->sched.readyQueue) {
    ready_qlen =
        atomic_load_explicit(&lg->sched.readyQueue->qlen, memory_order_acquire);
    ready_waiters =
        atomic_load_explicit(&lg->sched.readyQueue->waiters, memory_order_acquire);
    if (lg->sched.readyQueue->ring) {
      ready_head = atomic_load_explicit(&lg->sched.readyQueue->ring->head,
                                        memory_order_acquire);
      ready_tail = atomic_load_explicit(&lg->sched.readyQueue->ring->tail,
                                        memory_order_acquire);
      ready_mask = lg->sched.readyQueue->ring->mask;
    }
  }

  GraphTraceCounts counts = graph_trace_count_scheduler(lg);
  int jobs = atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire);
  int active_jobs = atomic_load_explicit(&g_active_job_count, memory_order_acquire);
  int active_workers =
      atomic_load_explicit(&g_engine.activeWorkerLimit, memory_order_acquire);
  uint32_t param_head = 0;
  uint32_t param_tail = 0;
  if (lg->params) {
    param_head = atomic_load_explicit(&lg->params->head, memory_order_acquire);
    param_tail = atomic_load_explicit(&lg->params->tail, memory_order_acquire);
  }
  uint64_t param_pushes =
      atomic_load_explicit(&g_param_push_count, memory_order_acquire);
  uint64_t param_fails =
      atomic_load_explicit(&g_param_push_fail_count, memory_order_acquire);
  uint64_t param_applied =
      atomic_load_explicit(&g_param_apply_count, memory_order_acquire);
  uint64_t event_pushes =
      atomic_load_explicit(&g_block_event_push_count, memory_order_acquire);
  uint64_t event_fails =
      atomic_load_explicit(&g_block_event_push_fail_count, memory_order_acquire);
  uint64_t events_applied =
      atomic_load_explicit(&g_block_event_apply_count, memory_order_acquire);
  uint64_t events_stale =
      atomic_load_explicit(&g_block_event_drop_stale_count, memory_order_acquire);
  uint64_t events_invalid =
      atomic_load_explicit(&g_block_event_drop_invalid_count, memory_order_acquire);
  uint64_t events_unsupported = atomic_load_explicit(
      &g_block_event_drop_unsupported_count, memory_order_acquire);
  uint64_t events_rejected = atomic_load_explicit(
      &g_block_event_schedule_reject_count, memory_order_acquire);

  fprintf(stderr,
          "[audiograph-trace] block=%llu nframes=%d peak=%0.6f silent_streak=%u topology_event=%d edits_ok=%d jobsInFlight=%d activeJobs=%d readyQ=%d waiters=%d readyHead=%llu readyTail=%llu readyMask=%u node_count=%d cached_total_jobs=%d recounted_total_jobs=%d source_count=%d recounted_source_count=%d orphaned=%d deleted=%d dirty=%d has_cycle=%d workers=%d active_workers=%d param_backlog=%u param_head=%u param_tail=%u param_pushes=%llu param_applied=%llu param_fails=%llu event_pushes=%llu event_applied=%llu event_fails=%llu event_stale=%llu event_invalid=%llu event_unsupported=%llu event_rejected=%llu\n",
          (unsigned long long)block, nframes, peak, silent_streak,
          topology_event ? 1 : 0, edits_ok ? 1 : 0, jobs, active_jobs,
          ready_qlen, ready_waiters, (unsigned long long)ready_head,
          (unsigned long long)ready_tail, ready_mask, lg->node_count,
          lg->sched.cached_total_jobs, counts.total_jobs, lg->sched.source_count,
          counts.source_count, counts.orphaned_count, counts.deleted_count,
          lg->sched.dirty ? 1 : 0, lg->sched.has_cycle ? 1 : 0,
          g_engine.workerCount, active_workers, param_head - param_tail, param_head,
          param_tail, (unsigned long long)param_pushes,
          (unsigned long long)param_applied, (unsigned long long)param_fails,
          (unsigned long long)event_pushes, (unsigned long long)events_applied,
          (unsigned long long)event_fails, (unsigned long long)events_stale,
          (unsigned long long)events_invalid,
          (unsigned long long)events_unsupported,
          (unsigned long long)events_rejected);

  if (silent_checkpoint &&
      (silent_streak == 1 || silent_streak == 16 ||
       (silent_streak % 128u) == 0)) {
    graph_trace_dump_signal_path(lg, nframes,
                                 silent_streak == 1 ? "first-silent"
                                                    : "silent-checkpoint");
  }
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

static void init_pending_and_seed(LiveGraph *lg, int nframes) {
  // Rebuild cache if topology changed
  if (lg->sched.dirty) {
    rebuild_scheduling_cache(lg);
  }

  // Properly reset/drain the ready queue and any stale semaphore signals before
  // publishing this block's work.
  rq_reset(lg->sched.readyQueue);

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

  // Seed ready queue from cached source list - O(sources) instead of O(n).
  // Use the helping enqueue path so a graph with more sources than queue slots
  // cannot deadlock the audio callback during block setup.
  for (int i = 0; i < lg->sched.source_count; i++) {
    schedule_ready_node(lg, lg->sched.source_nodes[i], nframes);
  }
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
  init_pending_and_seed(lg, nframes);

  // Check for cycles that would cause silent deadlocks
  if (detect_cycle(lg)) {
    atomic_store_explicit(&lg->sched.jobsInFlight, 0, memory_order_release);
    rq_reset(lg->sched.readyQueue);

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
    uint64_t stalled_empty_since_ns = 0;
#if AUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS
    bool stall_logged = false;
    struct timespec wait_started;
    clock_gettime(CLOCK_MONOTONIC, &wait_started);
#endif
    while (atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire) > 0) {
      if (rq_try_pop(lg->sched.readyQueue, &nid)) {
        if (!try_execute_ready_node(lg, nid, nframes)) {
          continue;
        }
        empty_spins = 0; // Reset on successful work
        stalled_empty_since_ns = 0;
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
          int active_jobs =
              atomic_load_explicit(&g_active_job_count, memory_order_acquire);
          if (active_jobs > 0) {
            stalled_empty_since_ns = 0;
          } else {
            uint64_t now_ns = nsec_now();
            if (stalled_empty_since_ns == 0) {
              stalled_empty_since_ns = now_ns;
            } else if (now_ns - stalled_empty_since_ns >=
                       AUDIOGRAPH_STALL_RECOVERY_TIMEOUT_NS) {
              int jobs =
                  atomic_load_explicit(&lg->sched.jobsInFlight, memory_order_acquire);
              int qlen =
                  atomic_load_explicit(&lg->sched.readyQueue->qlen, memory_order_acquire);
              uint64_t recovery = atomic_fetch_add_explicit(
                                      &g_stall_recovery_count, 1,
                                      memory_order_acq_rel) +
                                  1;
              fprintf(stderr,
                      "[audiograph] STALL RECOVERY #%llu: no active/ready jobs, jobsInFlight=%d readyQ=%d node_count=%d source_count=%d cached_total_jobs=%d dirty=%d\n",
                      (unsigned long long)recovery, jobs, qlen, lg->node_count,
                      lg->sched.source_count, lg->sched.cached_total_jobs,
                      lg->sched.dirty ? 1 : 0);
              atomic_store_explicit(&lg->sched.jobsInFlight, 0,
                                    memory_order_release);
              rq_reset(lg->sched.readyQueue);
              break;
            }
          }
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
      (void)try_execute_ready_node(lg, nid, nframes);
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

  int before_node_count = lg->node_count;
  int before_cached_jobs = lg->sched.cached_total_jobs;
  int before_source_count = lg->sched.source_count;
  bool before_dirty = lg->sched.dirty;
  bool edits_ok = true;
  bool topology_event = false;

  if (atomic_load_explicit(&lg->edit_batch_depth, memory_order_acquire) == 0) {
    edits_ok = apply_graph_edits(lg->graphEditQueue, lg);
    topology_event = !edits_ok || before_node_count != lg->node_count ||
                     before_cached_jobs != lg->sched.cached_total_jobs ||
                     before_source_count != lg->sched.source_count ||
                     before_dirty != lg->sched.dirty;
    if (lg->sched.dirty) {
      rebuild_invalid_io_caches(lg, lg->block_size);
    }
  }

  apply_params(lg);
  (void)drain_block_events_for_callback(lg, nframes);
  uint64_t block_serial =
      atomic_fetch_add_explicit(&lg->block_event_serial, 1, memory_order_acq_rel);

  // Process in slices if callback frames exceed internal block size.
  int remaining = nframes;
  int out_offset = 0; // in frames
  int event_index = 0;
  while (remaining > 0) {
    int slice = remaining;
    if (slice > lg->block_size)
      slice = lg->block_size;

    begin_event_slice_for_nodes(lg, block_serial, out_offset, slice);
    event_index = deliver_block_events_for_slice(lg, event_index, out_offset, slice);

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

  graph_trace_log_block(lg, nframes,
                        graph_trace_peak(output_buffer, nframes, lg->num_channels),
                        topology_event, edits_ok);

  // Throttle watchlist snapshots in the audio render path. A full snapshot can
  // be expensive when many UI/polling operators are watched, and most consumers
  // don't need audio-block-rate state updates.
#if AUDIOGRAPH_WATCH_UPDATE_INTERVAL <= 1
  update_watched_node_states(lg);
#else
  uint32_t watch_tick = atomic_fetch_add_explicit(&lg->watch.update_counter, 1,
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
