// Renders the original Beads Processor (granular / delay audio processing) to
// raw float32 stereo, fed from a raw float32 stereo input file.
// args: in.raw out.raw media time time_amt size size_amt shape shape_amt
//       pitch pitch_amt density feedback dry_wet reverb freeze seed
// media: 0 dirty tape, 1 clean tape, 2 vintage digital, 3 hi-fi (Processor::Media).
// seed: 0 off, 1 held, 2 locked; env SEED_AT="t1,t2,..." taps the button (s)
// for one block; env FREEZE_AT="on,off" holds FREEZE between two times (s).
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>
#include "beads/dsp/processor.h"
using namespace beads;
using namespace stmlib;

static uint8_t bank0[256 * 1024], bank1[256 * 1024], bank2[256 * 1024];
static uint16_t reverb_buffer[32768];

int main(int argc, char** argv) {
  if (argc < 18) { fprintf(stderr, "bad args\n"); return 1; }
  FILE* fi = fopen(argv[1], "rb");
  std::vector<FloatFrame> in;
  FloatFrame f;
  while (fread(&f, sizeof(f), 1, fi) == 1) in.push_back(f);
  fclose(fi);
  float a[15];
  for (int i = 0; i < 15; ++i) a[i] = atof(argv[3 + i]);
  uint8_t* buffers[3] = { bank0, bank1, bank2 };
  Processor* p = new Processor();
  p->Init(buffers, reverb_buffer);
  p->Configure(Processor::Media(int(a[0])), false);
  Parameters P;
  P.time.base = a[1]; P.time.attenuverter = a[2]; P.time.cv = 0; P.time.has_cv = false;
  P.size.base = a[3]; P.size.attenuverter = a[4]; P.size.cv = 0; P.size.has_cv = false;
  P.shape.base = a[5]; P.shape.attenuverter = a[6]; P.shape.cv = 0; P.shape.has_cv = false;
  P.pitch.base = a[7]; P.pitch.attenuverter = a[8]; P.pitch.cv = 0; P.pitch.has_cv = false;
  P.monophonic_mode = false;
  ModulatedParameter d; d.base = a[9]; d.cv = 0;
  Processor::MixParameters m;
  m.feedback = a[10]; m.dry_wet = a[11]; m.reverb_amount = a[12]; m.output_gate = false;
  const bool frz = a[13] > 0.5f;
  const int seed = int(a[14]);
  std::vector<size_t> taps;
  if (getenv("SEED_AT")) {
    char* s = strdup(getenv("SEED_AT"));
    for (char* t = strtok(s, ","); t; t = strtok(NULL, ",")) taps.push_back(size_t(atof(t) * p->sample_rate()));
  }
  // FREEZE_AT="on,off": freeze held between the two times (s).
  float f_on = -1, f_off = -1;
  if (getenv("FREEZE_AT")) sscanf(getenv("FREEZE_AT"), "%f,%f", &f_on, &f_off);
  const size_t kBlock = size_t(p->sample_rate() / 4000);
  FILE* fo = fopen(argv[2], "wb");
  bool freeze[64]; FloatFrame out[64];
  for (size_t n = 0; n + kBlock <= in.size(); n += kBlock) {
    for (size_t i = 0; i < kBlock; ++i) {
      float t = float(n + i) / p->sample_rate();
      freeze[i] = frz || (f_on >= 0 && t >= f_on && t < f_off);
    }
    SeedButtonState st = seed == 2 ? SEED_BUTTON_STATE_LOCKED
        : (seed == 1 ? SEED_BUTTON_STATE_ON : SEED_BUTTON_STATE_OFF);
    for (size_t t : taps) if (t >= n && t < n + kBlock) st = SEED_BUTTON_STATE_ON;
    p->Process(st, NULL, freeze, d, P, m, &in[n], out, kBlock);
    fwrite(out, sizeof(FloatFrame), kBlock, fo);
  }
  fclose(fo);
  return 0;
}
