(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))

(param tune @default 1.0 @min 0.25 @max 2.0)
(param body_level @default 0.95 @min 0 @max 2)
(param gain @default 0.75 @min 0 @max 1)

(param amp_attack @default 1 @min 1 @max 50 @unit ms)
(param amp_decay @default 520 @min 20 @max 2500 @unit ms)
(param amp_sustain @default 0 @min 0 @max 1)
(param amp_release @default 80 @min 5 @max 1500 @unit ms)

(param sweep_amount @default 105 @min 0 @max 360 @unit Hz)
(param sweep_decay @default 48 @min 5 @max 300 @unit ms)
(param sweep_curve @default 1.7 @min 0.3 @max 4.0)

(param punch @default 0.35 @min 0 @max 2)
(param punch_decay @default 32 @min 5 @max 180 @unit ms)

(param mode2_level @default 0.28 @min 0 @max 1.5)
(param mode2_ratio @default 1.58 @min 1.1 @max 2.8)
(param mode2_decay @default 155 @min 10 @max 1200 @unit ms)

(param mode3_level @default 0.15 @min 0 @max 1.5)
(param mode3_ratio @default 2.21 @min 1.5 @max 4.5)
(param mode3_decay @default 92 @min 10 @max 900 @unit ms)

(param click_level @default 0.22 @min 0 @max 1.5)
(param click_decay @default 9 @min 1 @max 80 @unit ms)
(param click_tone @default 6500 @min 800 @max 14000 @unit Hz)

(param damping @default 1400 @min 80 @max 7000 @unit Hz)
(param drive @default 1.7 @min 0.5 @max 8.0)

(def amp_env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))
(def sweep_env_raw (adsr gate trigger 1 sweep_decay 0 5))
(def sweep_env (pow (clip sweep_env_raw 0 1) sweep_curve))

(def punch_env (adsr gate trigger 1 punch_decay 0 5))
(def mode2_env (adsr gate trigger 1 mode2_decay 0 20))
(def mode3_env (adsr gate trigger 1 mode3_decay 0 20))
(def click_env (adsr gate trigger 1 click_decay 0 5))

(def base_freq (clip (* pitch tune) 24 220))
(def body_freq (clip (+ base_freq (* sweep_amount sweep_env)) 24 380))

(def body_phase (phasor body_freq))
(def mode2_phase (phasor (* body_freq mode2_ratio)))
(def mode3_phase (phasor (* body_freq mode3_ratio)))
(def punch_phase (phasor (* body_freq 0.5)))

(def body_mode (* (sin (* body_phase twopi)) amp_env body_level))
(def punch_mode (* (sin (* punch_phase twopi)) punch_env punch))
(def mode2 (* (sin (* mode2_phase twopi)) mode2_env mode2_level))
(def mode3 (* (sin (* mode3_phase twopi)) mode3_env mode3_level))

(def modal_raw (+ body_mode punch_mode mode2 mode3))
(def modal_damped (svf modal_raw (clip damping 80 7000) 0.65 0))

(def click_noise (svf (noise) (clip click_tone 800 14000) 0.75 2))
(def click (* click_noise click_env click_level))

(def struck (+ modal_damped click))
(def saturated (tanh (* struck drive)))
(out (* saturated velocity gain) 1 @name audio)