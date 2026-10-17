; ======================================================================
; FM voice model (rings/dsp/fm_voice.cc): a two-operator FM voice with
; modulator feedback. STRUCTURE quantizes the frequency ratio, BRIGHTNESS
; the modulation index, DAMPING the decay of the internal envelopes (and
; past 0.9 a crossfade to a sustained tone), POSITION the feedback (below
; noon: phase feedback, above: modulator self-FM). With the internal
; exciter the envelope follower sees silence, so the strum alone drives
; the amplitude and brightness envelopes.
; ======================================================================
(def fm_lut_sine (tensor @shape [5121 1] @file "lut_sine.json"))
(def fm_lut_ratio (tensor @shape [129 1] @file "lut_fm_frequency_quantizer.json"))

; FMVoice::SineFm: sine of (phase + fm) turns, lut_sine (4096 per cycle)
; interpolated linearly.
(defmacro fm-sine (phase fm)
  (def p (* (rg-wrap (+ phase fm)) 4096))
  (def i (floor p))
  (def a (peek fm_lut_sine i 0))
  (def b (peek fm_lut_sine (+ i 1) 0))
  (+ a (* (- b a) (- p i))))

; Phase accumulator in turns, wrapped to [0, 1).
(defmacro fm-phase (increment)
  (make-history h)
  (def p (rg-wrap (+ (read-history h) increment)))
  (write-history h p)
  p)

(defmacro rg-fm (bstart bphase strum f0 structure brightness damping position)
  ; ---- block-rate settings
  (def env_amount (gswitch (lt damping 0.9) 1 (* (- 1 damping) 10)))
  (def amp_rt60 (* 0.1 (rg-semi-ratio (* damping 96)) samplerate))
  (def amp_decay (- 1 (exp (/ -6.907755279 amp_rt60))))
  (def bri_rt60 (* 0.1 (rg-semi-ratio (* damping 84)) samplerate))
  (def bri_decay (- 1 (exp (/ -6.907755279 bri_rt60))))
  (def ratio (rg-lut fm_lut_ratio structure 128))
  (def mod_f (min (* f0 (rg-semi-ratio ratio)) 0.5))
  (def feedback (* (- position 0.5) 2))
  (def phase_feedback (gswitch (lt feedback 0) (* 0.5 feedback feedback) 0))
  ; ParameterInterpolators, from FMVoice::Init's values
  (def car_inc (rg-interp f0 bstart bphase 0.0045833333))
  (def mod_inc (rg-interp mod_f bstart bphase 0.0045833333))
  (def bri (rg-interp brightness bstart bphase 0.5))
  (def fb (rg-interp feedback bstart bphase 0))
  ; ---- internal envelopes: reset to 1 on the strum, decaying to 0
  (def trig (* bstart strum))
  (make-history h_amp)
  (make-history h_ben)
  (def amp0 (gswitch trig 1 (read-history h_amp)))
  (def amp_env (- amp0 (* amp_decay amp0)))
  (write-history h_amp amp_env)
  (def ben0 (gswitch trig 1 (read-history h_ben)))
  (def bri_env (- ben0 (* bri_decay ben0)))
  (write-history h_ben bri_env)
  ; ---- modulation index
  (def bv (* bri bri))
  (def fm_min (gswitch (lt bv 0.5) 0 (- (* bv 2) 1)))
  (def fm_max (gswitch (lt bv 0.5) (* 2 bv) 1))
  (def fm_env (+ 0.5 (* env_amount (- bri_env 0.5))))
  (def fm_target (* (+ fm_min (* fm_max fm_env)) 2))
  (make-history h_fma)
  (def fma_prev (read-history h_fma))
  (def slew (+ 0.005 (* fm_max 0.015)))
  (def fm_amount (+ fma_prev (clip (- fm_target fma_prev) (- slew) slew)))
  (write-history h_fma fm_amount)
  ; ---- operators
  (make-history h_prev)
  (def prev (read-history h_prev))
  (def mod_phase (fm-phase (* mod_inc (+ 1 (* prev phase_feedback)))))
  (def car_phase (fm-phase car_inc))
  (def modulator_fb (gswitch (gt fb 0) (* 0.25 fb fb) 0))
  (def modulator (fm-sine mod_phase (* modulator_fb prev)))
  (def carrier (fm-sine car_phase (* fm_amount modulator)))
  (write-history h_prev (+ prev (* 0.1 (- carrier prev))))
  ; ---- amplitude
  (def gain_target (+ 1 (* env_amount (- amp_env 1))))
  (def gain (rg-one-pole gain_target (+ 0.005 (* 0.045 fm_amount))))
  (tuple (* (+ carrier (* 0.5 modulator)) gain) (* 0.5 modulator gain)))
