; Digiwave — the Monomachine DPRO-DDRW "doubledraw" wavetable machine, revived
; from monomachine/dpro/monomachine-dpro-ddrw-v1 in the factory
; macro-vocabulary style (eseq-i2pw). Two oscillators each pick one of 8
; single-cycle banks (64 AKWF waves each; bank 0 is the ddrw-v1 bank
; verbatim) and scan it with a shared slew "time" that
; glides the wave position instead of snapping it, per-oscillator bit
; crushing, a semitone-based width offset for the second oscillator, and a
; crossfade mix — then the Monomachine track chain the original lacked:
; ring/AM, dual-mode filter with its own envelope, sample-rate reducer,
; +/-30 dB one-band EQ, and the shared drive stage.
;
; Patch-editor structure mirrors factory/poseidon: the top level is section
; macro nodes with no bare math; non-modulated params are declared inside the
; section that owns them; host-modulatable params stay top-level and reach
; their sections as inline (mod p) arguments.
;
; Changes vs ddrw-v1 (the original is untouched):
; - `wid` (unitless 0..127 mapped to 0..24 st) is now `width_st`, semitones.
; - Wave indices are 0-based (0..63) so the viewer and the DSP agree; the
;   whole bank stays one modulatable sweep per oscillator, as on the DDRW.
;   bank1/bank2 (0..7, not modulated) pick which 64-wave bank each draw scans.
; - Filter is the poseidon two-mode block (LP24 res / LP12+HP) with the shared
;   crossfade drive stage; resonance is 0..1, drive 0..1 (linear at 0).
; - New: am_rate/am_depth, srr, eq_freq/eq_gain_db/eq_q, glide, spread,
;   volume_db output staging.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(def bank (tensor @shape [512 512] @file "waves/bank.json"))

; ======================================================================
; host-modulatable params (must live at top level)
; ======================================================================

(param wave1 @default 8 @min 0 @max 63 @mod true @mod-mode additive @mod-depth-min -63 @mod-depth-max 63)
(param wave2 @default 37 @min 0 @max 63 @mod true @mod-mode additive @mod-depth-min -63 @mod-depth-max 63)
(param mix @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param crush1 @default 0 @min 0 @max 11 @mod true @mod-mode additive @mod-depth-min -11 @mod-depth-max 11)
(param crush2 @default 0 @min 0 @max 11 @mod true @mod-mode additive @mod-depth-min -11 @mod-depth-max 11)
(param width_st @default 0 @min 0 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param tune_cents @default 0 @min -100 @max 100 @unit cents @mod true @mod-mode additive @mod-depth-min -100 @mod-depth-max 100 @mod-unit cents)
(param cutoff @default 7200 @min 20 @max 18000 @unit Hz @mod true @mod-mode additive @mod-depth-min -8000 @mod-depth-max 8000 @mod-unit Hz)
(param resonance @default 0.1 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param filter_env_amt @default 1800 @min -8000 @max 8000 @unit Hz @mod true @mod-mode additive @mod-depth-min -8000 @mod-depth-max 8000 @mod-unit Hz)
(param drive @default 0.05 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param am_rate @default 55 @min 0.1 @max 2200 @unit Hz @mod true @mod-mode additive @mod-depth-min -1000 @mod-depth-max 1000 @mod-unit Hz)
(param am_depth @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param srr @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param eq_freq @default 1200 @min 40 @max 11000 @unit Hz @mod true @mod-mode additive @mod-depth-min -5000 @mod-depth-max 5000 @mod-unit Hz)
(param eq_gain_db @default 0 @min -30 @max 30 @unit dB @mod true @mod-mode additive @mod-depth-min -30 @mod-depth-max 30 @mod-unit dB)
(param volume_db @default -14 @min -36 @max 6 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)

; ======================================================================
; math / state helpers (leaf macros, collapsed inside section layers)
; ======================================================================

(defmacro semi-ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro db-amp (db)
  (exp (* 0.1151292546 db)))

; The shared drive stage: crossfade from pure linear (amount 0, transparent)
; into peak-normalized tanh saturation (amount 1, compressed + harmonics).
(defmacro drive-stage (x amount)
  (def g (+ 1.0 (* amount 7.0)))
  (def shaped (/ (tanh (* x g)) (tanh g)))
  (mix x shaped amount))

; Uniform requantizer: `crush` 0 leaves 12 bits (transparent), 11 leaves 1 bit.
; (floor on runtime scalars needs dgenlisp >= v0.1.6; content/dgenlisp.lock
; pins v0.1.7. An older local dgen build silently makes this a no-op.)
(defmacro bit-crush (sig crush_v)
  (def levels (pow 2 (clip (- 12 crush_v) 1 12)))
  (def u (* (+ (clip sig -1 1) 1) 0.5 levels))
  (- (* (/ (floor u) levels) 2) 1))

