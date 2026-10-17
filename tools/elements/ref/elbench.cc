// CPU benchmark of the original module: drives elements::Part at its native
// 32 kHz in 16-sample blocks, one note (gate `gate` seconds every `retrig`
// seconds). tools/bench.py runs it under /usr/bin/time -l and counts its
// cycles; the wall-clock % of one core it prints is informational.
//
// usage: elbench seconds key=value ...
//   note=48 strength=0.5 model=0 (0 modal 1 string 2 strings) easter=0
//   gate=0.5 (<0 = never) retrig=1 and any Patch field by name (contour bow
//   bow_timbre blow blow_meta blow_timbre strike strike_meta strike_timbre
//   geometry brightness damping position space)
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>

#include "elements/dsp/part.h"

using namespace elements;

static std::map<std::string, float> kv;
static float arg(const char* k, float d) {
  auto it = kv.find(k);
  return it == kv.end() ? d : it->second;
}

static uint16_t reverb_buffer[32768];
static Part part;

int main(int argc, char** argv) {
  if (argc < 2) return 1;
  float seconds = atof(argv[1]);
  for (int i = 2; i < argc; ++i) {
    const char* eq = strchr(argv[i], '=');
    if (eq) kv[std::string(argv[i], eq - argv[i])] = atof(eq + 1);
  }
  part.Init(reverb_buffer);
  Patch* p = part.mutable_patch();
  struct F { const char* name; float* ptr; } fields[] = {
    {"contour", &p->exciter_envelope_shape}, {"bow", &p->exciter_bow_level},
    {"bow_timbre", &p->exciter_bow_timbre}, {"blow", &p->exciter_blow_level},
    {"blow_meta", &p->exciter_blow_meta}, {"blow_timbre", &p->exciter_blow_timbre},
    {"strike", &p->exciter_strike_level}, {"strike_meta", &p->exciter_strike_meta},
    {"strike_timbre", &p->exciter_strike_timbre}, {"geometry", &p->resonator_geometry},
    {"brightness", &p->resonator_brightness}, {"damping", &p->resonator_damping},
    {"position", &p->resonator_position}, {"space", &p->space},
  };
  for (auto& f : fields) *f.ptr = arg(f.name, *f.ptr);
  // The instrument's unseeded defaults (shell/header.lisp).
  p->resonator_modulation_frequency = 0.5f / 32000.0f;
  p->resonator_modulation_offset = 0.1f;
  p->reverb_diffusion = 0.625f;
  p->reverb_lp = 0.7f;
  p->exciter_signature = 0.0f;
  part.set_resonator_model(ResonatorModel(int(arg("model", 0))));
  part.set_easter_egg(arg("easter", 0) > 0.5f);

  const float sr = 32000.0f;
  const size_t kBlock = 16;
  const size_t n = size_t(seconds * sr) / kBlock * kBlock;
  const float gate_s = arg("gate", 0.5f);
  const float retrig = arg("retrig", 1.0f);
  const float note = arg("note", 48.0f);
  const float strength = arg("strength", 0.5f);
  float zeros[kBlock] = {0};
  float main_[kBlock], aux_[kBlock];
  double energy = 0.0;
  auto t0 = std::chrono::steady_clock::now();
  for (size_t b = 0; b < n; b += kBlock) {
    float t = b / sr;
    float tr = retrig > 0 ? t - retrig * floorf(t / retrig) : t;
    PerformanceState s;
    s.gate = gate_s >= 0 && tr < gate_s;
    s.note = note;
    s.modulation = 0.0f;
    s.strength = strength;
    part.Process(s, zeros, zeros, main_, aux_, kBlock);
    for (size_t j = 0; j < kBlock; ++j) energy += double(main_[j]) * main_[j];
  }
  double elapsed = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
  printf("%.4f\n", elapsed / seconds * 100.0);
  fprintf(stderr, "rms %.5f\n", n ? sqrt(energy / n) : 0.0);
  return 0;
}
