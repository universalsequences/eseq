/*
 * test_fm_benchmark.c — FM synthesis stress test for threading model profiling
 *
 * Builds a 4000+ node graph:
 *   1000 sine oscillators (oscsB, harmonics of 330 Hz)
 *   1000 sine LFOs        (oscsC, harmonics of 0.1 Hz)
 *   1000 multiply nodes   (mult[i] = oscsB[i] * oscsC[i])
 *   1000 FM oscillators   (oscsA[i], phase-modulated by mult[i])
 *   1 gain (0.001)        (sums all 1000 FM oscs via auto-sum)
 *   1 DAC
 *
 * Then benchmarks process_next_block across configurations:
 *   - 0 workers  (audio thread only, single-threaded)
 *   - 3 workers  (4 total threads, normal scheduling)
 *   - 3 workers  (4 total threads, Mach RT time-constraint)
 */

#include "../graph_engine.h"
#include "../graph_edit.h"
#include "../graph_nodes.h"
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef __APPLE__
#include <mach/mach_time.h>
#endif

#define N_OSCS 1000
#define BLOCK_SIZE 512
#define SAMPLE_RATE 48000
#define NUM_BLOCKS 100
#define WARMUP_BLOCKS 5

// ===================== Custom Node Kernels =====================

// --- Sine oscillator: 0 inputs, 1 output ---
// State: [0] = phase, [1] = phase_increment

static void sine_osc_init(void *memory, int sr, int maxBlock,
                           const void *initial_state) {
  (void)sr;
  (void)maxBlock;
  float *mem = (float *)memory;
  mem[0] = 0.0f; // phase
  if (initial_state) {
    const float *init = (const float *)initial_state;
    mem[1] = init[0]; // phase increment = freq / sr
  } else {
    mem[1] = 0.0f;
  }
}

static void sine_osc_process(float *const *in, float *const *out, int n,
                              void *memory, void *buffers) {
  (void)in;
  (void)buffers;
  float *mem = (float *)memory;
  float *y = out[0];
  float phase = mem[0];
  float inc = mem[1];
  for (int i = 0; i < n; i++) {
    y[i] = sinf(2.0f * (float)M_PI * phase);
    phase += inc;
    if (phase >= 1.0f)
      phase -= 1.0f;
  }
  mem[0] = phase;
}

static const NodeVTable SINE_OSC_VT = {.process = sine_osc_process,
                                        .init = sine_osc_init,
                                        .reset = NULL,
                                        .migrate = NULL};

// --- FM oscillator: 1 input (phase modulation), 1 output ---
// State: [0] = phase, [1] = phase_increment

static void fm_osc_init(void *memory, int sr, int maxBlock,
                         const void *initial_state) {
  (void)sr;
  (void)maxBlock;
  float *mem = (float *)memory;
  mem[0] = 0.0f; // phase
  if (initial_state) {
    const float *init = (const float *)initial_state;
    mem[1] = init[0]; // phase increment = freq / sr
  } else {
    mem[1] = 0.0f;
  }
}

static void fm_osc_process(float *const *in, float *const *out, int n,
                            void *memory, void *buffers) {
  (void)buffers;
  float *mem = (float *)memory;
  float *y = out[0];
  const float *phase_mod = in[0]; // modulation signal
  float phase = mem[0];
  float inc = mem[1];
  for (int i = 0; i < n; i++) {
    y[i] = sinf(2.0f * (float)M_PI * (phase + phase_mod[i]));
    phase += inc;
    if (phase >= 1.0f)
      phase -= 1.0f;
  }
  mem[0] = phase;
}

static const NodeVTable FM_OSC_VT = {.process = fm_osc_process,
                                      .init = fm_osc_init,
                                      .reset = NULL,
                                      .migrate = NULL};

// --- Multiply: 2 inputs, 1 output (sample-wise) ---

static void mult_process(float *const *in, float *const *out, int n,
                          void *memory, void *buffers) {
  (void)memory;
  (void)buffers;
  const float *a = in[0];
  const float *b = in[1];
  float *y = out[0];
  for (int i = 0; i < n; i++)
    y[i] = a[i] * b[i];
}

static const NodeVTable MULT_VT = {
    .process = mult_process, .init = NULL, .reset = NULL, .migrate = NULL};

// ===================== Helpers =====================

static int add_custom(LiveGraph *lg, NodeVTable vt, size_t state_size,
                      const char *name, int nIn, int nOut,
                      const void *init_state) {
  int nid = atomic_fetch_add(&lg->next_node_id, 1);
  int r =
      apply_add_node(lg, vt, state_size, (uint64_t)nid, name, nIn, nOut, init_state);
  if (r < 0) {
    add_failed_id(lg, nid);
    return -1;
  }
  return r;
}

