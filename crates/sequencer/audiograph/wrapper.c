#include "graph_engine.h"

// Expose the static inline params_push as a linkable symbol for Rust FFI.
bool params_push_wrapper(LiveGraph *lg, ParamMsg m) {
    atomic_fetch_add_explicit(&g_param_push_count, 1, memory_order_relaxed);
    bool ok = params_push(lg->params, m);
    if (!ok) {
        uint64_t fail = atomic_fetch_add_explicit(
                            &g_param_push_fail_count, 1, memory_order_acq_rel) +
                        1;
        if (fail == 1 || (fail % 64) == 0) {
            uint32_t h = atomic_load_explicit(&lg->params->head, memory_order_acquire);
            uint32_t t = atomic_load_explicit(&lg->params->tail, memory_order_acquire);
            fprintf(stderr,
                    "[audiograph-trace] PARAM PUSH FAILED count=%llu backlog=%u head=%u tail=%u logical=%llu idx=%llu value=%0.6f\n",
                    (unsigned long long)fail, h - t, h, t,
                    (unsigned long long)m.logical_id, (unsigned long long)m.idx,
                    m.fvalue);
        }
    }
    return ok;
}

// Expose the static inline block_events_push as a linkable symbol for Rust FFI.
bool push_block_event(LiveGraph *lg, GraphBlockEvent event) {
    if (!lg || !lg->block_events) {
        return false;
    }
    atomic_fetch_add_explicit(&g_block_event_push_count, 1, memory_order_relaxed);
    if (event.aux_count > GBE_AUX_CAP) {
        event.aux_count = GBE_AUX_CAP;
    }
    bool ok = block_events_push(lg->block_events, event);
    if (!ok) {
        uint64_t fail = atomic_fetch_add_explicit(
                            &g_block_event_push_fail_count, 1, memory_order_acq_rel) +
                        1;
        if (fail == 1 || (fail % 64) == 0) {
            uint32_t h = atomic_load_explicit(&lg->block_events->head, memory_order_acquire);
            uint32_t t = atomic_load_explicit(&lg->block_events->tail, memory_order_acquire);
            fprintf(stderr,
                    "[audiograph-trace] BLOCK EVENT PUSH FAILED count=%llu backlog=%u head=%u tail=%u logical=%llu frame=%u kind=%u aux_count=%u\n",
                    (unsigned long long)fail, h - t, h, t,
                    (unsigned long long)event.logical_id, event.frame_offset,
                    event.kind, event.aux_count);
        }
    }
    return ok;
}
