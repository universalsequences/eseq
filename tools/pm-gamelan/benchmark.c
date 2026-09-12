/* Native timing driver; no audio-kernel source is modified for benchmarking. */
#include "../../crates/sequencer/audiograph/dgen_abi_v1.h"
#include <time.h>
#include <string.h>

typedef void (*process_t)(const float *const *, float *const *, uint32_t, void *,
                         const DGenProcessContextV1 *, const DGenHostServicesV1 *);

double run(process_t process, const float *const *inputs, float *const *outputs, uint32_t frames,
           void *state, const DGenProcessContextV1 *context, float *trigger, uint32_t blocks) {
    struct timespec start, end;
    const uint64_t hit_period = (uint64_t)(context->sample_rate * 0.5f);
    uint64_t sample = 0, next_hit = 0;
    clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &start);
    for (uint32_t block = 0; block < blocks; block++) {
        /* Exactly two strikes per second, independent of process-call size. */
        memset(trigger, 0, frames * sizeof(float));
        while (next_hit < sample + frames) {
            trigger[next_hit - sample] = 1.0f;
            next_hit += hit_period;
        }
        process(inputs, outputs, frames, state, context, 0);
        sample += frames;
    }
    clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &end);
    return (end.tv_sec - start.tv_sec) + (end.tv_nsec - start.tv_nsec)*1e-9;
}
