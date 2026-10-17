; ======================================================================
; voice (plaits/dsp/voice.cc): block clock, trigger, note, decay envelope,
; modulation routing, engine selection, LPG and output post-processing
; ======================================================================

(defmacro db-amp (db)
  (exp (* 0.1151292546 db)))

; Voice::ApplyModulations' attenuverter curve.
(defmacro pl-mod-amount (a)
  (* a (max (- (abs a) 0.05) 0.05) 1.05))

; stmlib Limiter (peak follower, 0.8 headroom) with its pre-gain.
(defmacro pl-limiter (x pre_gain)
  (def s (* x pre_gain))
  (def peak (pl-slope (abs s) 0.05 0.00002))
  (* s (/ 1 (max peak 1)) 0.8))

; ChannelPostProcessor: engine gain (negative = limiter), then either the
; low-pass gate or a plain copy; the module's DAC scale -32767/32768.
(defmacro pl-post (x gain bypass lpg_gain lpg_f lpg_hf bstart bphase)
  (def pre (gswitch (lt gain 0) (pl-limiter x (abs gain)) (* x gain)))
  (def s (* pre -0.99996948 (pl-interp lpg_gain bstart bphase 0)))
  (def (lp bp hp) (pl-svf s (pl-tan-dirty lpg_f) 2.5))
  (def gated (+ lp (* (- s lp) lpg_hf)))
  (clip (gswitch bypass (* pre -0.99996948) gated) -1 1))

; ---- block clock (kBlockSize = 12) ----
(make-history h_bp)
(def bphase (read-history h_bp))
(write-history h_bp (% (+ bphase 1) 12))
(def bstart (eq bphase 0))

; ---- model: UI order is the green bank, the red bank, then the 1.2
; firmware bank; Plaits' own engine order starts with the 1.2 bank. ----
(def pidx (pl-hold (% (+ (clip (round model) 0 23) 8) 24) bstart))

; ---- trigger: the note's gate, plus retrigger pulses, sampled per block ----
(def gate_hi (gt gate 0.5))
(make-history h_pending)
(def pending (max (read-history h_pending) (gt trigger 0.5)))
(write-history h_pending (gswitch bstart 0 pending))
(make-history h_prev_gate)
(def g_prev (pl-hold (read-history h_prev_gate) bstart))
(write-history h_prev_gate (gswitch bstart gate_hi (read-history h_prev_gate)))
(def rise (pl-hold (max pending (* gate_hi (- 1 g_prev))) bstart))
(def high (pl-hold gate_hi bstart))
(def gate_mode (gt lpg_mode 0.5))
(def ping_mode (- 1 gate_mode))

; ---- note: Voice averages the note CV over two blocks ----
(def midi (+ 69 (* 17.312340491 (log (/ (max pitch 1) 440))) (mod tune)))
(def note_cv (pl-hold midi bstart))
(make-history h_started)
(make-history h_note_prev)
(def note_prev (gswitch (read-history h_started) (read-history h_note_prev) note_cv))
(def note_avg (pl-hold (* 0.5 (+ note_cv note_prev)) bstart))
(write-history h_note_prev (gswitch bstart note_cv (read-history h_note_prev)))
(write-history h_started 1)

; ---- decay envelope (DecayEnvelope), per block ----
(def decay_v (pl-hold (clip (mod decay) 0 1) bstart))
(def short_decay (pl-hold (* 0.05 (pl-semi-ratio (* -96 decay_v))) bstart))
(make-history h_denv)
(def denv (gswitch bstart
  (* (gswitch rise 1 (read-history h_denv)) (- 1 (* 2 short_decay)))
  (read-history h_denv)))
(write-history h_denv denv)

; ---- level / accent ----
(def vel (clip velocity 0 1))
(def level_cv (* high vel))
(def comp_level (clip (/ (* 1.3 level_cv) (+ 0.3 level_cv)) 0 1))
(def accent (gswitch gate_mode comp_level 0.8))