// High-resolution timer (nanoseconds)
static uint64_t now_ns(void) {
#ifdef __APPLE__
  static mach_timebase_info_data_t tb;
  if (tb.denom == 0)
    mach_timebase_info(&tb);
  return mach_absolute_time() * tb.numer / tb.denom;
#else
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000ULL + ts.tv_nsec;
#endif
}

// ===================== Graph Builder =====================

static LiveGraph *build_fm_graph(void) {
  // Capacity: ~4002 user nodes + auto-sum nodes (~1000 more in worst case)
  LiveGraph *lg = create_live_graph(8192, BLOCK_SIZE, "FM-Bench", 1);
  if (!lg)
    return NULL;

  float sr = (float)SAMPLE_RATE;

  int oscsA[N_OSCS]; // FM oscillators (harmonic series of 220 Hz)
  int oscsB[N_OSCS]; // Sine oscillators (harmonic series of 330 Hz)
  int oscsC[N_OSCS]; // Sine LFOs (harmonic series of 0.1 Hz)
  int mults[N_OSCS]; // Multiply nodes

  float base_a = 220.0f;
  float base_b = 330.0f;
  float base_c = 0.1f;

  // Create all nodes
  for (int i = 0; i < N_OSCS; i++) {
    float harmonic = (float)(i + 1);

    // oscsB: sine oscillators at harmonics of 330 Hz
    float inc_b = (base_b * harmonic) / sr;
    oscsB[i] = add_custom(lg, SINE_OSC_VT, 2 * sizeof(float), NULL, 0, 1, &inc_b);

    // oscsC: sine LFOs at harmonics of 0.1 Hz
    float inc_c = (base_c * harmonic) / sr;
    oscsC[i] = add_custom(lg, SINE_OSC_VT, 2 * sizeof(float), NULL, 0, 1, &inc_c);

    // mults: sample-wise multiply (oscB * oscC)
    mults[i] = add_custom(lg, MULT_VT, 0, NULL, 2, 1, NULL);

    // oscsA: FM oscillators at harmonics of 220 Hz, 1 input for phase mod
    float inc_a = (base_a * harmonic) / sr;
    oscsA[i] = add_custom(lg, FM_OSC_VT, 2 * sizeof(float), NULL, 1, 1, &inc_a);
  }

  // Create gain node (0.001 to tame 1000 summed oscillators)
  int gain = live_add_gain(lg, 0.001f, "master_gain");

  // Wire everything up
  for (int i = 0; i < N_OSCS; i++) {
    // oscB[i] -> mult[i].in[0]
    apply_connect(lg, oscsB[i], 0, mults[i], 0);
    // oscC[i] -> mult[i].in[1]
    apply_connect(lg, oscsC[i], 0, mults[i], 1);
    // mult[i] -> oscA[i].in[0]  (phase modulation)
    apply_connect(lg, mults[i], 0, oscsA[i], 0);
    // oscA[i] -> gain.in[0]  (auto-summed)
    apply_connect(lg, oscsA[i], 0, gain, 0);
  }

  // gain -> DAC
  apply_connect(lg, gain, 0, lg->dac_node_id, 0);

  printf("  Graph built: %d nodes (incl. DAC + auto-sum)\n", lg->node_count);
  return lg;
}

// ===================== Benchmark Runner =====================

typedef struct {
  double avg_ms;
  double min_ms;
  double max_ms;
  double median_ms;
} BenchResult;

static int cmp_double(const void *a, const void *b) {
  double da = *(const double *)a, db = *(const double *)b;
  return (da > db) - (da < db);
}

static BenchResult run_benchmark(LiveGraph *lg) {
  float *buf = calloc(BLOCK_SIZE, sizeof(float));
  double times_ms[NUM_BLOCKS];

  // Warmup
  for (int i = 0; i < WARMUP_BLOCKS; i++)
    process_next_block(lg, buf, BLOCK_SIZE);

  // Timed run
  for (int i = 0; i < NUM_BLOCKS; i++) {
    uint64_t t0 = now_ns();
    process_next_block(lg, buf, BLOCK_SIZE);
    uint64_t t1 = now_ns();
    times_ms[i] = (double)(t1 - t0) / 1e6;
  }

  // Compute stats
  qsort(times_ms, NUM_BLOCKS, sizeof(double), cmp_double);

  double sum = 0;
  for (int i = 0; i < NUM_BLOCKS; i++)
    sum += times_ms[i];

  BenchResult r;
  r.avg_ms = sum / NUM_BLOCKS;
  r.min_ms = times_ms[0];
  r.max_ms = times_ms[NUM_BLOCKS - 1];
  r.median_ms = times_ms[NUM_BLOCKS / 2];

  free(buf);
  return r;
}

// ===================== Main =====================

