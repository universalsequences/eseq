; ======================================================================
; Snare drum engine (plaits/dsp/engine/snare_drum_engine.cc):
;   OUT = AnalogSnareDrum (808-style: 5 bridged-T modes + filtered noise)
;   AUX = SyntheticSnareDrum (909-style: 2 distorted sines + noise)
; Unpatched trigger (patched = 0) = sustain mode.
; ======================================================================

; Exact stmlib ParameterInterpolator (see bd-pi): `called` = 1 on samples
; where the C++ calls Next().
(defmacro sd-pi (target bstart called)
  (make-history h_v)
  (make-history h_inc)
  (def v0 (read-history h_v))
  (def inc (gswitch bstart (/ (- target v0) 12) (read-history h_inc)))
  (write-history h_inc inc)
  (def v (+ v0 (* called inc)))
  (write-history h_v v)
  v)

; stmlib Svf band-pass whose state only advances while `en` is 1.
(defmacro sd-svf-bp (in g r en)
  (make-history s1)
  (make-history s2)
  (def h (/ 1 (+ 1 (* r g) (* g g))))
  (def st1 (read-history s1))
  (def st2 (read-history s2))
  (def hp (* (- in (* r st1) (* g st1) st2) h))
  (def bp (+ (* g hp) st1))
  (def lp (+ (* g bp) st2))
  (write-history s1 (gswitch en (+ (* g hp) bp) st1))
  (write-history s2 (gswitch en (+ (* g bp) lp) st2))
  bp)

; stmlib OnePole (g from tan approximation); (tuple lp hp).
(defmacro sd-onepole (in g)
  (make-history h)
  (def st (read-history h))
  (def lp (/ (+ (* g in) st) (+ 1 g)))
  (write-history h (+ (* g (- in lp)) lp))
  (tuple lp (- in lp)))

(defmacro sd-softclip (x)
  (def c (clip x -3 3))
  (/ (* c (+ 27 (* c c))) (+ 27 (* 9 c c))))

; One mode of the analog snare: resonator (triggered) or sine (sustain).
(defmacro sd-mode (ratio qmul f0 q excitation leak sustain sgain)
  (make-history h_ph)
  (def f (min (* f0 ratio) 0.499))
  (def bp (sd-svf-bp excitation (pl-tan-fast f) (/ 1 (+ 1 (* f q qmul))) (- 1 sustain)))
  (def ph_prev (read-history h_ph))
  (def ph_inc (+ ph_prev (min f 0.5)))
  (def ph (gswitch sustain (- ph_inc (gte ph_inc 1)) ph_prev))
  (write-history h_ph ph)
  (gswitch sustain (* (sin (* twopi ph)) sgain 0.25) (+ bp (* excitation leak))))

(defmacro sd-analog (bstart trig sustain accent f0 tone decay snappy0 u_n)
  (make-history h_pr)      ; pulse_remaining_samples_
  (make-history h_pulse)   ; pulse_
  (make-history h_height)  ; pulse_height_
  (make-history h_plp)     ; pulse_lp_
  (make-history h_nenv)    ; noise_envelope_
  (def decay_xt (* decay (+ 1 (* decay (- decay 1)))))
  (def q (* 2000 (pl-semi-ratio (* decay_xt 84))))
  (def nenv_decay (- 1 (* 0.0017 (pl-semi-ratio (* (- decay) (+ 50 (* snappy0 10)))))))
  (def leak (* snappy0 (- 2 snappy0) 0.1))
  (def snappy (clip (- (* snappy0 1.1) 0.05) 0 1))
  (def height (gswitch trig (+ 3 (* 7 accent)) (read-history h_height)))
  (write-history h_height height)
  ; mode gains
  (def lo (lt tone 0.666667))
  (def t_lo (* tone 1.5))
  (def t_hi (* (- tone 0.666667) 3))
  (def g0 (gswitch lo (+ 1.5 (* (- 1 t_lo) (- 1 t_lo) 4.5)) (- 1.5 (* t_hi 0.5))))
  (def g1 (gswitch lo (+ (* 2 t_lo) 0.15) (- 2.15 (* t_hi 0.7))))
  (def g2 (* (- 1 lo) t_hi))
  (def g3 (* g2 t_hi))
  (def g4 (* g3 g3))
  (def sgain (sd-pi (* accent decay) bstart 1))
  ; Q45 / Q46 trigger pulse
  (def pr0 (gswitch trig 48 (read-history h_pr)))
  (def pon (gt pr0 0))
  (def pr1 (- pr0 pon))
  (write-history h_pr pr1)
  (def pulse (gswitch pon (gswitch (gt pr1 0) height (- height 1))
                          (* (read-history h_pulse) (- 1 (/ 1 4.8)))))
  (write-history h_pulse pulse)
  (def plp (+ (read-history h_plp) (* 0.75 (- pulse (read-history h_plp)))))
  (write-history h_plp plp)
  (def exc0 (+ (- pulse plp) (* 0.006 pulse)))
  (def exc (* 0.026 pulse))
  (def shell (sd-softclip
    (+ (* g0 (sd-mode 1.00 1 f0 q exc0 leak sustain sgain))
       (* g1 (sd-mode 2.00 0.25 f0 q exc leak sustain sgain))
       (* g2 (sd-mode 3.18 0.25 f0 q exc leak sustain sgain))
       (* g3 (sd-mode 4.16 0.25 f0 q exc leak sustain sgain))
       (* g4 (sd-mode 5.62 0.25 f0 q exc leak sustain sgain)))))
  ; noise
  (def nenv (* (gswitch trig 2 (read-history h_nenv)) nenv_decay))
  (write-history h_nenv nenv)
  (def noise0 (* (max (- (* 2 u_n) 1) 0) (gswitch sustain sgain nenv) snappy 2))
  (def f_noise (clip (* f0 16) 0 0.499))
  (def noise (sd-svf-bp noise0 (pl-tan-fast f_noise) (/ 1 (+ 1 (* f_noise 1.5))) 1))
  (+ noise (* shell (- 1 snappy))))

