; E-mu Orbit-9090 tom ("66.wav"), identified from the sample-library hit by
; SynthID-style scalar optimisation (dgen Examples/SynthID/scripts/
; fit_emu_tom.py). At the defaults every hit reproduces the learned render;
; every knob is a departure from the identified sound. No sample,
; target-derived table, FIR, or residual is embedded.
;
; Source provenance: 66.wav, tags EMU / EMU Orbit-9090,
; sha256 1f29dd660fe4dbc8b69915baee9ff47881db5dd808097f99469d9beb6999824c
; (44.1 kHz 16-bit mono, 222 ms).
;
; Voice: a fast two-exponential carrier sweep (~2.9 kHz, held 4 ms, down to
; ~155 Hz) with a modulator locked to it at ~0.56 (an FM tom: both operators
; on one pitch envelope) drives a bank of FM-style sidebands at
; fc + (k-1) fm, k = 0..6: the lower sideband fc - fm, the carrier, and
; five upper sidebands, each with its own level, extra decay and phase. The
; measured tail is 68 / 155 / 242 / 329 / 415 / 502 Hz - evenly spaced but
; offset from zero, which no harmonic bank can make and is what separates
; this tom from an 808 tom. Capped click / noise blocks carry whatever the
; sweep does not; gain-normalised tanh output.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; ---- departures from the identified sound (all no-ops at their defaults) ----
(param tune @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive)
(param ratio @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param sweep @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param attack @default 1 @min 0.1 @max 4 @mod true @mod-mode additive)
(param decay @default 1 @min 0.1 @max 4 @mod true @mod-mode additive)
(param harm @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param bright @default 1 @min 0.5 @max 1.5 @mod true @mod-mode additive)
(param noise @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param drive @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive)

; ---- the identified scalars (recovered_params.json), editable ----
(param onset @default 0.00157773 @min 0 @max 0.004 @unit s)
(param c_end @default 154 @min 100 @max 250 @unit Hz)
(param c_a1 @default 2016.43 @min 500 @max 8000 @unit Hz)
(param c_r1 @default -1138.86 @min -1200 @max -100)
(param c_hold @default 0.004 @min 0 @max 0.006 @unit s)
(param c_a2 @default 466.632 @min 10 @max 1200 @unit Hz)
(param c_r2 @default -81 @min -250 @max -10)
(param m_ratio @default 0.561 @min 0.3 @max 0.9)
(param m_a1 @default 0.154202 @min 0.1 @max 600 @unit Hz)
(param m_r1 @default -1489.83 @min -1500 @max -20)
(param attack_time @default 0.00224322 @min 0.0002 @max 0.03 @unit s)
(param sb_attack @default 0.00307133 @min 0.0002 @max 0.03 @unit s)
(param amp_decay @default -10.9499 @min -40 @max -0.5)
(param amp_curve @default -40.7128 @min -150 @max 0)
(param body_amp @default 0.736226 @min 0.1 @max 2.5)
(param h0 @default 0.77084 @min 0.0001 @max 1.5)
(param h2 @default 0.780505 @min 0.0001 @max 1.5)
(param h3 @default 0.293939 @min 0.0001 @max 1.5)
(param h4 @default 0.0885425 @min 0.0001 @max 1.5)
(param h5 @default 0.0268099 @min 0.0001 @max 1.5)
(param h6 @default 0.0275785 @min 0.0001 @max 1.5)
(param d0 @default -2.24688 @min -80 @max 20)
(param d2 @default -0.891255 @min -80 @max 20)
(param d3 @default -0.901467 @min -80 @max 20)
(param d4 @default -0.5 @min -80 @max 20)
(param d5 @default -0.266464 @min -80 @max 20)
(param d6 @default -7.11048 @min -80 @max 20)
(param p0 @default 0.353576 @min 0 @max 1)
(param p2 @default 0.0194386 @min 0 @max 1)
(param p3 @default 0.0676752 @min 0 @max 1)
(param p4 @default 0.12312 @min 0 @max 1)
(param p5 @default 0.309547 @min 0 @max 1)
(param p6 @default 0.177634 @min 0 @max 1)
(param click_freq @default 4709 @min 300 @max 6000 @unit Hz)
(param click_amp @default 0.00884845 @min 0 @max 0.02)
(param click_decay @default -300 @min -8000 @max -300)
(param noise_cutoff @default 5786.48 @min 300 @max 12000 @unit Hz)
(param noise_amp @default 0.05 @min 0 @max 0.15)
(param noise_decay @default -100 @min -2000 @max -100)
(param out_drive @default 1.28515 @min 0.05 @max 4)
(param out_gain @default 0.66 @min 0.05 @max 5)

(defmacro semi (st) (pow 2 (/ st 12)))
(defmacro bq-hz (hz) (* hz (/ 44100.0 samplerate)))

