"""Standalone CPU of each module (48 kHz), driven by simple inputs."""
import os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh
A = elh.A
SRC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src")
HEAD = """
(def gate (in 1 @name gate))
(def warm_start 1)
(def pitch (in 2 @name pitch))
(make-history h_bp)
(def bphase (read-history h_bp))
(write-history h_bp (% (+ bphase 1) 16))
(def bstart (eq bphase 0))
(make-history h_bpe)
(def bphase_e (read-history h_bpe))
(write-history h_bpe (% (+ bphase_e 1) 16))
(def bstart_e (eq bphase_e 0))
(defmacro el-ev (x) (event-hold (+ x (* bstart_e 0)) bstart_e))
(def x (* (- (noise) 0.5) (lt (noise-tap-x) 0.01)))
"""
def mod(*names):
    return "".join(open(os.path.join(SRC, n + ".lisp")).read() + "\n" for n in names)
def run(name, body, mods, seconds=6.0):
    d = os.path.join(elh.WORK, "part_" + name); os.makedirs(d, exist_ok=True)
    elh.link_data(d)
    src = mod("lib") + HEAD.replace("(noise-tap-x)", "(el-noise-tap (noise))") + mod(*mods) + body
    open(os.path.join(d, "dsp.lisp"), "w").write(src)
    i = A.Instrument(d); i.render(0.2, pitch="c3")
    best = 1e9
    for _ in range(3):
        t0 = time.process_time(); i.render(seconds, pitch="c3")
        best = min(best, time.process_time() - t0)
    print(f"{name:12s} {100 * best / seconds:6.2f}%", flush=True)
F = "(def f32 (el-hold (/ pitch 32000)))\n"
run("nothing", "(out x 1 @name left)(out x 2 @name right)", [])
run("modal", F + "(def (c s) (el-resonator f32 0.2 0.5 0.25 0.3 (/ 0.5 32000) 0.1 x 0))(out c 1 @name left)(out s 2 @name right)", ["envelope", "exciter", "resonator"])
run("string1", F + "(def l0 (tensor @shape [8192]))(def a0 (tensor @shape [4096]))(def (c s) (el-string l0 a0 f32 0 0.5 0.3 0.3 x (noise)))(out c 1 @name left)(out s 2 @name right)", ["envelope", "exciter", "resonator", "string"])
run("reverb", "(def (l r) (el-reverb x x 0.4 0.2 0.8 0.625 0.7))(out l 1 @name left)(out r 2 @name right)", ["resonator", "reverb"])
run("exciters", F + """(def b (exc-bow (* bstart (gt gate 0.5)) 0.5 (noise)))
(def g (exc-diffuser (exc-tube f32 1 0.3 0.5 (exc-granular 0.5 0.5 0 (el-noise-tap (noise))) 0.2)))
(def (s d) (exc-strike 0 1 0.5 0.5 0 x x x))
(out (+ b g s d) 1 @name left)(out b 2 @name right)""", ["envelope", "exciter"])
