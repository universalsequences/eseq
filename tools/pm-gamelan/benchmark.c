/* Native timing driver; no audio-kernel source is modified for benchmarking. */
#include "../../crates/sequencer/audiograph/dgen_abi_v1.h"
#include <time.h>

typedef void (*process_t)(const float *const *, float *const *, uint32_t, void *,
                         const DGenProcessContextV1 *, const DGenHostServicesV1 *);

double run(process_t process, const float *const *inputs, float *const *outputs, uint32_t frames,
           void *state, const DGenProcessContextV1 *context, float *trigger, uint32_t blocks) {
    struct timespec start, end;
    clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &start);
    for (uint32_t block = 0; block < blocks; block++) {
        /* About two strikes per second at the report's 48 kHz / 128 frames. */
        trigger[0] = (block % 187 == 0);
        process(inputs, outputs, frames, state, context, 0);
    }
    clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &end);
    return (end.tv_sec - start.tv_sec) + (end.tv_nsec - start.tv_nsec)*1e-9;
}
