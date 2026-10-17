// CPU benchmark driver for a compiled DGen patch (patch.dylib): runs
// dgen_process_v1 on a prepared state array with the same note pattern as
// ref/elbench (gate 0.5 s every 1 s, a trigger pulse at each onset).
// tools/bench.py builds it and counts its cycles; the wall-clock % of one
// core it prints is informational.
//
// usage: dgdrv patch.dylib mem.raw n_in n_out gate_ch pitch_ch vel_ch trig_ch
//              pitch_hz sample_rate block seconds idle
// (vel_ch / trig_ch -1 = absent; idle 1 = no notes)
#include <dlfcn.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

typedef struct { uint32_t abi_version, struct_size; float sample_rate; uint32_t reserved; } Context;
typedef void (*Process)(float**, float**, uint32_t, void*, Context*, void*);

int main(int argc, char** argv) {
  if (argc < 14) return 1;
  void* lib = dlopen(argv[1], RTLD_NOW);
  if (!lib) { fprintf(stderr, "%s\n", dlerror()); return 1; }
  Process process = (Process)dlsym(lib, "dgen_process_v1");
  FILE* f = fopen(argv[2], "rb");
  if (!f) return 1;
  fseek(f, 0, SEEK_END);
  long bytes = ftell(f);
  rewind(f);
  float* mem = malloc(bytes);
  if (fread(mem, 1, bytes, f) != (size_t)bytes) return 1;
  fclose(f);
  int n_in = atoi(argv[3]), n_out = atoi(argv[4]);
  int gate_ch = atoi(argv[5]), pitch_ch = atoi(argv[6]), vel_ch = atoi(argv[7]), trig_ch = atoi(argv[8]);
  float hz = atof(argv[9]), sr = atof(argv[10]);
  int block = atoi(argv[11]);
  double seconds = atof(argv[12]);
  int idle = atoi(argv[13]);
  if (n_in > 16 || n_out > 16) return 1;
  float* in[16];
  float* out[16];
  for (int i = 0; i < n_in; ++i) in[i] = calloc(block, sizeof(float));
  for (int i = 0; i < n_out; ++i) out[i] = calloc(block, sizeof(float));
  for (int j = 0; j < block; ++j) {
    in[pitch_ch][j] = hz;
    if (vel_ch >= 0) in[vel_ch][j] = 1.0f;
  }
  Context ctx = {1, sizeof(Context), sr, 0};
  long n = (long)(seconds * sr) / block * block, period = (long)sr, phase = 0;
  int report = getenv("DGDRV_RMS") != NULL;
  double energy = 0.0;
  struct timespec t0, t1;
  clock_gettime(CLOCK_MONOTONIC, &t0);
  for (long b = 0; b < n; b += block) {
    // Running phase, wrapped once per sample with a compare (no division in
    // the timed loop; elbench sets its gate once per 16-sample block).
    for (int j = 0; j < block; ++j) {
      in[gate_ch][j] = !idle && phase < period / 2;
      if (trig_ch >= 0) in[trig_ch][j] = !idle && phase == 0;
      if (++phase == period) phase = 0;
    }
    process(in, out, block, mem, &ctx, NULL);
    if (report) for (int j = 0; j < block; ++j) energy += out[n_out > 1][j] * out[n_out > 1][j];
  }
  clock_gettime(CLOCK_MONOTONIC, &t1);
  double elapsed = (t1.tv_sec - t0.tv_sec) + 1e-9 * (t1.tv_nsec - t0.tv_nsec);
  printf("%.4f\n", seconds > 0 ? elapsed / seconds * 100.0 : 0.0);
  fprintf(stderr, "rms %.5f\n", n ? sqrt(energy / n) : 0.0);
  return 0;
}
