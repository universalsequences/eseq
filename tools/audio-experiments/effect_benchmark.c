/* Native effect timing with the app's FFT host services; no Python in the loop. */
#include "../../crates/sequencer/audiograph/dgen_host_services.h"
#include <time.h>

typedef void (*process_fn)(const float *const *, float *const *, uint32_t, void *,
                           const DGenProcessContextV1 *, const DGenHostServicesV1 *);

double benchmark_effect(process_fn process, const float *const *inputs,
                        float *const *outputs, uint32_t frames, float *state,
                        const DGenProcessContextV1 *context, uint32_t blocks,
                        const uint32_t *cells, uint32_t cell_count,
                        const float *values) {
    const DGenHostServicesV1 *host = eseq_dgen_host_services_v1();
    struct timespec start, end;
    if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &start)) return -1;
    for (uint32_t block = 0; block < blocks; ++block) {
        for (uint32_t param = 0; param < cell_count; ++param)
            state[cells[param]] = values[(size_t)block * cell_count + param];
        process(inputs, outputs, frames, state, context, host);
    }
    if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &end)) return -1;
    return (end.tv_sec - start.tv_sec) + (end.tv_nsec - start.tv_nsec) * 1e-9;
}
