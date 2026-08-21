(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

(param amp_attack @default 1 @min 1 @max 100 @unit ms)
(param amp_decay @default 850 @min 60 @max 4000 @unit ms @mod true @mod-mode additive)
(param amp_sustain @default 0 @min 0 @max 0.35)
(param amp_release @default 90 @min 5 @max 1500 @unit ms)

(param tune @default -12 @min -36 @max 24)
(param pitch_sweep @default 28 @min -24 @max 72 @mod true @mod-mode additive)
(param sweep_decay @default 55 @min 5 @max 500 @unit ms)

(param x_spread @default 0.44 @min 0 @max 1 @mod true @mod-mode additive)
(param y_spread @default 0.58 @min 0 @max 1 @mod true @mod-mode additive)
(param z_depth @default 0.42 @min 0 @max 1 @mod true @mod-mode additive)
(param membrane_damp @default 0.34 @min 0 @max 1)
(param warp @default 0.55 @min 0 @max 2)

(param impact_decay @default 18 @min 1 @max 180 @unit ms)
(param click_level @default 0.28 @min 0 @max 1)
(param noise_level @default 0.22 @min 0 @max 1)
(param sub_level @default 0.72 @min 0 @max 1)
(param body_level @default 0.82 @min 0 @max 1)
(param shell_level @default 0.34 @min 0 @max 1)
(param cavity_level @default 0.36 @min 0 @max 1)

(param cavity_size @default 0.55 @min 0 @max 1)
(param drive @default 1.85 @min 0.3 @max 6)
(param tone @default 0.62 @min 0 @max 1 @mod true @mod-mode additive)
(param gain @default 0.42 @min 0 @max 1 @mod true @mod-mode additive)

(def tune_mul (pow 2 (/ tune 12)))
(def base_freq (clip (* pitch tune_mul) 18 260))

(def sweep_env (adsr gate trigger 1 sweep_decay 0 5))
(def sweep_amt (clip (mod pitch_sweep) -24 72))
(def sweep_mul (pow 2 (/ (* sweep_amt sweep_env) 12)))
(def f0 (clip (* base_freq sweep_mul) 18 820))

(def xs (clip (mod x_spread) 0 1))
(def ys (clip (mod y_spread) 0 1))
(def zs (clip (mod z_depth) 0 1))
(def damp (clip membrane_damp 0 1))
(def decay_time (clip (mod amp_decay) 60 4000))

(def amp_env (adsr gate trigger amp_attack decay_time amp_sustain amp_release))
(def env_x (adsr gate trigger 1 (clip (* decay_time (+ 0.32 (* 0.50 (- 1 damp)))) 25 3000) 0 amp_release))
(def env_y (adsr gate trigger 1 (clip (* decay_time (+ 0.24 (* 0.40 (- 1 damp)))) 20 2600) 0 amp_release))
(def env_z (adsr gate trigger 1 (clip (* decay_time (+ 0.16 (* 0.32 (- 1 damp)))) 15 2200) 0 amp_release))
(def strike_env (adsr gate trigger 1 impact_decay 0 5))
(def shell_env (adsr gate trigger 1 (clip (* impact_decay (+ 2.0 (* 4.0 (- 1 damp)))) 8 700) 0 8))
(def cavity_env (adsr gate trigger 2 (clip (* decay_time (+ 0.45 (* 0.80 cavity_size))) 60 5000) 0 amp_release))

(def ratio_x (+ 1.31 (* xs 0.38)))
(def ratio_y (+ 1.66 (* ys 0.58)))
(def ratio_z (+ 2.08 (* zs 0.95)))

(def phase0 (phasor f0))
(def core (sin (* phase0 twopi)))

(def phase_x (phasor (* f0 ratio_x)))
(def osc_x (sin (+ (* phase_x twopi) (* warp strike_env core))))

(def phase_y (phasor (* f0 ratio_y)))
(def osc_y (sin (+ (* phase_y twopi) (* warp 0.65 env_x osc_x))))

(def phase_z (phasor (* f0 ratio_z)))
(def osc_z (sin (+ (* phase_z twopi) (* warp 0.45 env_y osc_y))))

(def mode0 (* core amp_env))
(def mode_x (* osc_x env_x (- 0.68 (* damp 0.22))))
(def mode_y (* osc_y env_y (- 0.48 (* damp 0.16))))
(def mode_z (* osc_z env_z (- 0.34 (* damp 0.12))))

(def membrane (+ mode0 mode_x mode_y mode_z))

(def shell_ratio_a (+ 4.70 (* xs 1.55)))
(def shell_ratio_b (+ 6.35 (* ys 2.10)))
(def shell_phase_a (phasor (* f0 shell_ratio_a)))
(def shell_phase_b (phasor (* f0 shell_ratio_b)))
(def shell_a (sin (+ (* shell_phase_a twopi) (* 0.35 warp osc_y))))
(def shell_b (sin (+ (* shell_phase_b twopi) (* 0.25 warp osc_z))))
(def shell_raw (* shell_env (+ (* 0.62 shell_a) (* 0.38 shell_b))))
(def shell_hp (svf shell_raw (+ 900 (* zs 2200)) 0.75 2))

(def impact_noise_raw (noise))
(def impact_noise_bp (svf impact_noise_raw (+ 750 (* zs 3600)) (+ 0.7 (* xs 1.5)) 1))
(def impact_noise (* impact_noise_bp strike_env))

(def click_phase (phasor (* f0 (+ 10.0 (* zs 12.0)))))
(def click_osc (sin (+ (* click_phase twopi) (* 0.20 warp impact_noise_bp))))
(def click (* click_osc strike_env))

(def cavity_freq (clip (* f0 (+ 1.15 (* cavity_size 2.20))) 45 1800))
(def cavity_q (+ 0.7 (* cavity_size 3.4)))
(def cavity_raw (svf membrane cavity_freq cavity_q 1))
(def cavity (* cavity_raw cavity_env))

(def modal_mix (+ (* sub_level mode0) (* body_level membrane) (* shell_level shell_hp) (* cavity_level cavity)))
(def impact_mix (+ (* click_level click) (* noise_level impact_noise)))
(def excited (+ modal_mix impact_mix))

(def driven (tanh (* drive excited)))
(def tonev (clip (mod tone) 0 1))
(def shaped (svf driven (+ 480 (* tonev 9800)) (+ 0.55 (* 0.85 (- 1 damp))) 0))

(def signal (* shaped velocity (clip (mod gain) 0 1)))
(out signal 1 @name audio)