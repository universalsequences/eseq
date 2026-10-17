#!/usr/bin/env python3
"""Writes src/strings.lisp: the Rings string models (part.cc
RenderStringVoice, string.cc, plucker.h) at polyphony 1, from the
templates in tools/*.lisp.in (one String macro with the dispersion path for
string 0, one without for the 7 sympathetic strings)."""
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
T = os.path.join(ROOT, "tools")
head = open(os.path.join(T, "strings_head.lisp.in")).read()
body = open(os.path.join(T, "string_body.lisp.in")).read()
disp = open(os.path.join(T, "string_disp.lisp.in")).read()

plain = (body.replace("@NAME@", "rg-string-plain").replace("@DOC@", "")
         .replace("@APARG@", "").replace("@UARG@", "").replace("@DISPARG@", "")
         .replace("@NFARG@", "").replace("@DISP@", "  (def s_read (rg-hermite line c dl))"))
withd = (body.replace("@NAME@", "rg-string-disp")
         .replace("@DOC@", " `disp` = 1 enables the dispersion path\n; (allpass stretch line `ap`, noise `u`, curved bridge).")
         .replace("@APARG@", "ap ").replace("@UARG@", "u ").replace("@DISPARG@", "disp ")
         .replace("@NFARG@", "nfilt dispersion").replace("@DISP@", disp.rstrip("\n")))

LFO = [0.5, 0.4, 0.35, 0.23, 0.211, 0.2, 0.171, 0.0]
w = []
w.append("""
; Per-block state of sympathetic string k (k = 1..7): its gliding
; frequency (String::set_frequency(f, glide)) and its position LFO
; (CosineOscillator, one step per block). Both restart from their Init
; values while the string models are not selected.
(defmacro rg-sym-state (target glide lfo_coef bstart active)
  (make-history h_f)
  (make-history h_y0)
  (make-history h_y1)
  (def f_prev (+ 0.0045833333 (read-history h_f)))
  (def f_new (gswitch bstart (+ f_prev (* glide (- target f_prev))) f_prev))
  (write-history h_f (* active (- f_new 0.0045833333)))
  (def y0 (+ 0.5 (read-history h_y0)))
  (def y1 (+ (* lfo_coef 0.25) (read-history h_y1)))
  (def y0n (gswitch bstart (- (* lfo_coef y0) y1) y0))
  (def y1n (gswitch bstart y0 y1))
  (write-history h_y0 (* active (- y0n 0.5)))
  (write-history h_y1 (* active (- y1n (* lfo_coef 0.25))))
  (tuple (latch f_new bstart) (latch (+ y1n 0.5) bstart)))

; The string models. tonic/structure/brightness/damping/position/chord are
; block-latched; u0, u1 are uniform noise draws. Returns (out aux).
(defmacro rg-strings (bstart strum m f0 tonic chord structure brightness damping position u0 u1)
  (def act (+ (eq m 1) (eq m 2) (eq m 4) (eq m 5)))
  (def sym (+ (eq m 1) (eq m 4)))
  (def disp (+ (eq m 2) (eq m 5)))
  ; ---- Plucker: noise burst through a comb filter and a lowpass
  (def cutoff (* brightness (- 2 brightness)))
  (def fc (min (* f0 (rg-semi-ratio (* (- cutoff 0.5) 96))) 0.499))
  (def trig (* bstart strum act))
  (def ratio (+ (* position 0.9) 0.05))
  (def period_full (/ ratio f0))
  (def pa (gswitch (gte period_full 255) (* period_full 0.5) period_full))
  (def pb (gswitch (gte pa 255) (* pa 0.5) pa))
  (def pc (gswitch (gte pb 255) (* pb 0.5) pb))
  (def pd (gswitch (gte pc 255) (* pc 0.5) pc))
  (def pe (gswitch (gte pd 255) (* pd 0.5) pd))
  (def pf (gswitch (gte pe 255) (* pe 0.5) pe))
  (def pg (gswitch (gte pf 255) (* pf 0.5) pf))
  (def comb_period (latch pg trig))
  (def comb_gain (latch (* (- 1 position) 0.8) trig))
  (def pl_g (latch (rg-tan-dirty (min (* fc 8) 0.499)) trig))
  (make-history h_rem)
  (def rem0 (gswitch trig (floor period_full) (read-history h_rem)))
  (def burst (gt rem0 0))
  (write-history h_rem (- rem0 burst))
  (def pl_in (* burst (- (* 2 u0) 1)))
  (make-history h_cc)
  (def cc (read-history h_cc))
  (def comb_out (+ pl_in (* comb_gain (rg-tap rg_comb cc comb_period))))
  (def comb_w (seq (poke rg_comb cc comb_out) comb_out))
  (write-history h_cc (% (+ cc 1) 256))
  (def (pl_lp pl_bp pl_hp) (rg-svf comb_w pl_g 1))
  (def exc (rg-dc-block (* act pl_lp) (- 1 (/ 10 samplerate))))
  ; ---- string notes (Part::ComputeSympatheticStringsNotes / chords)
  (def sp1 structure)
  (def sp2 (+ sp1 (* (- 1 sp1) 0.2)))
  (def sp3 (+ sp2 (* (- 1 sp2) 0.2)))
  (def sp4 (+ sp3 (* (- 1 sp3) 0.2)))
  (def q (eq m 4))
  (def cbase (* chord 8))""")