(defmacro sd-distorted-sine (phase)
  (def tri (- (* 4 (gswitch (lt phase 0.5) phase (- 1 phase))) 1.3))
  (/ (* 2 tri) (+ 1 (abs tri))))

; Phase advance with the 909 reset coupling.
(defmacro sd-phase-step (prev f rn rna)
  (def p (+ prev f))
  (gswitch (gt rna 0.1)
           (gswitch (gte p (+ 1 rn)) (- 1 p) p)
           (- p (gte p 1))))

(defmacro sd-synthetic (bstart bphase trig sustain accent f0 timbre decay snappy0 u_n)
  (make-history h_ph0)
  (make-history h_ph1)
  (make-history h_drum)    ; drum_amplitude_
  (make-history h_snare)   ; snare_amplitude_
  (make-history h_fm)      ; fm_
  (make-history h_hold)    ; hold_counter_
  (def ns (- 1 sustain))
  (def decay_xt (* decay (+ 1 (* decay (- decay 1)))))
  (def fma (* timbre timbre))
  (def drum_decay (- 1 (* (/ 1 720) (pl-semi-ratio (+ (* decay_xt -72) (* fma -12) (* snappy0 7))))))
  (def snare_decay (- 1 (* (/ 1 480) (pl-semi-ratio (- (* decay -60) (* snappy0 7))))))
  (def fm_decay (- 1 (/ 1 336)))
  (def snappy (clip (- (* snappy0 1.1) 0.05) 0 1))
  (def drum_level (sqrt (- 1 snappy)))
  (def snare_level (sqrt snappy))
  (def f_min (min (* 10 f0) 0.5))
  (def f_max (min (* 35 f0) 0.5))
  (def level0 (+ 0.3 (* 0.7 accent)))
  (def sgain (sd-pi (* accent decay) bstart sustain))
  ; envelopes
  (def drum_prev (gswitch trig level0 (read-history h_drum)))
  (def snare_prev (gswitch trig level0 (read-history h_snare)))
  (def fm_prev (gswitch trig 1 (read-history h_fm)))
  (def hold0 (gswitch trig (floor (* (+ 0.04 (* decay 0.03)) 48000)) (read-history h_hold)))
  (def hold_on (* ns (gt hold0 0)))
  (write-history h_hold (- hold0 hold_on))
  ; `size` after the post-decrement is 11 - bphase: decay on odd bphase
  (def drum_mult (gswitch (+ (gt drum_prev 0.03) (% bphase 2)) drum_decay 1))
  (def drum_amp (gswitch sustain sgain (* drum_prev drum_mult)))
  (def snare_amp (gswitch sustain sgain (gswitch hold_on snare_prev (* snare_prev snare_decay))))
  (def fm (gswitch sustain 0 (* fm_prev fm_decay)))
  (write-history h_drum drum_amp)
  (write-history h_snare snare_amp)
  (write-history h_fm fm)
  ; oscillators
  (def rna0 (clip (* (- 0.125 f0) 8) 0 1))
  (def rna (* rna0 rna0 fma))
  (def ph0_prev (gswitch trig 0 (read-history h_ph0)))
  (def ph1_prev (gswitch trig 0 (read-history h_ph1)))
  (def rn (* (+ (gswitch (gt ph0_prev 0.5) -1 1) (gswitch (gt ph1_prev 0.5) -1 1)) rna 0.025))
  (def f (* f0 (+ 1 (* fma 4 fm))))
  (def ph0 (sd-phase-step ph0_prev f rn rna))
  (def ph1 (sd-phase-step ph1_prev (* f 1.47) rn rna))
  (write-history h_ph0 ph0)
  (write-history h_ph1 ph1)
  (def drum0 (* (+ -0.1 (* (sd-distorted-sine ph0) 0.6) (* (sd-distorted-sine ph1) 0.25))
                drum_amp drum_level))
  (def (drum d_hp) (sd-onepole drum0 (pl-tan-fast (* 3 f0))))
  ; noise
  (def (s_lp s_bp s_hp) (pl-svf u_n (pl-tan-fast f_max) (/ 1 (+ 0.5 (* 2 snappy)))))
  (def (s_lp2 s_hp2) (sd-onepole s_lp (pl-tan-fast f_min)))
  (def snare (* (+ s_hp2 0.1) (+ snare_amp fm) snare_level))
  (+ snare drum))

(defmacro plaits-sd (bstart bphase rise high patched note timbre morph harmonics accent)
  (def nz (noise))   ; per-sample uniform draw (identical (noise) nodes merge)
  (def f0 (pl-note-to-freq note))
  (def sustain (- 1 patched))
  (def trig (* bstart rise))
  (def out (sd-analog bstart trig sustain accent f0 timbre morph harmonics nz))
  (def aux (sd-synthetic bstart bphase trig sustain accent f0 timbre morph harmonics (pl-noise-tap nz)))
  (tuple out aux))
