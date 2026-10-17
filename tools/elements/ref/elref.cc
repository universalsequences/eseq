// Reference renderer for the Elements port: drives elements::Part at its
// native 32 kHz in 16-sample blocks (kAudioChunkSize) and writes raw float32
// frames with NCH channels:
//   0 main  1 aux  (Part outputs, before the codec's SoftConvert)
//   2 raw   3 center  4 sides   (the active voice's buffers)
//   5 bow   6 blow   7 strike   (exciter buffers after Voice::Process)
//   8 envelope value (per block, Voice::envelope_value_ at block end)
//
// usage: elref out.raw key=value ...
//   seconds=2 gate=1 (seconds high; <0 = never) retrig=0 (period, s)
//   note=48 strength=0.5 modulation=0 model=0 (0 modal 1 string 2 strings)
//   easter=0
//   any Patch field by name (contour bow bow_timbre blow blow_meta
//   blow_timbre strike strike_meta strike_timbre signature geometry
//   brightness damping position space mod_freq mod_offset rev_diffusion
//   rev_lp), and <key>_end=<v> for a linear sweep over the render (note too).
//   ELREF_DET=1 in the environment: deterministic random sequence.
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>

#define private public
#include "elements/dsp/part.h"
#undef private
#include "elements/resources.h"

using namespace elements;
using namespace stmlib;

static std::map<std::string, float> kv;
static float arg(const char* k, float d) {
  auto it = kv.find(k);
  return it == kv.end() ? d : it->second;
}

static uint16_t reverb_buffer[32768];
static Part part;

int main(int argc, char** argv) {
  if (argc < 2) return 1;
  for (int i = 2; i < argc; ++i) {
    const char* eq = strchr(argv[i], '=');
    if (!eq) continue;
    kv[std::string(argv[i], eq - argv[i])] = atof(eq + 1);
  }
  part.Init(reverb_buffer);
  Patch* p = part.mutable_patch();
  struct F { const char* name; float* ptr; };
  F fields[] = {
    {"contour", &p->exciter_envelope_shape},
    {"bow", &p->exciter_bow_level},
    {"bow_timbre", &p->exciter_bow_timbre},
    {"blow", &p->exciter_blow_level},
    {"blow_meta", &p->exciter_blow_meta},
    {"blow_timbre", &p->exciter_blow_timbre},
    {"strike", &p->exciter_strike_level},
    {"strike_meta", &p->exciter_strike_meta},
    {"strike_timbre", &p->exciter_strike_timbre},
    {"signature", &p->exciter_signature},
    {"geometry", &p->resonator_geometry},
    {"brightness", &p->resonator_brightness},
    {"damping", &p->resonator_damping},
    {"position", &p->resonator_position},
    {"mod_freq", &p->resonator_modulation_frequency},
    {"mod_offset", &p->resonator_modulation_offset},
    {"rev_diffusion", &p->reverb_diffusion},
    {"rev_lp", &p->reverb_lp},
    {"space", &p->space},
  };
  const int nf = sizeof(fields) / sizeof(fields[0]);
  float start[nf], end[nf];
  bool sweep[nf];
  for (int i = 0; i < nf; ++i) {
    start[i] = arg(fields[i].name, *fields[i].ptr);
    std::string e = std::string(fields[i].name) + "_end";
    sweep[i] = kv.count(e) > 0;
    end[i] = sweep[i] ? kv[e] : start[i];
    *fields[i].ptr = start[i];
  }
  part.set_resonator_model(ResonatorModel(int(arg("model", 0))));
  part.set_easter_egg(arg("easter", 0) > 0.5f);

  // The module runs continuously, so a note never meets the resonator's
  // power-on coefficients (Svf::Init) in its clock-divided upper modes:
  // settle them for the first note's settings before rendering.
  {
    Resonator& res = part.voice_[0].resonator_;
    float n0 = arg("note", 48.0f) + 48.0f;
    int32_t pi = static_cast<int32_t>(n0 * 256.0f);
    if (pi < 0) pi = 0;
    if (pi > 65535) pi = 65535;
    res.set_frequency(lut_midi_to_f_high[pi >> 8] * lut_midi_to_f_low[pi & 0xff]);
    res.set_geometry(p->resonator_geometry);
    res.set_brightness(p->resonator_brightness);
    res.set_damping(p->resonator_damping);
    res.ComputeFilters();
    res.ComputeFilters();
  }
  const float sr = 32000.0f;
  const size_t kBlock = 16;
  float seconds = arg("seconds", 2.0f);
  float gate_s = arg("gate", 1.0f);
  float retrig = arg("retrig", 0.0f);
  float note0 = arg("note", 48.0f);
  float note1 = kv.count("note_end") ? kv["note_end"] : note0;
  size_t n = size_t(seconds * sr) / kBlock * kBlock;
  const int NCH = 9;
  float* outbuf = new float[n * NCH];
  float zeros[kBlock] = {0};
  float main_[kBlock], aux_[kBlock];
  for (size_t b = 0; b < n; b += kBlock) {
    float frac = float(b) / float(n);
    for (int i = 0; i < nf; ++i) {
      if (sweep[i]) *fields[i].ptr = start[i] + (end[i] - start[i]) * frac;
    }
    float t = b / sr;
    float tr = retrig > 0 ? t - retrig * floorf(t / retrig) : t;
    PerformanceState s;
    s.gate = gate_s >= 0 && tr < gate_s;
    s.note = note0 + (note1 - note0) * frac;
    s.modulation = arg("modulation", 0.0f);
    s.strength = arg("strength", 0.5f);
    part.Process(s, zeros, zeros, main_, aux_, kBlock);
    Voice& v = part.voice_[0];
    for (size_t j = 0; j < kBlock; ++j) {
      float* o = &outbuf[(b + j) * NCH];
      o[0] = main_[j];
      o[1] = aux_[j];
      o[2] = part.raw_buffer_[j];
      o[3] = part.center_buffer_[j];
      o[4] = part.sides_buffer_[j];
      o[5] = v.bow_buffer_[j];
      o[6] = v.blow_buffer_[j];
      o[7] = v.strike_buffer_[j];
      o[8] = getenv("ELREF_PROBE_DAMP") ? v.strike_.damping() : v.envelope_value_;
    }
  }
  FILE* fp = fopen(argv[1], "wb");
  fwrite(outbuf, sizeof(float), n * NCH, fp);
  fclose(fp);
  return 0;
}
