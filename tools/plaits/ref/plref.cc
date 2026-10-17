// Reference renderer for the Plaits port. Renders either one raw engine
// (mode=engine) or the full Voice (mode=voice) to raw float32 frames
// (out, aux) at 48 kHz.
//
// usage: plref out.raw key=value ...
//   mode=engine|voice   engine=0..23 (Plaits engine index)
//   seconds=2  gate=1.0 (seconds the trigger/gate stays high; <0 = never)
//   retrig=0 (seconds between re-triggers, 0 = single)
//   note=48 harmonics=0.5 timbre=0.5 morph=0.5 accent=0.8
//   *_end=<v> for note/harmonics/timbre/morph: linear sweep over the render
//   engine mode: trig=1 (trigger patched) | 0 (TRIGGER_UNPATCHED)
//   voice mode: decay=0.5 lpg=0.5 fm_amt=0 timbre_amt=0 morph_amt=0
//               trig=1 level_patched=0 level=1 (level CV while gate high)
//   speech: prosody=0 speed=0 ; chiptune: env_shape=-1 (NO_ENVELOPE)
//   voice-mode output is the Voice's int16 frames / 32768.
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>

#include "plaits/dsp/voice.h"
#include "plaits/resources.h"

using namespace plaits;
using namespace stmlib;

static char ram[65536];

static std::map<std::string, float> kv;
static float arg(const char* k, float d) {
  auto it = kv.find(k);
  return it == kv.end() ? d : it->second;
}
static std::string mode = "engine";

struct Engines {
  VirtualAnalogVCFEngine vavcf;
  PhaseDistortionEngine pd;
  SixOpEngine six;
  WaveTerrainEngine terrain;
  StringMachineEngine strmach;
  ChiptuneEngine chip;
  VirtualAnalogEngine va;
  WaveshapingEngine ws;
  FMEngine fm;
  GrainEngine grain;
  AdditiveEngine add;
  WavetableEngine wt;
  ChordEngine chord;
  SpeechEngine speech;
  SwarmEngine swarm;
  NoiseEngine noise;
  ParticleEngine particle;
  StringEngine string;
  ModalEngine modal;
  BassDrumEngine bd;
  SnareDrumEngine sd;
  HiHatEngine hh;
};

int main(int argc, char** argv) {
  if (argc < 2) { fprintf(stderr, "usage: plref out.raw k=v...\n"); return 1; }
  for (int i = 2; i < argc; ++i) {
    char* eq = strchr(argv[i], '=');
    if (!eq) continue;
    std::string k(argv[i], eq - argv[i]);
    if (k == "mode") { mode = eq + 1; continue; }
    kv[k] = atof(eq + 1);
  }
  const float seconds = arg("seconds", 2.0f);
  const float gate_s = arg("gate", 1.0f);
  const float retrig = arg("retrig", 0.0f);
  const size_t total = size_t(seconds * 48000.0f);
  const size_t kB = kBlockSize;
  FILE* fp = fopen(argv[1], "wb");
  float note0 = arg("note", 48), note1 = arg("note_end", note0);
  float h0 = arg("harmonics", 0.5f), h1 = arg("harmonics_end", h0);
  float t0 = arg("timbre", 0.5f), t1 = arg("timbre_end", t0);
  float m0 = arg("morph", 0.5f), m1 = arg("morph_end", m0);
  auto lerp = [&](float a, float b, size_t n) { return a + (b - a) * float(n) / float(total); };
  auto gate_at = [&](size_t n) {
    float t = n / 48000.0f;
    if (gate_s < 0) return false;
    if (retrig > 0) t = fmodf(t, retrig);
    return t < gate_s;
  };

  if (mode == "engine") {
    Engines* e = new Engines();
    Engine* table[24] = { &e->vavcf, &e->pd, &e->six, &e->six, &e->six,
      &e->terrain, &e->strmach, &e->chip, &e->va, &e->ws, &e->fm, &e->grain,
      &e->add, &e->wt, &e->chord, &e->speech, &e->swarm, &e->noise,
      &e->particle, &e->string, &e->modal, &e->bd, &e->sd, &e->hh };
    int index = int(arg("engine", 8));
    BufferAllocator allocator(ram, sizeof(ram));
    Engine* eng = table[index];
    eng->Init(&allocator);
    eng->LoadUserData(index >= 2 && index <= 4 ? fm_patches_table[index - 2] : NULL);
    eng->Reset();
    e->speech.set_prosody_amount(arg("prosody", 0));
    e->speech.set_speed(arg("speed", 0));
    e->chip.set_envelope_shape(arg("env_shape", ChiptuneEngine::NO_ENVELOPE));
    bool patched = arg("trig", 1) > 0.5f;
    bool prev = false;
    float out[kB], aux[kB], frame[2 * kB];
    for (size_t n = 0; n < total; n += kB) {
      EngineParameters p;
      bool g = gate_at(n);
      p.trigger = patched ? ((g && !prev ? TRIGGER_RISING_EDGE : 0) | (g ? TRIGGER_HIGH : 0))
                          : TRIGGER_UNPATCHED;
      prev = g;
      p.note = lerp(note0, note1, n);
      p.harmonics = lerp(h0, h1, n);
      p.timbre = lerp(t0, t1, n);
      p.morph = lerp(m0, m1, n);
      p.accent = arg("accent", 0.8f);
      bool env = false;
      eng->Render(p, out, aux, kB, &env);
      for (size_t i = 0; i < kB; ++i) { frame[2 * i] = out[i]; frame[2 * i + 1] = aux[i]; }
      fwrite(frame, sizeof(float), 2 * kB, fp);
    }
  } else {
    Voice* voice = new Voice();
    BufferAllocator allocator(ram, sizeof(ram));
    voice->Init(&allocator);
    Patch patch;
    memset(&patch, 0, sizeof(patch));
    Modulations mods;
    memset(&mods, 0, sizeof(mods));
    patch.engine = int(arg("engine", 8));
    patch.decay = arg("decay", 0.5f);
    patch.lpg_colour = arg("lpg", 0.5f);
    patch.frequency_modulation_amount = arg("fm_amt", 0);
    patch.timbre_modulation_amount = arg("timbre_amt", 0);
    patch.morph_modulation_amount = arg("morph_amt", 0);
    mods.trigger_patched = arg("trig", 1) > 0.5f;
    mods.level_patched = arg("level_patched", 0) > 0.5f;
    Voice::Frame frames[kB];
    float frame[2 * kB];
    for (size_t n = 0; n < total; n += kB) {
      bool g = gate_at(n);
      patch.note = lerp(note0, note1, n);
      patch.harmonics = lerp(h0, h1, n);
      patch.timbre = lerp(t0, t1, n);
      patch.morph = lerp(m0, m1, n);
      mods.trigger = g ? 1.0f : 0.0f;
      // level_delay (samples) lines LEVEL up with the delayed trigger.
      size_t ld = size_t(arg("level_delay", 0));
      bool gl = n >= ld && gate_at(n - ld);
      mods.level = gl ? arg("level", 1.0f) : 0.0f;
      voice->Render(patch, mods, frames, kB);
      for (size_t i = 0; i < kB; ++i) {
        frame[2 * i] = frames[i].out / 32768.0f;
        frame[2 * i + 1] = frames[i].aux / 32768.0f;
      }
      fwrite(frame, sizeof(float), 2 * kB, fp);
    }
  }
  fclose(fp);
  return 0;
}
