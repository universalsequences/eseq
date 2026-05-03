#include "../graph_edit.h"
#include "../graph_engine.h"
#include "../graph_nodes.h"
#include <assert.h>
#include <math.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define DEFAULT_VOICES 64
#define DEFAULT_BLOCKS 20000
#define BLOCK_SIZE 128
#define SAMPLE_RATE 48000
#define MOD_OUTS 8
#define SYNTH_INS (4 + MOD_OUTS)

typedef struct {
  float phase;
  float base_freq;
  float gate_phase;
  float voice;
} GatePitchState;

typedef struct {
  float phase[MOD_OUTS];
} ModState;

typedef struct {
  float phase[4];
  float lp_l;
  float lp_r;
} SynthState;

static double now_ms(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

static int env_int(const char *name, int fallback) {
  const char *value = getenv(name);
  if (!value || !*value)
    return fallback;
  int parsed = atoi(value);
  return parsed > 0 ? parsed : fallback;
}

static void timeout_handler(int sig) {
  (void)sig;
  fprintf(stderr, "polysynth scheduler stress timed out\n");
  exit(2);
}

static void gatepitch_init(void *memory, int sr, int maxBlock,
                           const void *initial_state) {
  (void)sr;
  (void)maxBlock;
  GatePitchState *s = (GatePitchState *)memory;
  int voice = initial_state ? *(const int *)initial_state : 0;
  memset(s, 0, sizeof(*s));
  s->voice = (float)voice;
  s->base_freq = 110.0f + (float)(voice % 24) * 7.0f;
}

static void gatepitch_process(float *const *in, float *const *out, int n,
                              void *memory, void *buffers) {
  (void)in;
  (void)buffers;
  GatePitchState *s = (GatePitchState *)memory;
  float gate_phase = s->gate_phase;
  float phase = s->phase;
  float freq = s->base_freq;
  float vel = 0.35f + 0.65f * fmodf(s->voice * 0.173f, 1.0f);
  for (int i = 0; i < n; i++) {
    gate_phase += 0.00011f + s->voice * 0.0000007f;
    if (gate_phase >= 1.0f)
      gate_phase -= 1.0f;
    float gate = gate_phase < 0.62f ? 1.0f : 0.0f;
    phase += freq / (float)SAMPLE_RATE;
    if (phase >= 1.0f)
      phase -= 1.0f;
    out[0][i] = gate;
    out[1][i] = freq;
    out[2][i] = vel;
    out[3][i] = phase;
  }
  s->gate_phase = gate_phase;
  s->phase = phase;
}

static void mod_process(float *const *in, float *const *out, int n,
                        void *memory, void *buffers) {
  (void)buffers;
  ModState *s = (ModState *)memory;
  const float *gate = in[0];
  const float *freq = in[1];
  const float *vel = in[2];
  const float *seed = in[3];
  for (int i = 0; i < n; i++) {
    for (int m = 0; m < MOD_OUTS; m++) {
      float rate = 0.03f + 0.006f * (float)(m + 1);
      s->phase[m] += rate + freq[i] * 0.0000002f;
      if (s->phase[m] >= 1.0f)
        s->phase[m] -= 1.0f;
      out[m][i] = gate[i] * vel[i] *
                  sinf(6.28318530718f * (s->phase[m] + seed[i] * 0.01f));
    }
  }
}

static void synth_process(float *const *in, float *const *out, int n,
                          void *memory, void *buffers) {
  (void)buffers;
  SynthState *s = (SynthState *)memory;
  const float *gate = in[0];
  const float *freq = in[1];
  const float *vel = in[2];
  float *left = out[0];
  float *right = out[1];

  for (int i = 0; i < n; i++) {
    float sample = 0.0f;
    for (int op = 0; op < 4; op++) {
      float mod = in[4 + op][i] * (0.2f + 0.1f * (float)op);
      float inc = (freq[i] * (float)(op + 1)) / (float)SAMPLE_RATE;
      s->phase[op] += inc + mod * 0.0007f;
      if (s->phase[op] >= 1.0f)
        s->phase[op] -= 1.0f;
      sample += sinf(6.28318530718f * (s->phase[op] + mod));
    }
    // Extra math widens worker scheduling windows without changing topology.
    for (int k = 0; k < 6; k++) {
      sample = tanhf(sample + in[4 + (k % MOD_OUTS)][i] * 0.08f);
    }
    sample *= gate[i] * vel[i] * 0.03f;
    s->lp_l += 0.12f * (sample - s->lp_l);
    s->lp_r += 0.10f * ((sample * 0.85f) - s->lp_r);
    left[i] = s->lp_l;
    right[i] = s->lp_r;
  }
}

static const NodeVTable GATEPITCH_VT = {.process = gatepitch_process,
                                        .init = gatepitch_init};
static const NodeVTable MOD_VT = {.process = mod_process};
static const NodeVTable SYNTH_VT = {.process = synth_process};

static int add_custom(LiveGraph *lg, NodeVTable vt, size_t state_size,
                      const char *name, int nIn, int nOut,
                      const void *init_state) {
  int nid = atomic_fetch_add(&lg->next_node_id, 1);
  int r =
      apply_add_node(lg, vt, state_size, (uint64_t)nid, name, nIn, nOut, init_state);
  if (r < 0)
    add_failed_id(lg, nid);
  return r;
}

int main(void) {
  int voices = env_int("AUDIOGRAPH_POLY_STRESS_VOICES", DEFAULT_VOICES);
  int blocks = env_int("AUDIOGRAPH_POLY_STRESS_BLOCKS", DEFAULT_BLOCKS);
  int workers = env_int("AUDIOGRAPH_POLY_STRESS_WORKERS", 7);
  int timeout_s = env_int("AUDIOGRAPH_POLY_STRESS_TIMEOUT", 60);

  signal(SIGALRM, timeout_handler);
  alarm(timeout_s);

  initialize_engine(BLOCK_SIZE, SAMPLE_RATE);
  engine_start_workers(workers);

  LiveGraph *lg = create_live_graph(voices * 8 + 64, BLOCK_SIZE,
                                    "polysynth_scheduler_stress", 2);
  assert(lg != NULL);

  int voice_sum_l = live_add_gain(lg, 1.0f, "voice_sum_l");
  int voice_sum_r = live_add_gain(lg, 1.0f, "voice_sum_r");
  int track_l = live_add_gain(lg, 0.9f, "track_l");
  int track_r = live_add_gain(lg, 0.9f, "track_r");
  assert(voice_sum_l >= 0 && voice_sum_r >= 0 && track_l >= 0 && track_r >= 0);

  for (int v = 0; v < voices; v++) {
    int gp = add_custom(lg, GATEPITCH_VT, sizeof(GatePitchState), "gp", 0, 4, &v);
    int mod = add_custom(lg, MOD_VT, sizeof(ModState), "mod", 4, MOD_OUTS, NULL);
    int synth = add_custom(lg, SYNTH_VT, sizeof(SynthState), "synth", SYNTH_INS, 2, NULL);
    int route_l = live_add_gain(lg, 1.0f, "route_l");
    int route_r = live_add_gain(lg, 1.0f, "route_r");
    assert(gp >= 0 && mod >= 0 && synth >= 0 && route_l >= 0 && route_r >= 0);

    for (int p = 0; p < 4; p++) {
      assert(apply_connect_internal(lg, gp, p, synth, p));
      assert(apply_connect_internal(lg, gp, p, mod, p));
    }
    for (int m = 0; m < MOD_OUTS; m++) {
      assert(apply_connect_internal(lg, mod, m, synth, 4 + m));
    }
    assert(apply_connect_internal(lg, synth, 0, route_l, 0));
    assert(apply_connect_internal(lg, synth, 1, route_r, 0));
    assert(apply_connect_internal(lg, route_l, 0, voice_sum_l, 0));
    assert(apply_connect_internal(lg, route_r, 0, voice_sum_r, 0));
  }

  assert(apply_connect_internal(lg, voice_sum_l, 0, track_l, 0));
  assert(apply_connect_internal(lg, voice_sum_r, 0, track_r, 0));
  assert(apply_connect_internal(lg, track_l, 0, lg->dac_node_id, 0));
  assert(apply_connect_internal(lg, track_r, 0, lg->dac_node_id, 1));
  update_orphaned_status(lg);

  float *out = calloc((size_t)BLOCK_SIZE * 2, sizeof(float));
  assert(out != NULL);

  double worst_ms = 0.0;
  double start = now_ms();
  for (int b = 0; b < blocks; b++) {
    double t0 = now_ms();
    process_next_block(lg, out, BLOCK_SIZE);
    double elapsed = now_ms() - t0;
    if (elapsed > worst_ms)
      worst_ms = elapsed;
    if (elapsed > 500.0) {
      fprintf(stderr,
              "polysynth stress block %d took %.3f ms (voices=%d workers=%d)\n",
              b, elapsed, voices, workers);
      return 1;
    }
    if ((b % 1000) == 0) {
      float probe = out[(b % BLOCK_SIZE) * 2];
      printf("block=%d elapsed=%.3fms probe=%.6f\n", b, elapsed, probe);
    }
  }

  double total = now_ms() - start;
  printf("polysynth scheduler stress passed: voices=%d workers=%d blocks=%d total=%.1fms worst=%.3fms nodes=%d\n",
         voices, workers, blocks, total, worst_ms, lg->node_count);

  free(out);
  destroy_live_graph(lg);
  engine_stop_workers();
  alarm(0);
  return 0;
}
