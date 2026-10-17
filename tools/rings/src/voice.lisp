; ======================================================================
; voice (rings/rings.cc FillBuffer, cv_scaler.cc, strummer.h, part.cc
; Process at polyphony 1): block clock, strum, controls, model selection,
; effects, limiter and outputs
; ======================================================================

(defmacro db-amp (db)
  (exp (* 0.1151292546 db)))

; ---- block clock (kMaxBlockSize = 24) ----
(make-history h_bp)
(def bphase (read-history h_bp))
(write-history h_bp (% (+ bphase 1) 24))
(def bstart (eq bphase 0))

(def m (clip (round model) 0 11))

; ---- strum: the note's gate edge, plus retrigger pulses, once per block,
; then Strummer's 10 ms inhibit ----
(def gate_hi (gt gate 0.5))
(make-history h_pending)
(def pending (max (read-history h_pending) (gt trigger 0.5)))
(write-history h_pending (gswitch bstart 0 pending))
(make-history h_prev_gate)
(def g_prev (read-history h_prev_gate))
(write-history h_prev_gate (gswitch bstart gate_hi g_prev))
(def strum_raw (max pending (* gate_hi (- 1 g_prev))))
(make-history h_inh)
(def inh (read-history h_inh))
(def strum_now (* strum_raw (eq inh 0)))
(def inh_new (gswitch (gt inh 0) (- inh 1) (* strum_now (floor (* 0.01 (/ samplerate 24))))))
(write-history h_inh (gswitch bstart inh_new inh))
(def strum (latch strum_now bstart))
(def strum_h (rg-hold strum_now bstart))
(def vel (latch (clip velocity 0 1) (* bstart strum_now)))

; ---- controls, read once per block ----
(def midi (+ 69 (* 17.312340491 (log (/ (max pitch 1) 440))) (mod tune)))
(def tonic (latch midi bstart))
(def f0 (rg-note-to-freq tonic))
(def structure_b (latch (clip (mod structure) 0 0.9995) bstart))
(def brightness_b (latch (clip (mod brightness) 0 1) bstart))
(def damping_b (latch (clip (mod damping) 0 1) bstart))
(def position_b (latch (clip (mod position) 0 1) bstart))
; the same values as plain sample-and-holds, for the block-gated models
(def tonic_h (rg-hold midi bstart))
(def f0_h (rg-note-to-freq tonic_h))
(def structure_h (rg-hold (clip (mod structure) 0 0.9995) bstart))
(def brightness_h (rg-hold (clip (mod brightness) 0 1) bstart))
(def damping_h (rg-hold (clip (mod damping) 0 1) bstart))
(def position_h (rg-hold (clip (mod position) 0 1) bstart))

; ---- chord: STRUCTURE x 10 with CvScaler's hysteresis ----
(make-history h_chord)
(def chord_prev (read-history h_chord))
(def chord_cv (* structure_b 10))
(def chord_new (clip (floor (+ chord_cv (gswitch (gt (- chord_cv chord_prev) 0) -0.1 0.1) 0.5)) 0 10))
(write-history h_chord (gswitch bstart chord_new chord_prev))
(def chord (latch chord_new bstart))
(def chord_h (rg-hold chord_new bstart))

; ---- models (only the selected one runs, except the string loops) ----
(def u0 (noise))
(def u1 (rg-wrap (* (rg-noise-tap (rg-noise-tap (rg-noise-tap u0))) 4093.7)))
(def (mo ma) (block-gate (eq m 0) (rg-modal bstart bphase strum_h f0_h structure_h brightness_h damping_h position_h)))
(def (fo fa) (block-gate (eq m 3) (rg-fm bstart bphase strum_h f0_h structure_h brightness_h damping_h position_h)))
(def (so sa) (rg-strings bstart strum m f0 tonic chord structure_b brightness_b damping_b position_b u0 u1))
(def (po pa) (block-gate (gte m 6) (rg-peace bstart bphase strum_h tonic_h chord_h brightness_h damping_h position_h (- m 6))))

; ---- effects ----
(def is5 (eq m 5))
(def part_l (+ mo fo so))
(def part_r (+ ma fa sa))
; model 5: odd/even crossfeed by POSITION before the reverb
(def x5_l (+ (* part_l position_b) (* (- 1 position_b) part_r)))
(def x5_r (+ (* part_r position_b) (* (- 1 position_b) part_l)))
(def dp_rev (+ (eq m 8) (eq m 11)))
(def rv_on (max is5 dp_rev))
(def rv_amount (gswitch is5 (+ 0.1 (* damping_b 0.5)) (* position_b 0.5)))
(def rv_time (gswitch is5 (+ 0.35 (* 0.63 damping_b))
                      (gswitch (eq m 8) (+ 0.5 (* 0.49 position_b)) (+ 0.3 (* 0.6 position_b)))))
(def rv_lp (gswitch is5 (+ 0.3 (* 0.6 brightness_b)) (gswitch (eq m 8) 0.3 0.6)))
(def (rv_l rv_r) (rg-reverb (* rv_on (gswitch is5 x5_l po)) (* rv_on (gswitch is5 x5_r pa))
                            rv_amount rv_time 0.2 rv_lp))
(def is_ch (eq m 7))
(def is_en (eq m 10))
(def mod_on (max is_ch is_en))
(def (mf_l mf_r) (rg-modfx (* mod_on po) (* mod_on pa) is_ch
                           (gswitch is_ch position_b (* position_b (- 2 position_b)))
                           (gswitch is_ch (+ 0.15 (* 0.5 position_b)) (+ 0.2 (* 0.8 position_b position_b)))))

; ---- outputs before the limiter (the module's OUT / AUX) ----
(def sel (+ m 1))
(def pre_l (selector sel part_l part_l part_l part_l part_l rv_l po mf_l rv_l po mf_l rv_l))
(def pre_r (selector sel part_r part_r part_r part_r part_r (- rv_r) (- pa) (- mf_r) (- rv_r) (- pa) (- mf_r) (- rv_r)))

; ---- Limiter (part.cc model_gains_; the string synth uses 1) ----
(def lim_gain (selector sel 1.4 1 1.4 0.7 1 1.4 1 1 1 1 1 1))
(def l_pre (* pre_l lim_gain))
(def r_pre (* pre_r lim_gain))
(def peak (rg-slope (max (abs l_pre) (abs r_pre) (abs (- r_pre l_pre))) 0.05 0.00002 0.5))
(def lim (gswitch (lte peak 1) 0.8 (/ 0.8 peak)))
(def odd (clip (rg-soft-limit (* l_pre lim)) -1 1))
(def even (clip (rg-soft-limit (* r_pre lim)) -1 1))

; ---- spread, level ----
(def sp (clip (mod spread) 0 1))
(def ka (* 0.5 (+ 1 sp)))
(def kb (* 0.5 (- 1 sp)))
(def out_level (* (db-amp (clip (mod level_db) -60 12)) vel))
(def voice_left (* (+ (* odd ka) (* even kb)) out_level))
(def voice_right (* (+ (* even ka) (* odd kb)) out_level))
(out voice_left 1 @name left)
(out voice_right 2 @name right)
(use-defmacro voice-amp)
(use-defmacro release-window)
(out (voice-amp (release-window gate 30) voice_left voice_right) 3 @name amp @amp true)
