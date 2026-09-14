#!/usr/bin/env python3
"""Export the shared candidate and complete presets; optionally install locally."""
import argparse
import json
from pathlib import Path
from fit import ROOT,NAMES,LOW,HIGH

GROUP=['3 Kick -roundhouse.wav','Boom-Bap Kick 52.wav','Boom-Bap Kick 60.wav','Boom-Bap Kick 65.wav','Boom-Bap Kick 72.wav']


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--install',action='store_true')
    args=ap.parse_args()
    report=json.loads((ROOT/'tools/pm-doom-kick/fit-results.json').read_text())
    fits=[next(x for x in report['fits'] if x['title']==name) for name in GROUP]
    default=next(x for x in fits if x['title']=='Boom-Bap Kick 52.wav')['params']
    source=''';; Reduced three-mode resonant kick study, fit to a local sample family.
;; Shared empirical tension relaxation, independent modal initial conditions,
;; radiation rise and damping, followed by a saturating recording stage/gate.
;; This is NOT an identified two-head drum geometry or sample playback.
;; See tools/pm-doom-kick/README.md for fit errors and model limitations.
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
'''
    for i,n in enumerate(NAMES):
        unit=' @unit ms' if n.endswith('_ms') or n.endswith('_decay') else (' @unit Hz' if n=='frequency' else '')
        source+=f'(param {n} @default {default[n]:.10g} @min {LOW[i]:.10g} @max {HIGH[i]:.10g}{unit})\n'
    source+='''
(defmacro kick-hit (onset pitch velocity)
(make-history elapsed)
(make-history armed)
(def played (max onset (read-history armed)))
(write-history armed played)
;; Bounded integer-valued sample age avoids accumulating floating time error.
(def age_samples (gswitch onset 0 (min (* 2 samplerate) (+ 1 (read-history elapsed)))))
(write-history elapsed age_samples)
(def age (/ age_samples samplerate))
(def hit_velocity (latch (clip velocity 0 1) onset))
(def tuning (* 2 (clip (latch (/ pitch 440) onset) 0.25 4)))
'''
    for i,n in enumerate(NAMES):
        source+=f'(def {n}_v (clip (latch {n} onset) {LOW[i]:.10g} {HIGH[i]:.10g}))\n'
    source+='''
(def fast_s (* 0.001 fast_ms_v))
(def slow_s (* 0.001 slow_ms_v))
(def fast_e (exp (/ (- age) fast_s)))
(def slow_e (exp (/ (- age) slow_s)))
(def relaxed_time (+ (* bend_fast_v fast_s (- 1 fast_e)) (* bend_slow_v slow_s (- 1 slow_e))))
(def instantaneous_bend (+ (* bend_fast_v fast_e) (* bend_slow_v slow_e)))
(def f (* frequency_v tuning))
(defmacro kick-mode (time relaxed bend_now frequency bend gain decay phase attack)
  (def cycles (* frequency (+ time (* bend relaxed))))
  (def rise (- 1 (exp (/ (- time) (* 0.001 attack)))))
  (def envelope (* rise (exp (/ (- time) (* 0.001 decay)))))
  ;; Avoid oscillator foldover outside the fitted register during transposition.
  (def hz (* frequency (+ 1 (* bend bend_now))))
  (def audible (clip (/ (- (* 0.48 samplerate) hz) (* 0.08 samplerate)) 0 1))
  (* gain envelope audible (sin (+ (* twopi (wrap cycles 0 1)) phase))))
(def head_a (kick-mode age relaxed_time instantaneous_bend f 1
  mode1_gain_v mode1_decay_v mode1_phase_v attack_ms_v))
(def head_b (kick-mode age relaxed_time instantaneous_bend (* f mode2_ratio_v) mode2_bend_v
  mode2_gain_v mode2_decay_v mode2_phase_v mode2_attack_ms_v))
(def contact (kick-mode age relaxed_time instantaneous_bend (* f mode3_ratio_v) mode3_bend_v
  mode3_gain_v mode3_decay_v mode3_phase_v mode3_attack_ms_v))
(def raw (* (+ head_a head_b contact) hit_velocity))
;; Smooth saturation knee, from rounded to nearly hard clipping. This models
;; the processed recording, not membrane motion. No output normalization.
(def compressed (/ raw (pow (+ 1 (pow (abs raw) shape_v)) (/ 1 shape_v))))
(def u (clip (/ (- age (* 0.001 hold_ms_v)) (* 0.001 fade_ms_v)) 0 1))
(def recording_gate (- 1 (* u u (- 3 (* 2 u)))))
(def output (* played level_v recording_gate (mix raw compressed saturation_v)))
output)
(def (trigger_a trigger_b gain_a gain_b) (kick-retrigger trigger gate))
(def hit_a (kick-hit trigger_a pitch velocity))
(def hit_b (kick-hit trigger_b pitch velocity))
(def mixed (+ (* hit_a gain_a) (* hit_b gain_b)))
(out mixed 1 @name left)
(out mixed 2 @name right)
'''
    folder=ROOT/'tools/pm-doom-kick'
    source=(ROOT/'tools/instrument-support/kick-retrigger.lisp').read_text()+'\n'+source
    (folder/'model.lisp').write_text(source)
    bank={'version':1,'engine_name':'Studies/DOOM Kick','source_file':'instruments/Studies/DOOM Kick/dsp.lisp',
          'presets':[{'id':x['hash'][:12],'name':Path(x['title']).stem,'base_note_offset':0,'params':x['params']} for x in fits]}
    (folder/'model.presets').write_text(json.dumps(bank,indent=2)+'\n')
    (folder/'selection.json').write_text(json.dumps({'method':'Five best fits among ten members of the five closest measured neighborhoods; all candidate errors retained in fit-results.json.','selected':fits},indent=2)+'\n')
    if args.install:
        dest=ROOT/'.local/instruments/Studies/DOOM Kick'
        dest.mkdir(parents=True,exist_ok=True)
        (dest/'dsp.lisp').write_text(source)
        (dest/'ui.lisp').write_text((folder/'ui.lisp').read_text())
        (dest/'instrument.json').write_text('{"version":1,"run_mode":"instrument"}\n')
        dest.with_suffix('.presets').write_text(json.dumps(bank,indent=2)+'\n')
        print(dest)


if __name__=='__main__':main()
