#!/usr/bin/env python3
"""Export and install a controllable reduced resonant Kick 53 study."""
import argparse,json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2];HERE=Path(__file__).parent
PARAMS=[('tune',1,.5,2),('decay',1,.25,2.5),('bend',1,0,2),('attack',1,.25,3),
('body',1,0,2),('knock',1,0,2),('air',1,0,3),('air_ms',22,3,100),
('air_hz',3500,1000,7000),('drive',1,1,5),('clip_mix',1,0,1),('ceiling',.8889390649866298,.2,1),('level',1,0,1.5),('length_ms',700,150,2000)]
def main():
    ap=argparse.ArgumentParser();ap.add_argument('--install',action='store_true');args=ap.parse_args()
    fit=json.loads((HERE/'fit-results.json').read_text())
    s=''';; Kick 53 reduced resonant study. Measured modal coefficients plus an
;; empirical pitch relaxation and recording clip; not identified drum geometry.
;; No source waveform or residual is stored. A3 reproduces the fitted register; the default register is one octave up.
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
'''
    for n,d,lo,hi in PARAMS:s+=f'(param {n} @default {d} @min {lo} @max {hi})\n'
    s+='''(defmacro kick-hit (onset pitch velocity)
(make-history elapsed)
(make-history armed)
(def played (max onset (read-history armed)))
(write-history armed played)
(def age_samples (gswitch onset 0 (min (* 8 samplerate) (+ 1 (read-history elapsed)))))
(write-history elapsed age_samples)
(def age (/ age_samples samplerate))
(def vel (latch (clip velocity 0 1) onset))
(def note (* 2 (clip (latch (/ pitch 440) onset) 0.25 4)))
'''
    for n,d,lo,hi in PARAMS:s+=f'(def {n}_v (clip (latch {n} onset) {lo} {hi}))\n'
    s+='''(def tuning (* note tune_v))
(defmacro break-mode (f decay_s delta tau rise cosine sine amount)
  (def shift (max (* -0.9 f) (* bend_v delta)))
  (def phase (* 2 pi tuning (+ (* f age) (* shift tau (- 1 (exp (/ (- age) tau)))))))
  (def hz (* tuning (+ f (* shift (exp (/ (- age) tau))))))
  (def audible (clip (/ (- (* 0.48 samplerate) hz) (* 0.08 samplerate)) 0 1))
  (def envelope (* (exp (/ (- age) (* decay_v decay_s)))
    (- 1 (exp (/ (- age) (* attack_v rise))))))
  (* amount audible envelope (+ (* cosine (cos phase)) (* sine (sin phase)))))
'''
    for i,m in enumerate(fit['modes']):
        f,dec,b,tau,a=m['shape'];c,si=m['weights'];amount='body_v' if f<80 else 'knock_v'
        s+=f'(def mode_{i} (break-mode '+' '.join(f'{v:.12g}' for v in [f,dec,b,tau,a,c,si])+f' {amount}))\n'
    s+='(def resonances (+ '+' '.join(f'mode_{i}' for i in range(8))+'))\n'
    s+=''';; A fresh filtered noise burst supplies stochastic attack texture. Its
;; envelope is independent from the resonant tail; no recording noise is copied.
(def white (* (- (* (noise) 2) 1) (sqrt (/ samplerate 16000))))
(def air_band (biquad (biquad white 700 0.707 1 1) (min air_hz_v (* 0.4 samplerate)) 0.707 1 0))
(def air_env (* (- 1 (exp (/ (- age) 0.0006))) (exp (/ (- age) (* 0.001 air_ms_v)))))
(def texture (* 0.018 air_v air_env air_band))
;; Recording clipping and creative drive are separate. Air is mixed after
;; both, so the low body cannot flatten the noise transient against a ceiling.
(def recorded (mix resonances (clip resonances (- ceiling_v) ceiling_v) clip_mix_v))
;; Complementary split: the unchanged body plus a driven upper branch.
;; At drive=1 the residual is exactly zero. Filter the added distortion to
;; prevent new low-frequency products from thickening the body.
(def upper (biquad recorded (min (* 180 tuning) (* 0.2 samplerate)) 0.707 1 1))
(def amount (/ (- drive_v 1) 4))
(def saturated (/ (tanh (* drive_v upper)) (sqrt drive_v)))
(def residual (* amount (- saturated upper)))
(def colour (biquad residual (min (* 180 tuning) (* 0.2 samplerate)) 0.707 1 1))
(def limited (+ recorded colour texture))
(def end_s (* length_ms_v 0.001))
(def u (clip (/ (- age (- end_s 0.02)) 0.02) 0 1))
(def fade (- 1 (* u u (- 3 (* 2 u)))))
(def output (* played vel level_v fade limited))
output)
(def (trigger_a trigger_b gain_a gain_b) (kick-retrigger trigger gate))
(def hit_a (kick-hit trigger_a pitch velocity))
(def hit_b (kick-hit trigger_b pitch velocity))
(def mixed (+ (* hit_a gain_a) (* hit_b gain_b)))
(out mixed 1 @name left)
(out mixed 2 @name right)
'''
    s=(ROOT/'tools/instrument-support/kick-retrigger.lisp').read_text()+'\n'+s
    (HERE/'model.lisp').write_text(s)
    bank={'version':1,'engine_name':'Studies/Break Kick 53','source_file':'instruments/Studies/Break Kick 53/dsp.lisp','presets':[{'id':'kick-53','name':'Boom-Bap Kick 53','base_note_offset':0,'params':{n:d for n,d,_,_ in PARAMS}}]}
    bright={n:d for n,d,_,_ in PARAMS}
    bright.update({'air':2.5,'clip_mix':0.25,'drive':1,'level':0.4})
    bank['presets'].append({'id':'kick-53-air-plus12','name':'Air','base_note_offset':0,'params':bright})
    (HERE/'model.presets').write_text(json.dumps(bank,indent=2)+'\n')
    if args.install:
        dest=ROOT/'.local/instruments/Studies/Break Kick 53';dest.mkdir(parents=True,exist_ok=True)
        (dest/'dsp.lisp').write_text(s);(dest/'ui.lisp').write_text((HERE/'ui.lisp').read_text())
        (dest/'instrument.json').write_text('{"version":1,"run_mode":"instrument"}\n')
        # Retain user-saved or edited presets. Add only missing study presets.
        bank_path=dest.with_suffix('.presets')
        if bank_path.exists():
            saved=json.loads(bank_path.read_text())
            ids={p['id'] for p in saved['presets']}
            saved['presets'].extend(p for p in bank['presets'] if p['id'] not in ids)
            bank=saved
        bank_path.write_text(json.dumps(bank,indent=2)+'\n')
if __name__=='__main__':main()
