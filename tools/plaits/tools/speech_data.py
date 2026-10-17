#!/usr/bin/env python3
"""Speech engine data: decodes the LPC word banks exactly like
LPCSpeechSynthWordBank::LoadNextWord and writes
  data/speech_frames.json   [N 12] frames (energy/256, period, k0..k9 scaled):
                            rows 0..14 = LPC phonemes, then bank 0..4 frames
  data/speech_words.json    [M 1]  word boundaries (frame index local to bank)
  data/speech_naive.json    [250 1] naive phonemes: ((p*5+r)*5+i)*2 + {0 freq, 1 amp}
  data/speech_sam.json      [108 1] SAM phonemes: (row*3+i)*2 + {0 freq, 1 amp(lut)}
  data/lut_lpc_excitation_pulse.json [640 1]
and prints the per-bank Lisp constants (frame base, num frames, num words,
word-boundary offset)."""
import json, os, re, sys
sys.path.insert(0, os.path.dirname(__file__))
import lut
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Data files live in the instrument folder (PLAITS_DATA overrides).
DATA = os.environ.get("PLAITS_DATA", os.path.join(os.path.dirname(os.path.dirname(ROOT)), "content", "instruments", "Synths", "Plaits", "data"))
SP = os.path.join(os.environ["EURORACK"], "plaits", "dsp", "speech")
SRCNOTE = "Mutable Instruments Plaits (MIT, Emilie Gillet)"

def nums(s):
    s = re.sub(r"//.*", "", s)
    return [int(v, 0) for v in re.findall(r"-?(?:0x[0-9a-fA-F]+|\d+)", s)]

def carray(src, name):
    m = re.search(re.escape(name) + r"\[[^\]]*\]\s*=\s*\{(.*?)\};", src, re.S)
    return nums(m.group(1))

ctl = open(os.path.join(SP, "lpc_speech_synth_controller.cc")).read()
L = {k: carray(ctl, "LPCSpeechSynthWordBank::" + k) for k in
     ["energy_lut_", "period_lut_", "k0_lut_", "k1_lut_", "k2_lut_", "k3_lut_",
      "k4_lut_", "k5_lut_", "k6_lut_", "k7_lut_", "k8_lut_", "k9_lut_"]}

class Bits:
    def __init__(self, data, p):
        self.d, self.p, self.avail, self.bits = data, p, 0, 0
    @staticmethod
    def rev(b):
        return int(f"{b:08b}"[::-1], 2)
    def get(self, n):
        shift = n
        if n > self.avail:
            self.bits = (self.bits << self.avail) & 0xffff
            shift -= self.avail
            self.bits |= self.rev(self.d[self.p]); self.p += 1
            self.avail += 8
        self.bits = (self.bits << shift) & 0xffff
        r = (self.bits >> 8) & 0xff
        self.bits &= 0xff
        self.avail -= n
        return r
    def flush(self):
        while self.avail:
            self.get(1)

def load_bank(data):
    frames, bounds, p = [], [], 0
    while p < len(data):
        bounds.append(len(frames))
        bs = Bits(data, p)
        fr = dict(energy=0, period=0, k=[0] * 10)
        while True:
            e = bs.get(4)
            if e == 0:
                fr["energy"] = 0
            elif e == 0xf:
                bs.flush(); break
            else:
                fr["energy"] = L["energy_lut_"][e]
                rep = bs.get(1)
                fr["period"] = L["period_lut_"][bs.get(6)]
                if not rep:
                    k = fr["k"]
                    k[0] = L["k0_lut_"][bs.get(5)]; k[1] = L["k1_lut_"][bs.get(5)]
                    k[2] = L["k2_lut_"][bs.get(4)]; k[3] = L["k3_lut_"][bs.get(4)]
                    if fr["period"]:
                        k[4] = L["k4_lut_"][bs.get(4)]; k[5] = L["k5_lut_"][bs.get(4)]
                        k[6] = L["k6_lut_"][bs.get(4)]; k[7] = L["k7_lut_"][bs.get(3)]
                        k[8] = L["k8_lut_"][bs.get(3)]; k[9] = L["k9_lut_"][bs.get(3)]
            frames.append((fr["energy"], fr["period"], list(fr["k"])))
        p = bs.p
    bounds.append(len(frames))
    return frames, bounds

def frow(energy, period, k):
    return [energy / 256.0, float(period), k[0] / 32768.0, k[1] / 32768.0] + [v / 128.0 for v in k[2:]]

def dump(name, rows, cols, data):
    with open(os.path.join(DATA, name), "w") as f:
        json.dump({"shape": [rows, cols], "source": SRCNOTE, "data": data}, f, separators=(",", ":"))

if __name__ == "__main__":
    ph = open(os.path.join(SP, "lpc_speech_synth_phonemes.cc")).read()
    pv = nums(ph[ph.index("= {"):])
    rows = [frow(pv[i * 12], pv[i * 12 + 1], pv[i * 12 + 2:i * 12 + 12]) for i in range(15)]
    words = open(os.path.join(SP, "lpc_speech_synth_words.cc")).read()
    consts, wb = [], []
    for b in range(5):
        data = carray(words, f"bank_{b}")
        fr, bounds = load_bank(data)
        assert len(fr) <= 1024 and len(bounds) - 1 <= 32
        consts.append((len(rows), len(fr), len(bounds) - 1, len(wb)))
        wb += bounds
        rows += [frow(*f) for f in fr]
    n = len(rows)
    dump("speech_frames.json", n, 12, [rows[r][c] for c in range(12) for r in range(n)])
    dump("speech_words.json", len(wb), 1, wb)
    nv = open(os.path.join(SP, "naive_speech_synth.cc")).read()
    nvv = nums(nv[nv.index("phonemes_[][kNaiveSpeechNumRegisters] = {"):nv.index("void NaiveSpeechSynth::Init")].split("= {", 1)[1])
    assert len(nvv) == 250
    dump("speech_naive.json", 250, 1, nvv)
    sm = open(os.path.join(SP, "sam_speech_synth.cc")).read()
    sv = nums(sm[sm.index("phonemes_[] = {"):sm.index("formant_amplitude_lut[] =")].split("= {", 1)[1])
    amp = [float(v) for v in re.findall(r"\d\.\d+", sm[sm.index("formant_amplitude_lut[] ="):].split("};")[0])]
    assert len(sv) == 108 and len(amp) == 16, (len(sv), len(amp))
    dump("speech_sam.json", 108, 1, [v if i % 2 == 0 else amp[v] for i, v in enumerate(sv)])
    pulse = lut.array("lut_lpc_excitation_pulse")
    assert len(pulse) == 640
    dump("lut_lpc_excitation_pulse.json", 640, 1, pulse)
    print("frames", n, "words", len(wb))
    for b, c in enumerate(consts):
        print(f"bank {b}: base {c[0]} frames {c[1]} words {c[2]} wb_off {c[3]}")
