; Revsynt — an FM stab fed into a per-voice tonal reverb, promoted from the
; user-library fullrevfm patch. Two cosine operators share a slow triangle
; shaper swept at fm_rate; the second is detuned by unison semitones. The
; stab (fixed 19 ms attack, stab_decay_ms decay) feeds a feedback delay tuned
; to the note period (times 2^octave) followed by an eight-stage allpass
; diffuser whose lengths track the same period, scaled by size. Two copies of
; the tank run on two delay taps, and a settle-then-crossfade controller
; (pitchless_delay_control) moves one tap at a time so note changes never
; pitch-bend the ringing tail. The amp envelope gates the tank output rather
; than its input, so a retriggered voice can reveal the previous note's tail:
; that gating is the point, keep it.
;
; Changes vs fullrevfm: the octave multiplier no longer runs through a
; zero-initialised one-pole smoother (a fresh voice used to spend its first
; note with a near-zero delay time and no tail); the allpass size is a
; modulatable `size` param with a cents-style `size_fine` trim; feedback and
; wet are exposed; the amp envelope
; uses flat factory names.

(use-defmacro allpass-diffuser)
(use-defmacro pitch-transpose)
(use-defmacro pitchless_delay_control)

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(param fm_rate @default 1.06 @min 0.1 @max 8 @unit Hz @mod true @mod-mode additive @mod-depth-min -8 @mod-depth-max 8 @mod-unit Hz)
(param unison @default 0.27 @min 0 @max 1 @unit st @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1 @mod-unit st)
(param stab_decay_ms @default 320 @min 20 @max 2000 @unit ms)
(param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param octave @default 0 @min -2 @max 2)
(param fade_s @default 2.34 @min 0.1 @max 4 @unit s)
(param feedback @default 0.6 @min 0 @max 0.95 @mod true @mod-mode additive @mod-depth-min -0.95 @mod-depth-max 0.95)
(param damping @default 2650 @min 100 @max 6000 @unit Hz @mod true @mod-mode additive @mod-depth-min -3000 @mod-depth-max 3000 @mod-unit Hz)
(param size @default 1 @min 0.1 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
(param size_fine @default 0 @min -100 @max 100 @unit cents @mod true @mod-mode additive @mod-depth-min -100 @mod-depth-max 100 @mod-unit cents)
(param wet @default 0.9 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param amp_attack_ms @default 20 @min 0 @max 1000 @unit ms)
(param amp_decay_ms @default 65 @min 1 @max 2000 @unit ms)
(param amp_sustain @default 1 @min 0 @max 1)
(param amp_release_ms @default 20 @min 1 @max 5000 @unit ms)

;; One operator: a cosine whose phase runs through a triangle shaper. The
;; shaper's skew is the shared slow phasor, so the operator's timbre sweeps.
(defmacro fm-op (freq shape)
  (def ph (phasor freq))
  (def tri (triangle ph shape))
  (cos (* tri twopi)))

;; Two operators (the second detuned by `detune_st` semitones) under the
;; percussive stab envelope.
(defmacro fm-stab (freq gate_sig trig_sig shape detune_st decay_ms vel level)
  (def stab_env (adsr gate_sig trig_sig 19 decay_ms 0.01 300))
  (def op_a (fm-op freq shape))
  (def op_b (fm-op (pitch-transpose freq detune_st) shape))
  (def pair (mix op_a op_b 0.4))
  (* pair stab_env vel level))

;; Note period in samples: the tank is tuned to the played note.
(defmacro hz-to-samples (freq sr)
  (def clipped (clip freq 0.1 22000))
  (/ sr clipped))

;; Feedback delay with a lowpass in the loop and a DC blocker on the output.
(defmacro damped-feedback-delay (input delay_time fb damp_hz dry_wet)
  (make-history dc_in_hist)
  (make-history dc_out_hist)
  (make-history fb_hist)
  (def delay_in (+ input (* (read-history fb_hist) fb)))
  (def delayed_sig (delay delay_in delay_time))
  (def dampened (svf delayed_sig damp_hz 0.707 0))
  (def dc_blocked (- (+ dampened (* 0.995 (read-history dc_out_hist))) (read-history dc_in_hist)))
  (def mixed (mix input dc_blocked dry_wet))
  (write-history dc_in_hist dampened)
  (write-history dc_out_hist dc_blocked)
  (write-history fb_hist dc_blocked)
  (tuple mixed dc_blocked))

;; One-pole smoother for the diffuser size. Knob moves arrive once per block
;; and would otherwise jump all eight allpass lengths at once (a click);
;; 5 ms is enough to make knob drags clean (measured: worst-case sample step
;; 0.16 -> 0.008) while a 10 Hz modulator still passes at 95 %. Seeded to the target on the voice's first sample so
;; a fresh voice never sweeps up from zero (the fullrevfm first-note bug).
(defmacro smooth-size (target tau_s)
  (make-history size_hist)
  (make-history size_init)
  (def seeded (gswitch (read-history size_init) (read-history size_hist) target))
  (def coeff (- 1.0 (exp (/ -1.0 (* tau_s samplerate)))))
  (def smoothed (+ seeded (* coeff (- target seeded))))
  (write-history size_hist smoothed)
  (write-history size_init 1)
  smoothed)

;; One reverb tank: tuned feedback delay into the allpass diffuser. The
;; diffuser lengths follow the delay (period / 10) scaled by `apsize`, capped
;; so the longest stage stays inside the delay op's 88000-sample buffer.
(defmacro reverb-tank (input wet_amt delaysamples fb damp_hz apsize)
  (def ap_size (min (* (/ delaysamples 10) apsize) 90))
  (def (mixed dc_blocked) (damped-feedback-delay input delaysamples fb damp_hz 0.6))
  (def tank_in (mix mixed dc_blocked 0.5))
  (def (diff_l diff_r) (allpass-diffuser tank_in tank_in 0.734 ap_size))
  (tuple (mix input diff_l wet_amt) (mix input diff_r wet_amt)))

(def shape (phasor (mod fm_rate)))
(def stab (fm-stab pitch gate trigger shape (mod unison) stab_decay_ms velocity (mod gain)))
(def period (hz-to-samples pitch samplerate))
(def target (* period (pow 2 octave)))
(def fade_rate (/ 1 (* fade_s samplerate)))
(def (tap_a tap_b fade) (pitchless_delay_control target fade_rate))
;; Coarse multiplier times a cents-style fine ratio: +-100 cents is +-6 %
;; of every allpass length, the range where the tail's coloration retunes
;; rather than smears.
(def size_target (* (mod size) (pow 2 (/ (mod size_fine) 1200))))
(def size_smooth (smooth-size size_target 0.005))
(def (a_l a_r) (reverb-tank stab (mod wet) tap_a (mod feedback) (mod damping) size_smooth))
(def (b_l b_r) (reverb-tank stab (mod wet) tap_b (mod feedback) (mod damping) size_smooth))
(def env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def out_l (* env (mix a_l b_l fade)))
(def out_r (* env (mix a_r b_r fade)))

(out out_l 1 @name audio)
(out out_r 2 @name audio-2)
