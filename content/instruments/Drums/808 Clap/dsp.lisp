; Factory ID808 Clap — the identified Roland R-8 MkII '808Clap' sample,
; recovered by SynthID-style optimisation (dgen Examples/SynthID/scripts/
; fit_clap.py) rather than sampled: at the defaults below every hit reproduces
; the learned render; every knob is a departure from the identified sound.
;
; Voice: one noise stream -> two bandpasses (hand cup + slap) and a highpass
; -> four bursts on a measured flam (0 / 9.0 / 11.8 / 8.1 ms) each with a
; 1.7 ms sub-burst -> a two-stage tail (fast bright, slow dark) from the last
; burst -> tanh -> the R-8's 13 kHz output stage (fixed 12 kHz lowpass) and a
; fitted output highpass.

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
(param flam @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param snap @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param body @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param decay @default 1 @min 0.1 @max 6 @mod true @mod-mode additive)
(param bright @default 1 @min 0 @max 6 @mod true @mod-mode additive)
(param drive @default 1 @min 0.2 @max 6 @mod true @mod-mode additive)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive)

; ---- the identified scalars (recovered_params.json), editable ----
(param fc1 @default 954.987 @min 300 @max 3000 @unit Hz)
(param q1 @default 1.82718 @min 0.5 @max 6)
(param fc2 @default 1822.78 @min 800 @max 6000 @unit Hz)
(param q2 @default 0.617773 @min 0.5 @max 6)
(param g2 @default 2.08088 @min 0 @max 4)
(param sp1 @default 8.96755 @min 4 @max 16 @unit ms)
(param sp2 @default 11.8699 @min 4 @max 16 @unit ms)
(param sp3 @default 8.48419 @min 4 @max 16 @unit ms)
(param bdecay @default -368.838 @min -1500 @max -100)
(param l2 @default 1.66352 @min 0 @max 3)
(param l3 @default 1.02267 @min 0 @max 1.5)
(param l4 @default 0 @min 0 @max 1.5)
(param sub_delay @default 1.521 @min 1.5 @max 6 @unit ms)
(param sub_gain @default 0.863983 @min 0 @max 2)
(param burst_amp @default 0.480209 @min 0.05 @max 20)
(param tail_a1 @default 0.569816 @min 0 @max 10)
(param tail_d1 @default -46.9733 @min -150 @max -10)
(param tail_a2 @default 0.478803 @min 0 @max 10)
(param tail_d2 @default -14.9908 @min -40 @max -4)
(param tail_lpf @default 12000 @min 500 @max 12000 @unit Hz)
(param hp_fc @default 500 @min 500 @max 10000 @unit Hz)
(param b_hp @default 0.00830026 @min 0 @max 1.5)
(param t_hp @default 0 @min 0 @max 1.5)
(param out_hp @default 655.089 @min 20 @max 2000 @unit Hz)
(param out_drive @default 2.31957 @min 0.5 @max 4)
(param out_gain @default 0.509693 @min 0.05 @max 2)

(defmacro semi (s) (pow 2 (/ s 12)))
; DGen's biquad hardcodes a 44.1 kHz frame in its coefficient math; scale the
; requested Hz so the filter lands on the physical frequency at any host rate.
(defmacro bq-hz (hz) (* hz (/ 44100.0 samplerate)))
; exponential segment starting at t0 seconds, zero before it
(defmacro seg (t t0 rate) (gswitch (lt t t0) 0.0 (exp (* rate (- t t0)))))

(def tn (semi (mod tune)))
(def vel (clip velocity 0 1))
; accum holds 0 for the trigger sample and the one after it, so it lags the
; fit's n/samplerate ramp by one sample; add the sample back (only sample 0
; differs, where every envelope is ~1 either way).
(def t (+ (accum (/ 1.0 samplerate) trigger 0 1000000) (/ 1.0 samplerate)))

; sources: one noise stream, filtered three ways
(def n (- (* (noise) 2.0) 1.0))
(def bp1 (biquad n (bq-hz (* fc1 tn)) q1 1.0 2))
(def bp2 (biquad n (bq-hz (* fc2 tn)) q2 1.0 2))
(def hpn (biquad n (bq-hz (* hp_fc tn)) 0.707 1.0 1))
(def src (+ bp1 (* g2 bp2)))
(def lp_src (biquad bp1 (bq-hz tail_lpf) 0.707 1.0 0))

; burst train: measured flam, each burst plus a short sub-burst
(def fl (clip (mod flam) 0.25 4))
(def t2 (* sp1 fl 0.001))
(def t3 (* (+ sp1 sp2) fl 0.001))
(def t4 (* (+ sp1 sp2 sp3) fl 0.001))
(def sd (* sub_delay 0.001))
(def env_b (+ (seg t 0.0 bdecay) (* sub_gain (seg t sd bdecay))
              (* l2 (+ (seg t t2 bdecay) (* sub_gain (seg t (+ t2 sd) bdecay))))
              (* l3 (+ (seg t t3 bdecay) (* sub_gain (seg t (+ t3 sd) bdecay))))
              (* l4 (+ (seg t t4 bdecay) (* sub_gain (seg t (+ t4 sd) bdecay))))))

; tail from the last burst: fast bright stage + slow dark stage
(def dk (/ 1.0 (clip (mod decay) 0.1 6)))
(def tail_fast (* tail_a1 (seg t t4 (* tail_d1 dk))))
(def tail_slow (* tail_a2 (seg t t4 (* tail_d2 dk))))

(def br (clip (mod bright) 0 6))
(def x (+ (* (+ src (* b_hp br hpn)) env_b burst_amp (clip (mod snap) 0 4))
          (* (+ src (* t_hp br hpn)) tail_fast (clip (mod body) 0 4))
          (* lp_src tail_slow (clip (mod body) 0 4))))
(def shaped (* (tanh (* x out_drive (clip (mod drive) 0.2 6))) out_gain))
; the R-8's output stage: nothing above its 13 kHz Nyquist leaves the machine
(def r8 (biquad (biquad shaped (bq-hz 12000) 0.707 1.0 0) (bq-hz 12000) 0.707 1.0 0))
(def voice (biquad r8 (bq-hz out_hp) 0.707 1.0 1))
(out (* voice vel (clip (mod level) 0 1.5)) 1 @name audio)
