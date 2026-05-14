; Swoosh 909-style open hi-hat synth
; Cymbal-square metal bank, noisy air, and a decaying resonant filter sweep for the classic 909 wash.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))
(def mod5 (in 9 @name mod5 @modulator 5))
(def mod6 (in 10 @name mod6 @modulator 6))

(defmacro sq (freq)
  (scale (lt (phasor freq) 0.5) 0 1 -1 1))

(param attack_ms @default 0.4 @min 0.1 @max 30 @unit ms)
(param decay_ms @default 1180 @min 120 @max 5000 @unit ms @mod true @mod-mode additive)
(param release_ms @default 240 @min 10 @max 1600 @unit ms)

(param tune @default 0 @min -18 @max 18 @unit st @mod true @mod-mode additive)
(param metal_mix @default 0.72 @min 0 @max 1 @mod true @mod-mode additive)
(param noise_mix @default 0.42 @min 0 @max 1 @mod true @mod-mode additive)
(param body_level @default 0.34 @min 0 @max 1)

(param cutoff @default 7600 @min 1800 @max 15000 @unit Hz @mod true @mod-mode additive)
(param resonance @default 2.1 @min 0.5 @max 5 @mod true @mod-mode additive)
(param swoosh @default 0.82 @min 0 @max 1 @mod true @mod-mode additive)
(param air @default 0.72 @min 0 @max 1 @mod true @mod-mode additive)
(param drive @default 1.25 @min 0.5 @max 7 @mod true @mod-mode additive)
(param gain @default 0.36 @min 0 @max 1 @mod true @mod-mode additive)

(def amp_env (adsr gate trigger attack_ms (clip (mod decay_ms) 120 5000) 0 release_ms))
(def sweep_env (adsr gate trigger 0.1 360 0 70))
(def click_env (adsr gate trigger 0.1 13 0 3))
(def bloom_env (adsr gate trigger 2 520 0 120))

; 909 cymbal oscillator cluster: intentionally non-harmonic square tones.
(def tune_mul (pow 2 (/ (clip (mod tune) -18 18) 12)))
(def f1 (* 204 tune_mul))
(def f2 (* 317 tune_mul))
(def f3 (* 379 tune_mul))
(def f4 (* 510 tune_mul))
(def f5 (* 540 tune_mul))
(def f6 (* 800 tune_mul))

(def s1 (sq f1))
(def s2 (sq f2))
(def s3 (sq f3))
(def s4 (sq f4))
(def s5 (sq f5))
(def s6 (sq f6))

; Layered sums create the 909 beating/phasey metallic cloud rather than a pure white-noise hat.
(def metal_a (* 0.17 (+ s1 s2 s3 s4 s5 s6)))
(def metal_b (* 0.16 (+ (* s1 s4) (* s2 s5) (* s3 s6))))
(def metal_raw (tanh (* 1.7 (+ metal_a metal_b))))

(def swo (clip (mod swoosh) 0 1))
(def res (clip (mod resonance) 0.5 5))
(def base_cut (clip (mod cutoff) 1800 15000))

; The swoosh: a per-hit downward high-band sweep plus a slower blooming resonant wash.
(def sweep_cut (clip (+ base_cut (* swoosh 0) (* swo 5200 sweep_env) (* -1700 swo bloom_env)) 1800 15500))
(def low_sweep_cut (clip (+ 3900 (* swo 3400 sweep_env) (* -900 swo bloom_env)) 1600 11000))
(def high_sweep_cut (clip (+ 8500 (* swo 4800 sweep_env) (* -1400 swo bloom_env)) 3200 16000))

(def metal_hp (svf metal_raw 3300 0.72 2))
(def metal_low (svf metal_hp low_sweep_cut (+ 1.1 (* res 0.55)) 1))
(def metal_high (svf metal_hp high_sweep_cut (+ 1.0 (* res 0.42)) 1))
(def metal_peak (svf metal_hp sweep_cut (+ 0.8 (* res 0.35)) 4))

(def white (noise))
(def noise_hp (svf white (+ 5200 (* (clip (mod air) 0 1) 5200)) 0.72 2))
(def noise_band (svf noise_hp high_sweep_cut (+ 0.9 (* res 0.32)) 1))
(def noise_air (svf noise_hp 11800 0.65 2))

(def click (svf white 12500 0.7 2))
(def swish_tail (+ (* 0.92 metal_low)
                   (* 0.78 metal_high)
                   (* 0.55 metal_peak)
                   (* (clip (mod noise_mix) 0 1) 0.52 noise_band)
                   (* (clip (mod air) 0 1) 0.26 noise_air)))

(def source (+ (* (clip (mod metal_mix) 0 1) swish_tail)
               (* body_level 0.38 metal_hp)
               (* click_env 0.16 click)))

(def driven (tanh (* (clip (mod drive) 0.5 7) source)))
(def final_hp (svf driven 4700 0.72 2))
(def final_air (svf final_hp 9900 0.75 4))
(def signal (* final_air amp_env velocity (clip (mod gain) 0 1)))
(out signal 1 @name audio)
