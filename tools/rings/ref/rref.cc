// Reference renderer for the Rings port: renders rings::Part (models 0..5)
// or rings::StringSynthPart (Disastrous Peace, models 6..11 = fx 0..5) at
// polyphony 1, the way the instrument drives it: internal exciter (IN
// unpatched), TRIG patched, V/OCT unpatched (FREQUENCY sets the pitch
// directly), to raw float32 frames (out, aux) at 48 kHz.
//
// usage: rref out.raw key=value ...
//   model=0..11  seconds=2  note=48 (the tonic, MIDI)  fm=0
//   structure brightness damping position (0..1, default 0.5)
//   *_end=<v> for note/structure/brightness/damping/position: linear sweep
//   retrig=0 (seconds between strums, 0 = a single strum at t=0)
//   RREF_U=<u> in the environment makes the noise source constant;
//   RREF_NOISE=<file> feeds the plucker and the string dispersion the
//   port's two noise streams (float32 pairs per sample).
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>

#include "rings/dsp/part.h"
#include "rings/dsp/string_synth_part.h"
#include "rings/dsp/strummer.h"

using namespace rings;

static uint16_t reverb_buffer[32768];
static Part part;
static StringSynthPart string_synth;
static Strummer strummer;

static std::map<std::string, float> kv;
static float arg(const char* k, float d) {
  auto it = kv.find(k);
  return it == kv.end() ? d : it->second;
}

int main(int argc, char** argv) {
  if (argc < 2) { fprintf(stderr, "usage: rref out.raw k=v...\n"); return 1; }
  for (int i = 2; i < argc; ++i) {
    char* eq = strchr(argv[i], '=');
    if (!eq) continue;
    kv[std::string(argv[i], eq - argv[i])] = atof(eq + 1);
  }
  const float seconds = arg("seconds", 2.0f);
  const float retrig = arg("retrig", 0.0f);
  const size_t total = size_t(seconds * 48000.0f);
  const size_t kB = kMaxBlockSize;
  const int model = int(arg("model", 0));
  auto sweep = [&](const char* k, float d, size_t n) {
    float a = arg(k, d);
    std::string ke = std::string(k) + "_end";
    float b = arg(ke.c_str(), a);
    return a + (b - a) * float(n) / float(total);
  };

  strummer.Init(0.01f, kSampleRate / kMaxBlockSize);
  part.Init(reverb_buffer);
  string_synth.Init(reverb_buffer);
  part.set_polyphony(1);
  string_synth.set_polyphony(1);
  if (model < 6) {
    part.set_model(ResonatorModel(model));
  } else {
    string_synth.set_fx(FxType(model - 6));
  }

  FILE* fp = fopen(argv[1], "wb");
  float in[kB], out[kB], aux[kB], frame[2 * kB];
  int32_t chord_state = 0;
  size_t next_strum = 0;
  for (size_t n = 0; n < total; n += kB) {
    Patch patch;
    patch.structure = std::min(std::max(sweep("structure", 0.5f, n), 0.0f), 0.9995f);
    patch.brightness = sweep("brightness", 0.5f, n);
    patch.damping = sweep("damping", 0.5f, n);
    patch.position = sweep("position", 0.5f, n);

    PerformanceState ps;
    ps.internal_exciter = true;
    ps.internal_strum = false;
    ps.internal_note = true;
    ps.note = 0.0f;
    ps.tonic = sweep("note", 48.0f, n);
    ps.fm = arg("fm", 0.0f);
    // cv_scaler.cc chord hysteresis, on the STRUCTURE pot.
    float chord = patch.structure * float(kNumChords - 1);
    float hysteresis = chord - chord_state > 0.0f ? -0.1f : +0.1f;
    chord_state = int32_t(chord + hysteresis + 0.5f);
    if (chord_state < 0) chord_state = 0;
    if (chord_state > kNumChords - 1) chord_state = kNumChords - 1;
    ps.chord = chord_state;
    ps.strum = false;
    if (n >= next_strum) {
      ps.strum = true;
      next_strum = retrig > 0 ? next_strum + size_t(retrig * 48000.0f) : size_t(-1);
    }

    std::fill(&in[0], &in[kB], 0.0f);
    stmlib::Random::sample_base_ = n;
    if (model < 6) {
      strummer.Process(in, kB, &ps);
      part.Process(ps, patch, in, out, aux, kB);
    } else {
      strummer.Process(NULL, kB, &ps);
      string_synth.Process(ps, patch, in, out, aux, kB);
    }
    for (size_t i = 0; i < kB; ++i) { frame[2 * i] = out[i]; frame[2 * i + 1] = aux[i]; }
    fwrite(frame, sizeof(float), 2 * kB, fp);
  }
  fclose(fp);
  return 0;
}
