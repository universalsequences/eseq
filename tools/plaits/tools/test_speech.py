import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import subprocess
import numpy as np
import plh
# NOTE: use retrig values that are exact binary fractions (0.25, 0.5, 1.0):
# plh's float32 t/retrig can floor one block early (e.g. 0.4), retriggering
# the port one block after the reference.
# Deterministic-noise variant: with u >= 0 the C++ LPC noise sign is
# constant (tools/plref_speech, Random::GetSample() = u > 0.5 ? 1 : -1) and
# the port's (noise) is replaced by the same constant.
REF_S = os.path.join(plh.ROOT, "tools", "plref_speech", "plref_speech")
if not os.path.exists(REF_S):
    subprocess.run(["bash", os.path.join(plh.ROOT, "tools", "plref_speech", "build.sh")], check=True)
DET = os.path.join(plh.WORK, "tests", "speech", "speech_det.lisp")
os.makedirs(os.path.dirname(DET), exist_ok=True)
src = open(os.path.join(plh.ROOT, "engines", "speech.lisp")).read()
assert "(gt (noise) 0.5)" in src
open(DET, "w").write(src.replace("(gt (noise) 0.5)", "(gt (gswitch (gte p_u 0) p_u (noise)) 0.5)"))

class SpeechTest(plh.EngineTest):
    u = -1.0
    def ref(self, **kw):
        path = os.path.join(self.dir, "ref.raw")
        env = dict(os.environ)
        exe = plh.REF
        if self.u >= 0:
            exe = REF_S
            env["PLREF_U"] = repr(float(self.u))
        subprocess.run([exe, path, "mode=engine"] + [f"{k}={v}" for k, v in kw.items()], check=True, env=env)
        return np.fromfile(path, dtype=np.float32).reshape(-1, 2)
WRAP = """
(defmacro speech-test (bstart bphase rise high patched note timbre morph harmonics accent prosody speed)
  (def (o a e) (plaits-speech bstart bphase rise high patched note timbre morph harmonics accent prosody speed))
  (tuple o a))
"""
t = SpeechTest("speech", [DET],
    "(speech-test bstart bphase rise high patched note timbre morph harmonics accent p_prosody p_speed)",
    data_files=["data/speech_naive.json", "data/speech_sam.json", "data/speech_frames.json",
                "data/speech_words.json", "data/lut_lpc_excitation_pulse.json"],
    extra_params=["(param p_u @default -1 @min -1 @max 1)", "(param p_prosody @default 0 @min 0 @max 1)", "(param p_speed @default 0 @min 0 @max 1)"],
    extra_defs=WRAP)

def cmp(label, prosody=0.0, speed=0.0, u=-1.0, **kw):
    t.u = u
    return t.compare(label + (" [det]" if u >= 0 else ""), port={"p_prosody": prosody, "p_speed": speed, "p_u": u},
                     prosody=prosody, speed=speed, engine=15, **kw)

def bank_h(b):  # HARMONICS in the middle of word bank b's quantizer step
    return min((2 + (b + 1.5) / 1.65) / 6, 1.0)

which = sys.argv[1:] or ["naive", "sam", "lpc", "words"]
if "naive" in which:
    for h, tb, m in [(0.0, 0.5, 0.5), (0.0, 0.0, 0.0), (0.0, 1.0, 1.0), (0.0, 0.3, 0.8),
                     (1 / 12, 0.5, 0.5), (1 / 12, 0.8, 0.2)]:
        cmp(f"h{h:.3f} t{tb} m{m}", note=48, harmonics=h, timbre=tb, morph=m)
    cmp("naive sweep", note=36, note_end=72, harmonics=0, timbre=0, timbre_end=1,
        morph=1, morph_end=0, seconds=3)
if "sam" in which:
    for h, tb, m in [(1 / 6, 0.5, 0.5), (1 / 6, 0.0, 0.0), (1 / 6, 1.0, 1.0), (1 / 6, 0.2, 0.7)]:
        cmp(f"h{h:.3f} t{tb} m{m}", note=48, harmonics=h, timbre=tb, morph=m)
    cmp("sam sweep", note=36, note_end=84, harmonics=1 / 6, timbre=0, timbre_end=1,
        morph=0, morph_end=1, seconds=3)
if "lpc" in which:
    for h, tb, m in [(1 / 3, 0.5, 0.5), (1 / 3, 0.0, 0.0), (1 / 3, 1.0, 1.0), (0.25, 0.4, 0.7)]:
        cmp(f"h{h:.3f} t{tb} m{m}", note=48, harmonics=h, timbre=tb, morph=m, trig=0)
    cmp("lpc vowel trig", note=48, harmonics=1 / 3, timbre=0.5, morph=0.3, gate=0.05, retrig=0.25)
    # consonant frame 5 (unvoiced, noise) on each trigger
    cmp("lpc noise consonant", note=48, harmonics=1 / 3, timbre=0.2, morph=0.7, gate=0.05, retrig=0.25, u=0.9)
    cmp("lpc noise consonant", note=48, harmonics=1 / 3, timbre=0.2, morph=0.7, gate=0.05, retrig=0.25)
    cmp("lpc vowel sweep", note=36, note_end=72, harmonics=1 / 3, timbre=0.2, timbre_end=0.8,
        morph=0, morph_end=1, seconds=3, trig=0)
if "words" in which:
  for u in (0.9, -1.0):
      for b in range(5):
          cmp(f"bank{b} trig", note=48, harmonics=bank_h(b), timbre=0.5, morph=0.3,
              gate=0.5, retrig=1.0, seconds=2, u=u)
      cmp("bank2 m0.9 t0.2", note=55, harmonics=bank_h(2), timbre=0.2, morph=0.9, gate=0.5, retrig=1.0, u=u)
      cmp("bank4 prosody/speed", note=48, harmonics=1.0, timbre=0.5, morph=0.5,
          prosody=0.7, speed=0.6, gate=0.5, retrig=1.5, seconds=3, u=u)
      cmp("bank0 unpatched scan", note=48, harmonics=0.5, timbre=0.5, morph=0, morph_end=1,
          trig=0, seconds=3, u=u)
      cmp("harm sweep trig", note=48, harmonics=0, harmonics_end=1, timbre=0.5, morph=0.5,
          gate=0.1, retrig=0.5, seconds=4, u=u)
if "cpu" in which:
    t.cpu(harmonics=0.1)
    t.cpu(harmonics=0.8)
