; Roland R-8 'Kick03', identified from the sample-library hit by SynthID-style
; scalar optimisation (dgen Examples/SynthID/scripts/fit_r8_kick.py). At the
; defaults every hit reproduces the learned render; every knob is a departure
; from the identified sound. No sample, target-derived table, FIR, or residual
; is embedded.
;
; Source provenance: Kick03.wav, tags drums / kick / Roland / Roland R8,
; sha256 26eb639f7d6587382cdc95d297626bef8fac396f5b5ff5fc8892fb805042278e
; (44.1 kHz, 182 ms, no capture band-limit above the natural roll-off).
;
; Voice: the hit is a real drum, not a swept sine. Five inharmonic membrane
; modes (each with its own level, decay, glide depth and initial phase) sit under one shared
; two-exponential tension glide; eight fixed-pitch ring modes (shell / beater
; 'knock', ~300 Hz .. 3.5 kHz) carry the woody attack; a lowpassed noise burst
; is the beater click; noise gated by the membrane's negative half-cycles is
; the rattle heard on every trough; a slowly decaying high-passed hiss is the
; recording texture; gain-normalised tanh output (linear regime).

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
(param glide @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param attack @default 1 @min 0.1 @max 8 @mod true @mod-mode additive)
(param decay @default 1 @min 0.1 @max 4 @mod true @mod-mode additive)
(param knock @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param ring @default 1 @min 0.1 @max 4 @mod true @mod-mode additive)
; How much the ring bank follows the played note: 1 = with the membrane (the
; timbre transposes as one instrument), 0 = the shell modes stay where the
; sample has them (a fixed resonance across notes). Exact no-op at C4 either way.
(param ring_track @default 1 @min 0 @max 1 @mod true @mod-mode additive)
(param noise @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param rattle @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param hiss @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param drive @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive)

; ---- the identified scalars (recovered_params.json), editable ----
(param glide_a1 @default 0.250029 @min 0.05 @max 3)
(param glide_r1 @default -480.911 @min -1200 @max -60)
(param glide_a2 @default 0.263726 @min 0.02 @max 1.5)
(param glide_r2 @default -34.9268 @min -80 @max -8)
(param attack_time @default 0.0001 @min 0.0001 @max 0.006 @unit s)
(param lf1 @default 45.6676 @min 20 @max 400 @unit Hz)
(param lf2 @default 89.6389 @min 20 @max 400 @unit Hz)
(param lf3 @default 127.696 @min 20 @max 400 @unit Hz)
(param lf4 @default 167.715 @min 20 @max 400 @unit Hz)
(param lf5 @default 244.085 @min 20 @max 400 @unit Hz)
(param la1 @default 0.856324 @min 0.001 @max 1)
(param la2 @default 0.989175 @min 0.001 @max 1)
(param la3 @default 0.63755 @min 0.001 @max 1)
(param la4 @default 0.520093 @min 0.001 @max 1)
(param la5 @default 0.11475 @min 0.001 @max 1)
(param ld1 @default -11.463 @min -400 @max -3)
(param ld2 @default -24.241 @min -400 @max -3)
(param ld3 @default -12.504 @min -400 @max -3)
(param ld4 @default -30.7075 @min -400 @max -3)
(param ld5 @default -24.3515 @min -400 @max -3)
(param lg1 @default 2.97554 @min 0 @max 3)
(param lg2 @default 2.51896 @min 0 @max 3)
(param lg3 @default 1.7639 @min 0 @max 3)
(param lg4 @default 2.34637 @min 0 @max 3)
(param lg5 @default 1.9717 @min 0 @max 3)
(param lp1 @default 0.203198 @min 0 @max 1)
(param lp2 @default 0.44 @min 0 @max 1)
(param lp3 @default 0.48266 @min 0 @max 1)
(param lp4 @default 0.444286 @min 0 @max 1)
(param lp5 @default 0.38743 @min 0 @max 1)
(param mf1 @default 348.101 @min 200 @max 6000 @unit Hz)
(param mf2 @default 456.473 @min 200 @max 6000 @unit Hz)
(param mf3 @default 840.253 @min 200 @max 6000 @unit Hz)
(param mf4 @default 1356.64 @min 200 @max 6000 @unit Hz)
(param mf5 @default 2067.64 @min 200 @max 6000 @unit Hz)
(param mf6 @default 2826.18 @min 200 @max 6000 @unit Hz)
(param mf7 @default 3379.52 @min 200 @max 6000 @unit Hz)
(param mf8 @default 4111.14 @min 200 @max 6000 @unit Hz)
(param ma1 @default 0.0765368 @min 0.0001 @max 0.3)
(param ma2 @default 0.0378978 @min 0.0001 @max 0.3)
(param ma3 @default 0.0317308 @min 0.0001 @max 0.3)
(param ma4 @default 0.0261733 @min 0.0001 @max 0.3)
(param ma5 @default 0.0906526 @min 0.0001 @max 0.3)
(param ma6 @default 0.163107 @min 0.0001 @max 0.3)
(param ma7 @default 0.26807 @min 0.0001 @max 0.3)
(param ma8 @default 0.211719 @min 0.0001 @max 0.3)
(param md1 @default -26.3748 @min -800 @max -5)
(param md2 @default -18.883 @min -800 @max -5)
(param md3 @default -21.0564 @min -800 @max -5)
(param md4 @default -25.6401 @min -800 @max -5)
(param md5 @default -50.743 @min -800 @max -5)
(param md6 @default -80.6973 @min -800 @max -5)
(param md7 @default -111.99 @min -800 @max -5)
(param md8 @default -129.162 @min -800 @max -5)
(param noise_cutoff @default 2851.74 @min 500 @max 14000 @unit Hz)
(param noise_amp @default 0.8 @min 0 @max 0.8)
(param noise_decay @default -64.4698 @min -4000 @max -40)
(param rattle_amp @default 0 @min 0 @max 1)
(param rattle_hp @default 630.463 @min 300 @max 4000 @unit Hz)
(param rattle_decay @default -53.5398 @min -300 @max -5)
(param hiss_cutoff @default 2000 @min 2000 @max 12000 @unit Hz)
(param hiss_amp @default 0.00148168 @min 0 @max 0.02)
(param hiss_decay @default -2.8568 @min -80 @max -2)
(param out_drive @default 0.102266 @min 0.02 @max 0.15)
(param out_gain @default 0.467551 @min 0.05 @max 5)