int main(void) {
  printf("=== FM Synthesis Threading Benchmark ===\n");
  printf("Graph: %d FM oscillators, phase-modulated by %d sine*LFO pairs\n",
         N_OSCS, N_OSCS);
  printf("Block: %d samples @ %d Hz (%.2f ms budget)\n", BLOCK_SIZE,
         SAMPLE_RATE, (double)BLOCK_SIZE / SAMPLE_RATE * 1000.0);
  printf("Timing: %d blocks (+ %d warmup)\n\n", NUM_BLOCKS, WARMUP_BLOCKS);

  initialize_engine(BLOCK_SIZE, SAMPLE_RATE);

  // Build the graph once — reuse across configs
  printf("Building graph...\n");
  LiveGraph *lg = build_fm_graph();
  if (!lg) {
    fprintf(stderr, "Failed to build graph\n");
    return 1;
  }

  // --- Config 1: single-threaded (0 workers) ---
  printf("\n[1/4] 1 thread (audio thread only)...\n");
  BenchResult r1 = run_benchmark(lg);
  printf("       avg=%.3f ms  median=%.3f ms  min=%.3f ms  max=%.3f ms\n",
         r1.avg_ms, r1.median_ms, r1.min_ms, r1.max_ms);

  // --- Config 2: 4 normal threads (3 workers + audio thread) ---
  printf("\n[2/4] 4 normal threads...\n");
  engine_start_workers(3);
  BenchResult r2 = run_benchmark(lg);
  engine_stop_workers();
  printf("       avg=%.3f ms  median=%.3f ms  min=%.3f ms  max=%.3f ms\n",
         r2.avg_ms, r2.median_ms, r2.min_ms, r2.max_ms);

  // --- Config 3: 4 Mach RT threads ---
  printf("\n[3/4] 4 Mach RT threads...\n");
  engine_enable_rt_time_constraint(1);
  engine_start_workers(3);
  BenchResult r3 = run_benchmark(lg);
  engine_stop_workers();
  engine_enable_rt_time_constraint(0);
  printf("       avg=%.3f ms  median=%.3f ms  min=%.3f ms  max=%.3f ms\n",
         r3.avg_ms, r3.median_ms, r3.min_ms, r3.max_ms);

  // --- Config 4: 8 Mach RT threads ---
  printf("\n[4/4] 8 Mach RT threads...\n");
  engine_enable_rt_time_constraint(1);
  engine_start_workers(7);
  BenchResult r4 = run_benchmark(lg);
  engine_stop_workers();
  engine_enable_rt_time_constraint(0);
  printf("       avg=%.3f ms  median=%.3f ms  min=%.3f ms  max=%.3f ms\n",
         r4.avg_ms, r4.median_ms, r4.min_ms, r4.max_ms);

  // --- Summary table ---
  double budget_ms = (double)BLOCK_SIZE / SAMPLE_RATE * 1000.0;
  double baseline = r1.median_ms;
  printf("\n");
  printf("┌──────────────────────────────────────┬────────────┬─────────┬──────────┐\n");
  printf("│ Configuration                        │  Median    │ Speedup │ CPU %%    │\n");
  printf("├──────────────────────────────────────┼────────────┼─────────┼──────────┤\n");
  printf("│ 1 thread (audio only)                │ %7.2f ms │  1.00x  │ %5.1f%%   │\n",
         r1.median_ms, (r1.median_ms / budget_ms) * 100.0);
  printf("│ 4 normal threads                     │ %7.2f ms │  %4.2fx  │ %5.1f%%   │\n",
         r2.median_ms, baseline / r2.median_ms,
         (r2.median_ms / budget_ms) * 100.0);
  printf("│ 4 Mach RT threads                    │ %7.2f ms │  %4.2fx  │ %5.1f%%   │\n",
         r3.median_ms, baseline / r3.median_ms,
         (r3.median_ms / budget_ms) * 100.0);
  printf("│ 8 Mach RT threads                    │ %7.2f ms │  %4.2fx  │ %5.1f%%   │\n",
         r4.median_ms, baseline / r4.median_ms,
         (r4.median_ms / budget_ms) * 100.0);
  printf("│ 4 RT threads + OS Workgroup (*)      │     —      │    —    │    —     │\n");
  printf("│ 8 RT threads + OS Workgroup (*)      │     —      │    —    │    —     │\n");
  printf("└──────────────────────────────────────┴────────────┴─────────┴──────────┘\n");
  printf("\n(*) OS Workgroup requires an os_workgroup_t from AVAudioEngine;\n");
  printf("    run the Swift example with AudioUnit for this measurement.\n");
  printf("\nBlock budget: %.2f ms (%d samples @ %d Hz)\n", budget_ms,
         BLOCK_SIZE, SAMPLE_RATE);
  printf("Graph: %d nodes (%d FM oscs + %d sine oscs + %d LFOs + %d multipliers"
         " + gain + DAC + auto-sum)\n",
         lg->node_count, N_OSCS, N_OSCS, N_OSCS, N_OSCS);

  if (r1.median_ms > budget_ms) {
    printf("\nNote: single-thread exceeds real-time budget — parallelism is essential.\n");
  }

  destroy_live_graph(lg);
  printf("\nDone.\n");
  return 0;
}
