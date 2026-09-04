; Access Virus B BassDrum_23, identified from the sample-library hit by
; SynthID-style scalar optimisation (dgen Examples/SynthID/scripts/
; fit_virus_kick.py). At the defaults every hit reproduces the learned render;
; every knob is a departure from the identified sound. No sample,
; target-derived table, FIR, or residual is embedded.
;
; Source provenance: BassDrum_23.wav, tags Access / Access Virus - B,
; sha256 32cee493358b8dd6e60a5e82761c21b2f7e42445f9488b522bb805668d5579cf
; (native 32.5 kHz; the 44.1 kHz library duplicate is the same signal
; resampled).
;
; Voice: a two-exponential pitch sweep drives an additive bank of ten
; harmonics. Each harmonic above the fundamental has its own level and its
; own extra decay on top of the shared attack / log-quadratic body envelope
; (the ladder H2 ~ -25 dB, H3 ~ -40, H4 ~ -43, H7 ~ -47 is the sound's
; character; the v1-v3 sine-with-decoration voice missed it). A decaying sine
; click and a lowpassed noise burst carry the first 20 ms of non-harmonic
; transient; a slowly decaying high-passed hiss is the recording /
; machine texture (-70 dBFS, under the gate metric's floor, audible as the
; sample's air); gain-normalised tanh output.

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
(param sweep @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param attack @default 1 @min 0.1 @max 4 @mod true @mod-mode additive)
(param decay @default 1 @min 0.1 @max 4 @mod true @mod-mode additive)
(param harm @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param bright @default 1 @min 0.5 @max 1.5 @mod true @mod-mode additive)
(param noise @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param hiss @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param drive @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive)

; ---- the identified scalars (recovered_params.json), editable ----
(param f_end @default __FEND__ @min 40 @max 60 @unit Hz)
(param sweep_a1 @default __A1__ @min 400 @max 3000 @unit Hz)
(param sweep_r1 @default __R1__ @min -400 @max -60)
(param sweep_a2 @default __A2__ @min 50 @max 1500 @unit Hz)
(param sweep_r2 @default __R2__ @min -150 @max -15)
(param attack_time @default __ATTACKTIME__ @min 0.003 @max 0.3 @unit s)
(param amp_decay @default __AMPDECAY__ @min -40 @max -0.5)
(param amp_curve @default __AMPCURVE__ @min -100 @max 0)
(param body_amp @default __BODYAMP__ @min 0.2 @max 2.5)
(param h2 @default __H2__ @min 0.0001 @max 0.3)
(param h3 @default __H3__ @min 0.0001 @max 0.3)
(param h4 @default __H4__ @min 0.0001 @max 0.3)
(param h5 @default __H5__ @min 0.0001 @max 0.3)
(param h6 @default __H6__ @min 0.0001 @max 0.3)
(param h7 @default __H7__ @min 0.0001 @max 0.3)
(param h8 @default __H8__ @min 0.0001 @max 0.3)
(param h9 @default __H9__ @min 0.0001 @max 0.3)
(param h10 @default __H10__ @min 0.0001 @max 0.3)
(param d2 @default __D2__ @min -80 @max 20)
(param d3 @default __D3__ @min -80 @max 20)
(param d4 @default __D4__ @min -80 @max 20)
(param d5 @default __D5__ @min -80 @max 20)
(param d6 @default __D6__ @min -80 @max 20)
(param d7 @default __D7__ @min -80 @max 20)
(param d8 @default __D8__ @min -80 @max 20)
(param d9 @default __D9__ @min -80 @max 20)
(param d10 @default __D10__ @min -80 @max 20)
(param click_freq @default __CLICKFREQ__ @min 300 @max 4000 @unit Hz)
(param click_amp @default __CLICKAMP__ @min 0 @max 0.02)
(param click_decay @default __CLICKDECAY__ @min -8000 @max -50)
(param noise_cutoff @default __NOISECUTOFF__ @min 300 @max 12000 @unit Hz)
(param noise_amp @default __NOISEAMP__ @min 0 @max 0.05)
(param noise_decay @default __NOISEDECAY__ @min -800 @max -5)
(param hiss_cutoff @default __HISSCUTOFF__ @min 2000 @max 12000 @unit Hz)
(param hiss_amp @default __HISSAMP__ @min 0 @max 0.01)
(param hiss_decay @default __HISSDECAY__ @min -60 @max -2)
(param out_drive @default __DRIVE__ @min 0.05 @max 4)
(param out_gain @default __OUTGAIN__ @min 0.05 @max 5)

(defmacro semi (st) (pow 2 (/ st 12)))
(defmacro bq-hz (hz) (* hz (/ 44100.0 samplerate)))

; Exact seconds-since-trigger clock: t=0 on the trigger sample, then n/sr.
(make-history time-h)
(def previous-time (read-history time-h))
(def t (gswitch (gt trigger 0.5) 0.0 previous-time))
(write-history time-h (+ t (/ 1.0 samplerate)))

; The fit was rendered with the host pitch at C4 (261.63 Hz); this ratio
; keeps that render exact while making the complete voice playable.
(def pitch-ratio (* (/ pitch 261.63) (semi (mod tune))))
(def sweep-scale (/ 1.0 (clip (mod sweep) 0.25 4)))
(def r1 (* sweep_r1 sweep-scale))
(def r2 (* sweep_r2 sweep-scale))
(def sweep-phase
  (* pitch-ratio
     (+ (* f_end t)
        (* (/ sweep_a1 r1) (- (exp (* r1 t)) 1.0))
        (* (/ sweep_a2 r2) (- (exp (* r2 t)) 1.0)))))
(def phase-frac (- sweep-phase (floor sweep-phase)))

(def attack-seconds (* attack_time (clip (mod attack) 0.1 4)))
(def attack-env
  (/ (- 1.0 (exp (/ (- t) attack-seconds)))
     (- 1.0 (exp (/ -0.05 attack-seconds)))))
(def decay-scale (/ 1.0 (clip (mod decay) 0.1 4)))
(def body-env
  (* attack-env
     (exp (+ (* amp_decay decay-scale t)
             (* amp_curve decay-scale t t)))))

; Harmonic bank. harm scales every partial above the fundamental; bright
; tilts them (bright^(k-1)), both exact no-ops at 1.
(def harm-scale (clip (mod harm) 0 4))
(def tilt (clip (mod bright) 0.5 1.5))
(defmacro partial (k level rate)
  (* level (pow tilt (- k 1)) (exp (* rate t)) (sin (* 2.0 pi k phase-frac))))
(def bank
  (+ (sin (* 2.0 pi phase-frac))
     (* harm-scale
        (+ (partial 2 h2 d2)
           (partial 3 h3 d3)
           (partial 4 h4 d4)
           (partial 5 h5 d5)
           (partial 6 h6 d6)
           (partial 7 h7 d7)
           (partial 8 h8 d8)
           (partial 9 h9 d9)
           (partial 10 h10 d10)))))
(def body (* bank body-env body_amp))

(def click-voice
  (* (sin (* 2.0 pi click_freq pitch-ratio t))
     (exp (* click_decay t)) click_amp))
(def bipolar-noise (- (* (noise) 2.0) 1.0))
(def filtered-noise
  (biquad bipolar-noise (bq-hz noise_cutoff) 0.707 1.0 0))
(def noise-voice
  (* filtered-noise (exp (* noise_decay t)) noise_amp
     (clip (mod noise) 0 4)))

; Recording / machine hiss: high-passed noise with its own slow decay,
; through the Virus B's fixed 16.25 kHz output band (two 16 kHz lowpasses).
(def hiss-hp (biquad bipolar-noise (bq-hz hiss_cutoff) 0.707 1.0 1))
(def hiss-band
  (biquad (biquad hiss-hp (bq-hz 16000.0) 0.707 1.0 0) (bq-hz 16000.0) 0.707 1.0 0))
(def hiss-voice
  (* hiss-band (exp (* hiss_decay t)) hiss_amp (clip (mod hiss) 0 4)))

(def mixed (+ body click-voice noise-voice hiss-voice))
; Gain-normalised saturator: out_drive / drive set the shape only.
(def drive-amount (* out_drive (clip (mod drive) 0.25 4)))
(def shaped
  (* (/ (tanh (* mixed drive-amount)) drive-amount) out_gain))
(out (* shaped (clip velocity 0 1) (clip (mod level) 0 1.5)) 1 @name audio)