; ======================================================================
; shared cores (param-less; instantiated per oscillator)
; ======================================================================

; Doubledraw oscillator core: the wave position glides toward its target
; with the shared slew coefficient (1.0 = snap), reads the 64-wave bank with
; a free-running phasor, then requantizes. Phase never resets, so retriggers
; never click; the glide starting from wherever the position last sat is
; part of the DDRW character (a long `time` sweeps in from the last note).
(defmacro draw-osc (bank_t bank_idx wave_target slew_coeff freq crush_v)
  (def target (+ (* (clip (round bank_idx) 0 7) 64) (clip wave_target 0 63)))
  (make-history pos_hist)
  (def pos_prev (read-history pos_hist))
  (def pos (+ pos_prev (* slew_coeff (- target pos_prev))))
  (write-history pos_hist pos)
  (def raw (wavetable-read bank_t (clip pos 0 511) (phasor freq)))
  (bit-crush raw crush_v))

; ======================================================================
; section macros (one collapsed node each at the top level)
; ======================================================================

; Draw 1: bank select + the shared wave-scan core.
(defmacro osc-one (bank_t wave_v slew_coeff freq crush_v)
  (param bank1 @default 0 @min 0 @max 7)
  (draw-osc bank_t bank1 wave_v slew_coeff freq crush_v))

; Draw 2: same core, its own bank.
(defmacro osc-two (bank_t wave_v slew_coeff freq crush_v)
  (param bank2 @default 0 @min 0 @max 7)
  (draw-osc bank_t bank2 wave_v slew_coeff freq crush_v))

; Exponential glide toward the played pitch; bypasses when glide time ~0.
(defmacro glide-to (target)
  (param glide_ms @default 0 @min 0 @max 1000 @unit ms)
  (def coeff (exp (/ -1.0 (max 1.0 (* glide_ms 0.001 samplerate)))))
  (make-history gl_hist)
  (def prev (read-history gl_hist))
  (def glided (+ (* target (- 1 coeff)) (* prev coeff)))
  (def held (gswitch (gt glide_ms 0.1) glided target))
  (write-history gl_hist held)
  held)

; Oscillator frequency: base pitch offset by fine tune (cents) plus a
; semitone offset (the DDRW "width" for oscillator 2; 0 for oscillator 1).
(defmacro tune-pitch (hz cents st)
  (* hz (semi-ratio (+ (/ cents 100.0) st))))

; DDRW `time`: how long the wave position takes to reach a new target.
; 0 snaps; otherwise a one-pole coefficient for 0..650 ms.
(defmacro draw-time ()
  (param time @default 18 @min 0 @max 127)
  (def time_ms (* (/ (clip time 0 127) 127) 650))
  (gswitch (gt time 0.5)
           (- 1.0 (exp (/ -1.0 (* (+ time_ms 1.0) 0.001 samplerate))))
           1.0))

; Equal-gain crossfade between the two draws.
(defmacro draw-mix (o1 o2 mix_v)
  (def m (clip mix_v 0 1))
  (+ (* o1 (- 1 m)) (* o2 m)))

; Amp ADSR (owns its times).
(defmacro amp-env (gate_s trig_s)
  (param amp_attack_ms @default 3 @min 1 @max 5000 @unit ms)
  (param amp_decay_ms @default 120 @min 1 @max 5000 @unit ms)
  (param amp_sustain @default 0.78 @min 0 @max 1)
  (param amp_release_ms @default 90 @min 1 @max 5000 @unit ms)
  (adsr gate_s trig_s amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))

; Filter ADSR (owns its times).
(defmacro filter-env (gate_s trig_s)
  (param filter_attack_ms @default 6 @min 1 @max 5000 @unit ms)
  (param filter_decay_ms @default 260 @min 1 @max 5000 @unit ms)
  (param filter_sustain @default 0.18 @min 0 @max 1)
  (param filter_release_ms @default 180 @min 1 @max 5000 @unit ms)
  (adsr gate_s trig_s filter_attack_ms filter_decay_ms filter_sustain filter_release_ms))

; Monomachine AM: ring-modulate against a free sine, mixed in by depth.
(defmacro mnm-am (x rate_hz depth)
  (mix x (* x (sin (* twopi (phasor (clip rate_hz 0.05 3000))))) (clip depth 0 1)))