for k in range(8):
    if k == 0:
        sym_note = "tonic"
    elif k <= 4:
        sym_note = f"(rg-sym-note tonic sp{k})"
    else:
        sym_note = {5: "(+ tonic 0.013)", 6: "(+ n1s 0.011)", 7: "(+ n2s 0.007)"}[k]
    w.append(f"  (def n{k}s {sym_note})")
    w.append(f"  (def n{k} (gswitch q (+ tonic (peek rg_part_chords (+ cbase {k}) 0)) n{k}s))")
w.append("""  ; ---- string 0: plucked, dispersion in models 2 and 5
  (def d0 (gswitch (eq m 5) (* damping (- 2 damping)) damping))
  (def dispersion (gswitch (lt structure 0.24) (* (- structure 0.24) 4.166)
                           (gswitch (gt structure 0.26) (* (- structure 0.26) 1.35135) 0)))
  (def f_0 (gswitch sym (rg-note-to-freq n0) f0))
  (def (sf0 sr0 dly0 cp0 dc0 br0 g0 h0 dcm0 nf0) (rg-string-ctl f_0 brightness d0 position))
  (def (o0 a0) (rg-string-disp rg_line0 rg_stretch exc u1 bstart act disp sf0 sr0 dly0 cp0 dc0 br0 g0 h0 dcm0 nf0 dispersion))
  ; ---- strings 1..7: sympathetic resonance from string 0
  (def sym_in (* (/ 0.2 8) (- o0 a0)))
  (def bs1 (* brightness (- 2 brightness)))
  (def bs (* bs1 (- 2 bs1)))
  (def glide (rg-semi-ratio (* (- bs 1) 36)))
  (def ds (+ 0.7 (* damping 0.27)))
  (def lfo_amount (* (- 0.5 (abs (- 0.5 position))) 0.9))""")
for k in range(1, 8):
    coef = f"(rg-cos-coef (* {LFO[k]} (/ 24 samplerate)))"
    w.append(f"  (def (fq{k} lfo{k}) (rg-sym-state (rg-note-to-freq n{k}) glide {coef} bstart sym))")
    w.append(f"  (def (sf{k} sr{k} dly{k} cp{k} dc{k} br{k} g{k} h{k} dcm{k} nf{k})")
    w.append(f"    (rg-string-ctl fq{k} bs (+ ds (* {k / 8} (- 0.95 ds))) (+ position (* lfo{k} lfo_amount))))")
    w.append(f"  (def (o{k} a{k}) (rg-string-plain rg_line{k} (* sym sym_in) bstart sym sf{k} sr{k} dly{k} cp{k} dc{k} br{k} g{k} h{k} dcm{k}))")
w.append("  (tuple (+ " + " ".join(f"o{k}" for k in range(8)) + ")\n         (+ "
         + " ".join(f"a{k}" for k in range(8)) + ")))")

src = ("; GENERATED by tools/gen_strings.py from tools/*.lisp.in -- edit those.\n"
       + head + plain + "\n" + withd + "\n" + "\n".join(w) + "\n")
open(os.path.join(ROOT, "src", "strings.lisp"), "w").write(src)
print("wrote src/strings.lisp", len(src))