; ---- synthesis parameters (Voice::ApplyModulations, internal envelope) ----
(def harm_b (pl-hold (clip (mod harmonics) 0 1) bstart))
(def iea (gswitch (eq pidx 15) (clip (- 2 (* harm_b 6)) 0 1) 1))
(def iea_timbre (gswitch (eq pidx 7) 0 1))
(def p_note (clip (+ note_avg (* (pl-mod-amount fm_amt) iea denv denv 48)) -119 120))
(def p_timbre (clip (+ (pl-hold (mod timbre) bstart) (* (pl-mod-amount timbre_amt) iea_timbre denv)) 0 1))
(def p_morph (clip (+ (pl-hold (mod morph) bstart) (* (pl-mod-amount morph_amt) iea denv)) 0 1))

; ---- engines (only the selected one runs) ----
; One uniform [0, 1) draw per sample, shared by the noise-driven engines.
(def u0 (noise))
;@ENGINES@

; ---- post-processing settings per engine (Voice::Init registry) ----
(def sel (+ pidx 1))
(def out_gain (selector sel 1 0.7 1 1 1 0.7 0.8 0.5 0.8 0.7 0.6 0.7 0.8 0.6 0.8 -0.7 -3 -1 -2 -1 -1 0.8 0.8 0.8))
(def aux_gain (selector sel 1 0.7 1 1 1 0.7 0.8 0.5 0.8 0.6 0.6 0.6 0.8 0.6 0.8 0.8 1 -1 1 0.8 0.8 0.8 0.8 0.8))
(def bypass (pl-hold (selector sel 0 0 1 1 1 0 0 1 0 0 0 0 0 0 0 speech_enveloped 0 0 0 1 1 1 1 1) bstart))

; ---- LPG envelope (LPGEnvelope), per block ----
(def hf (pl-hold (clip (mod lpg) 0 1) bstart))
(def decay_tail (- (* 0.005 (pl-semi-ratio (+ (* -72 decay_v) (* 12 hf)))) short_decay))
(def lpg_attack (* (pl-note-to-freq p_note) 24))
(make-history h_vac)
(make-history h_ramp)
(def ramp0 (max (read-history h_ramp) (* rise ping_mode)))
(def vac0 (read-history h_vac))
(def vac_r (+ vac0 lpg_attack))
(def vac_p (gswitch ramp0 (min vac_r 1) vac0))
(def ramp1 (* ramp0 (lt vac_r 1)))
(def lpg_level (gswitch gate_mode comp_level (* ramp1 vac_p)))
(def vac_s (gswitch gate_mode vac0 vac_p))
(def vac_err (- lpg_level vac_s))
(def vac_4 (* vac_s vac_s vac_s vac_s))
(def vac_tail2 (* (- 1 vac_s) (- 1 vac_s)))
(def vac_coef (gswitch (gt vac_err 0) 0.6 (+ short_decay (* (- 1 vac_4) decay_tail))))
(def vac_new (+ vac_s (* vac_coef vac_err)))
(write-history h_vac (gswitch bstart (* (- 1 bypass) vac_new) vac0))
(write-history h_ramp (gswitch bstart (* (- 1 bypass) (* ramp1 ping_mode)) (read-history h_ramp)))
(def lpg_gain (pl-hold (gswitch bypass 1 vac_new) bstart))
(def lpg_f (pl-hold (gswitch bypass 0.5 (+ 0.003 (* 0.3 vac_4) (* hf 0.04))) bstart))
(def lpg_hf (pl-hold (gswitch bypass 0 (* (+ vac_tail2 (* (- 1 vac_tail2) hf)) hf hf)) bstart))

; ---- outputs ----
(def out_ch (pl-post eng_out out_gain bypass lpg_gain lpg_f lpg_hf bstart bphase))
(def aux_ch (pl-post eng_aux aux_gain bypass lpg_gain lpg_f lpg_hf bstart bphase))
(def mixed (mix out_ch aux_ch (pl-interp (clip (mod aux_mix) 0 1) bstart bphase 0)))
(def out_level (pl-interp (* (db-amp (clip (mod level_db) -60 12)) (gswitch gate_mode 1 vel)) bstart bphase 0))
(def voice_left (* mixed out_level))
(def voice_right voice_left)
(out voice_left 1 @name left)
(out voice_right 2 @name right)
(use-defmacro voice-amp)
(use-defmacro release-window)
(out (voice-amp (release-window gate 30) voice_left voice_right) 3 @name amp @amp true)