; Filter block: shared drive stage into mode A (24dB resonant LP, drive-
; faded interstage clip) or mode B (12dB LP + 12dB HP). Cutoff is keytracked
; (Hz per Hz of pitch, as ddrw-v1) and pushed by the filter envelope in Hz.
; Linear at drive 0 — the drive knob owns the color.
(defmacro digiwave-filter (x base_hz cutoff_v res_v env_amt feg_s drive_v)
  (param filter_mode @default 0 @min 0 @max 1)
  (param hp_freq @default 20 @min 10 @max 8000 @unit Hz)
  (param keytrack @default 0.18 @min 0 @max 2)
  (def cut (clip (+ cutoff_v (* base_hz keytrack) (* feg_s env_amt)) 20 16000))
  (def res (clip res_v 0 1))
  (def driven (drive-stage x (clip drive_v 0 1)))
  ; mode A: 24dB resonant LP — resonance on the first stage, interstage
  ; soft clip faded with drive so drive 0 stays linear
  (def lp_a1 (svf driven cut (+ 0.55 (* res 9.0)) 0))
  (def a_mid (mix lp_a1 (/ (tanh (* lp_a1 0.9)) 0.9) (clip drive_v 0 1)))
  (def lp_a (svf a_mid cut 0.707 0))
  ; mode B: 12dB LP + 12dB HP
  (def lp_b1 (svf driven cut (+ 0.55 (* res 6.0)) 0))
  (def lp_b (svf lp_b1 (clip hp_freq 10 8000) 0.6 2))
  (gswitch (gte filter_mode 0.5) lp_b lp_a))

; Monomachine sample-rate reducer: sample-and-hold at a rate that falls
; exponentially from the host rate (amount 0, transparent) to 300 Hz.
(defmacro mnm-srr (x amount)
  (def amt (clip amount 0 1))
  (def hold_hz (* samplerate (pow (/ 300.0 samplerate) amt)))
  (make-history ph_hist)
  (make-history val_hist)
  (def prev_ph (read-history ph_hist))
  (def ph (wrap (+ prev_ph (/ hold_hz samplerate)) 0 1))
  (def wrapped (lt ph prev_ph))
  (write-history ph_hist ph)
  (def held (gswitch (max wrapped (lte amt 0.001)) x (read-history val_hist)))
  (write-history val_hist held)
  held)

; Monomachine one-band EQ: +/-30 dB peaking boost/cut. The preamble svf's
; bandpass peaks at gain q at cutoff, so divide by q for a unity bandpass.
(defmacro mnm-eq (x freq gain_db)
  (param eq_q @default 2.2 @min 0.5 @max 12)
  (+ x (* (- (db-amp (clip gain_db -30 30)) 1.0)
          (/ 1.0 eq_q)
          (svf x (clip freq 40 12000) eq_q 1))))

; Amp EG, velocity, per-voice pan spread, volume, soft-limited stereo out.
; -> (left right)
(defmacro output-stage (x aeg_s vel vol_db trig)
  (param vel_to_amp @default 0.6 @min 0 @max 1)
  (param spread @default 0.1 @min 0 @max 1)
  (def vel_gain (+ (- 1 vel_to_amp) (* vel vel_to_amp)))
  (def vol (db-amp (clip vol_db -36 6)))
  (def amp (* x aeg_s vel_gain vol))
  (def pan (clip (* (latch (noise) trig) spread) -1 1))
  (tuple (tanh (* amp (clip (- 1 (* pan 0.5)) 0 1.5)))
         (tanh (* amp (clip (+ 1 (* pan 0.5)) 0 1.5)))))

; ======================================================================
; voice: section nodes only
; ======================================================================

(def base_pitch (glide-to pitch))
(def hz1 (tune-pitch base_pitch (mod tune_cents) 0))
(def hz2 (tune-pitch base_pitch (mod tune_cents) (mod width_st)))

(def slew (draw-time))
(def osc1 (osc-one bank (mod wave1) slew hz1 (mod crush1)))
(def osc2 (osc-two bank (mod wave2) slew hz2 (mod crush2)))
(def drawn (draw-mix osc1 osc2 (mod mix)))

(def aeg (amp-env gate trigger))
(def feg (filter-env gate trigger))

(def ringed (mnm-am drawn (mod am_rate) (mod am_depth)))
(def filt (digiwave-filter ringed base_pitch (mod cutoff) (mod resonance)
                           (mod filter_env_amt) feg (mod drive)))
(def crushed (mnm-srr filt (mod srr)))
(def shaped (mnm-eq crushed (mod eq_freq) (mod eq_gain_db)))

(def (left right) (output-stage shaped aeg velocity (mod volume_db) trigger))

(out left 1 @name left)
(out right 2 @name right)