(defmacro semi (st) (pow 2 (/ st 12)))
(defmacro bq-hz (hz) (* hz (/ 44100.0 samplerate)))

; Exact seconds-since-trigger clock: t=0 on the trigger sample, then n/sr.
(make-history time-h)
(def previous-time (read-history time-h))
(def t (gswitch (gt trigger 0.5) 0.0 previous-time))
(write-history time-h (+ t (/ 1.0 samplerate)))

; The fit was rendered with the host pitch at C4 (261.63 Hz); this ratio
; keeps that render exact while making the membrane modes playable.
(def pitch-ratio (* (/ pitch 261.63) (semi (mod tune))))

; Shared tension glide, integrated in closed form: G(t) = int (g(t)-1) dt.
(def glide-depth (clip (mod glide) 0 4))
(def glide-int
  (* glide-depth
     (+ (* (/ glide_a1 (- glide_r1)) (- 1.0 (exp (* glide_r1 t))))
        (* (/ glide_a2 (- glide_r2)) (- 1.0 (exp (* glide_r2 t)))))))

(def attack-seconds (* attack_time (clip (mod attack) 0.1 8)))
(def attack-env (- 1.0 (exp (/ (- t) attack-seconds))))
(def decay-scale (/ 1.0 (clip (mod decay) 0.1 4)))

; Low membrane bank: each mode has its own level, decay and glide depth.
(defmacro wrap (ph) (- ph (floor ph)))
(defmacro low-phase (freq gscale ph0)
  (+ (* freq pitch-ratio (+ t (* gscale glide-int))) ph0))
(defmacro low-mode (freq level rate gscale ph0)
  (* level (exp (* rate decay-scale t))
     (sin (* 2.0 pi (wrap (low-phase freq gscale ph0))))))
(def low-bank
  (+ (low-mode lf1 la1 ld1 lg1 lp1)
     (low-mode lf2 la2 ld2 lg2 lp2)
     (low-mode lf3 la3 ld3 lg3 lp3)
     (low-mode lf4 la4 ld4 lg4 lp4)
     (low-mode lf5 la5 ld5 lg5 lp5)))

; Ring bank: fixed-pitch shell / beater modes. knock scales the level, ring
; scales the decay time; both exact no-ops at 1.
(def ring-scale (/ 1.0 (clip (mod ring) 0.1 4)))
(def ring-pitch (pow pitch-ratio (clip (mod ring_track) 0 1)))
(defmacro ring-mode (freq level rate)
  (* level (exp (* rate ring-scale t))
     (sin (* 2.0 pi (wrap (* freq ring-pitch t))))))
(def ring-bank
  (* (clip (mod knock) 0 4)
     (+ (ring-mode mf1 ma1 md1)
        (ring-mode mf2 ma2 md2)
        (ring-mode mf3 ma3 md3)
        (ring-mode mf4 ma4 md4)
        (ring-mode mf5 ma5 md5)
        (ring-mode mf6 ma6 md6)
        (ring-mode mf7 ma7 md7)
        (ring-mode mf8 ma8 md8))))
(def bipolar-noise (- (* (noise) 2.0) 1.0))

; Rattle: noise gated by the negative half of the membrane signal, the buzz
; that rides every trough of the real hit.
(def rattle-noise (biquad bipolar-noise (bq-hz rattle_hp) 0.707 1.0 1))
(def rattle-voice
  (* rattle-noise (max (- 0.0 (+ low-bank ring-bank)) 0.0)
     (exp (* rattle_decay t)) rattle_amp (clip (mod rattle) 0 4)))
(def body (* (+ low-bank ring-bank rattle-voice) attack-env))

(def filtered-noise
  (biquad bipolar-noise (bq-hz noise_cutoff) 0.707 1.0 0))
(def noise-voice
  (* filtered-noise (exp (* noise_decay t)) noise_amp
     (clip (mod noise) 0 4)))

; Recording hiss: high-passed noise with its own slow decay.
(def hiss-hp (biquad bipolar-noise (bq-hz hiss_cutoff) 0.707 1.0 1))
(def hiss-voice
  (* hiss-hp (exp (* hiss_decay t)) hiss_amp (clip (mod hiss) 0 4)))

(def mixed (+ body noise-voice hiss-voice))
; Gain-normalised saturator: out_drive / drive set the shape only.
(def drive-amount (* out_drive (clip (mod drive) 0.25 4)))
(def shaped
  (* (/ (tanh (* mixed drive-amount)) drive-amount) out_gain))
(out (* shaped (clip velocity 0 1) (clip (mod level) 0 1.5)) 1 @name audio)