; Exact seconds-since-trigger clock: t=0 on the trigger sample, then n/sr.
(make-history time-h)
(def previous-time (read-history time-h))
(def t-raw (gswitch (gt trigger 0.5) 0.0 previous-time))
(write-history time-h (+ t-raw (/ 1.0 samplerate)))
; The sample has a short silence before the hit; t counts from its onset.
(def on (gswitch (gt t-raw onset) 1.0 0.0))
(def t (* on (- t-raw onset)))

; The fit was rendered with the host pitch at C4 (261.63 Hz); this ratio
; keeps that render exact while making the complete voice playable.
(def pitch-ratio (* (/ pitch 261.63) (semi (mod tune))))
(def sweep-scale (/ 1.0 (clip (mod sweep) 0.25 4)))
(def cr1 (* c_r1 sweep-scale))
(def cr2 (* c_r2 sweep-scale))
(def mr1 (* m_r1 sweep-scale))
; The fast fall SNAP holds at its start for c_hold seconds (the sample sits
; at ~2.9 kHz for 3 ms before dropping), then falls exponentially.
(def hold-time (* c_hold (clip (mod sweep) 0.25 4)))
(def t-after-hold (gswitch (gt t hold-time) (- t hold-time) 0.0))
(def t-in-hold (gswitch (gt t hold-time) hold-time t))
(def carrier-phase
  (* pitch-ratio
     (+ (* c_end t)
        (* c_a1 t-in-hold)
        (* (/ c_a1 cr1) (- (exp (* cr1 t-after-hold)) 1.0))
        (* (/ c_a2 cr2) (- (exp (* cr2 t)) 1.0)))))
; The modulator is locked to the carrier (an FM tom: both operators ride one
; pitch envelope) at m_ratio, plus its own small extra fall. The ratio knob
; scales the spacing alone: the sidebands move, the carrier stays.
(def mod-phase
  (* (clip (mod ratio) 0.25 4)
     (+ (* m_ratio carrier-phase)
        (* pitch-ratio (/ m_a1 mr1) (- (exp (* mr1 t)) 1.0)))))

(def attack-seconds (* attack_time (clip (mod attack) 0.1 4)))
(def attack-env
  (/ (- 1.0 (exp (/ (- t) attack-seconds)))
     (- 1.0 (exp (/ -0.05 attack-seconds)))))
(def decay-scale (/ 1.0 (clip (mod decay) 0.1 4)))
(def body-env
  (* on attack-env
     (exp (+ (* amp_decay decay-scale t)
             (* amp_curve decay-scale t t)))))

; Sideband bank. Partial k sits at carrier + (k-1) x modulator; harm scales
; every sideband, bright tilts them by distance from the carrier (both exact
; no-ops at 1).
; The sidebands grow in over the first milliseconds (the modulation index
; rises; the sample's first cycles are nearly a bare sine).
(def sb-rise (- 1.0 (exp (/ (- t) sb_attack))))
(def harm-scale (* sb-rise (clip (mod harm) 0 4)))
(def tilt (clip (mod bright) 0.5 1.5))
(def carrier-frac (- carrier-phase (floor carrier-phase)))
(defmacro sideband-phase (k phase)
  (+ carrier-phase (* (- k 1) mod-phase) phase))
(def ph0 (sideband-phase 0 p0))
(def ph2 (sideband-phase 2 p2))
(def ph3 (sideband-phase 3 p3))
(def ph4 (sideband-phase 4 p4))
(def ph5 (sideband-phase 5 p5))
(def ph6 (sideband-phase 6 p6))
(defmacro sideband (ph dist level rate)
  (* level (pow tilt dist) (exp (* rate t)) (sin (* 2.0 pi (- ph (floor ph))))))
(def bank
  (+ (sin (* 2.0 pi carrier-frac))
     (* harm-scale
        (+ (sideband ph0 1 h0 d0)
           (sideband ph2 1 h2 d2)
           (sideband ph3 2 h3 d3)
           (sideband ph4 3 h4 d4)
           (sideband ph5 4 h5 d5)
           (sideband ph6 5 h6 d6)))))
(def body (* bank body-env body_amp))

(def click-voice
  (* on (sin (* 2.0 pi click_freq pitch-ratio t))
     (exp (* click_decay t)) click_amp))
(def bipolar-noise (- (* (noise) 2.0) 1.0))
(def filtered-noise
  (biquad bipolar-noise (bq-hz noise_cutoff) 0.707 1.0 0))
(def noise-voice
  (* on filtered-noise (exp (* noise_decay t)) noise_amp
     (clip (mod noise) 0 4)))

(def mixed (+ body click-voice noise-voice))
; Gain-normalised saturator: out_drive / drive set the shape only.
(def drive-amount (* out_drive (clip (mod drive) 0.25 4)))
(def shaped
  (* (/ (tanh (* mixed drive-amount)) drive-amount) out_gain))
(out (* shaped (clip velocity 0 1) (clip (mod level) 0 1.5)) 1 @name audio)
