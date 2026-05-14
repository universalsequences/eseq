; TR-707 inspired PCM-style drum synth
; One triggerable mono drum voice with selectable 707 family models.

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
  (- (* (lt (phasor freq) 0.5) 2) 1))

(param voice @default 1 @min 1 @max 10)

(param amp_attack @default 1 @min 0.2 @max 80 @unit ms)
(param decay @default 360 @min 12 @max 2500 @unit ms @mod true @mod-mode additive)
(param amp_release @default 18 @min 2 @max 600 @unit ms)

(param tune @default 0 @min -24 @max 24 @mod true @mod-mode additive)
(param keytrack @default 0.0 @min 0 @max 1)
(param pitch_sweep @default 18 @min -36 @max 60 @mod true @mod-mode additive)
(param sweep_decay @default 38 @min 4 @max 280 @unit ms)

(param body_level @default 0.82 @min 0 @max 1 @mod true @mod-mode additive)
(param noise_level @default 0.55 @min 0 @max 1 @mod true @mod-mode additive)
(param metal_level @default 0.38 @min 0 @max 1 @mod true @mod-mode additive)
(param snap @default 0.48 @min 0 @max 1 @mod true @mod-mode additive)
(param tone @default 0.62 @min 0 @max 1 @mod true @mod-mode additive)
(param grit @default 0.16 @min 0 @max 1 @mod true @mod-mode additive)
(param drive @default 1.35 @min 0.4 @max 8 @mod true @mod-mode additive)
(param gain @default 0.42 @min 0 @max 1 @mod true @mod-mode additive)

(def v (clip (floor voice) 1 10))
(def dec (clip (mod decay) 12 2500))
(def tune_mul (pow 2 (/ (clip (mod tune) -24 24) 12)))
(def key_mul (pow (clip (/ pitch 261.6256) 0.25 4.0) keytrack))
(def pmul (* tune_mul key_mul))
(def tonev (clip (mod tone) 0 1))
(def snapv (clip (mod snap) 0 1))
(def bodyv (clip (mod body_level) 0 1))
(def noisev (clip (mod noise_level) 0 1))
(def metalv (clip (mod metal_level) 0 1))
(def gritv (clip (mod grit) 0 1))

(def amp_env (adsr gate trigger amp_attack dec 0 amp_release))
(def short_env (adsr gate trigger 1 (clip (* dec 0.10) 8 180) 0 8))
(def snap_env (adsr gate trigger 0.5 (clip (* dec (+ 0.06 (* 0.16 snapv))) 8 320) 0 5))
(def sweep_env (adsr gate trigger 1 sweep_decay 0 5))
(def sweep_mul (pow 2 (/ (* (clip (mod pitch_sweep) -36 60) sweep_env) 12)))

(def n (noise))
(def n_bright (svf n (+ 2500 (* tonev 11000)) (+ 0.7 (* snapv 2.4)) 1))
(def n_dark (svf n (+ 700 (* tonev 5200)) (+ 0.65 (* snapv 1.6)) 0))
(def n_air (svf n (+ 4200 (* tonev 12500)) 0.8 2))

; Kick: short 707-ish sampled thump with click and pitch drop.
(def kick_f (clip (* 52 pmul sweep_mul) 28 260))
(def kick_phase (phasor kick_f))
(def kick_body (* (sin (* kick_phase twopi)) amp_env))
(def kick_click (* n_air snap_env))
(def kick_raw (+ (* bodyv 1.30 kick_body) (* noisev snapv 0.45 kick_click)))
(def kick (svf kick_raw (+ 420 (* tonev 5200)) 0.75 0))

; Snare: two pitched partials plus filtered noise.
(def sn_env (adsr gate trigger 1 (clip (* dec 0.72) 35 1400) 0 amp_release))
(def snp1 (sin (* (phasor (* 184 pmul)) twopi)))
(def snp2 (sin (* (phasor (* 327 pmul)) twopi)))
(def sn_body (* sn_env (+ (* 0.68 snp1) (* 0.32 snp2))))
(def sn_noise (* n_bright (adsr gate trigger 1 (clip (* dec 0.55) 28 1100) 0 8)))
(def snare (+ (* bodyv 0.82 sn_body) (* noisev (+ 0.35 (* 0.85 snapv)) sn_noise)))

; Toms: tuned sine bodies with a sampled attack tick.
(def lotom_env (adsr gate trigger 1 (clip (* dec 0.92) 60 1800) 0 amp_release))
(def lotom_f (clip (* 112 pmul (pow 2 (/ (* 0.42 (clip (mod pitch_sweep) -36 60) sweep_env) 12))) 55 420))
(def lotom_tone (* (sin (* (phasor lotom_f) twopi)) lotom_env))
(def lotom (+ (* bodyv 1.10 lotom_tone) (* noisev 0.24 snapv n_dark snap_env)))

(def hitom_env (adsr gate trigger 1 (clip (* dec 0.70) 45 1300) 0 amp_release))
(def hitom_f (clip (* 176 pmul (pow 2 (/ (* 0.35 (clip (mod pitch_sweep) -36 60) sweep_env) 12))) 80 620))
(def hitom_tone (* (sin (* (phasor hitom_f) twopi)) hitom_env))
(def hitom (+ (* bodyv 1.00 hitom_tone) (* noisev 0.30 snapv n_dark snap_env)))

; Rimshot: short woody partial stack.
(def rim_env (adsr gate trigger 0.5 (clip (* dec 0.12) 10 240) 0 5))
(def rim_a (sin (* (phasor (* 515 pmul)) twopi)))
(def rim_b (sin (* (phasor (* 1040 pmul)) twopi)))
(def rim_c (sin (* (phasor (* 1720 pmul)) twopi)))
(def rim_raw (* rim_env (+ (* 0.56 rim_a) (* 0.31 rim_b) (* 0.18 rim_c) (* 0.28 snapv n_bright))))
(def rim (svf rim_raw (+ 950 (* tonev 5200)) 1.5 1))

; Clap: noisy handclap smear with quick attack layer.
(def clap_env_a (adsr gate trigger 0.5 (clip (* dec 0.09) 12 170) 0 5))
(def clap_env_b (adsr gate trigger 6 (clip (* dec 0.48) 45 900) 0 15))
(def clap_src (* n_bright (+ (* 0.95 clap_env_a) (* 0.62 clap_env_b))))
(def clap (svf clap_src (+ 1100 (* tonev 4700)) (+ 0.7 (* snapv 1.2)) 1))

; Tambourine: metallic jingle plus white-noise skin.
(def tamb_env (adsr gate trigger 0.5 (clip (* dec 0.36) 30 900) 0 8))
(def tamb_metal (+ (* 0.34 (sq (* 360 pmul))) (* 0.28 (sq (* 522 pmul))) (* 0.22 (sq (* 735 pmul))) (* 0.16 (sq (* 980 pmul)))))
(def tamb_raw (* tamb_env (+ (* metalv tamb_metal) (* noisev 0.75 n_bright))))
(def tamb (svf tamb_raw (+ 4200 (* tonev 11000)) 1.1 2))

; Closed and open hats: 707/727-style PCM metallic hash.
(def hat_core (+ (* 0.23 (sq (* 245 pmul))) (* 0.21 (sq (* 306 pmul))) (* 0.19 (sq (* 384 pmul))) (* 0.17 (sq (* 522 pmul))) (* 0.12 (sq (* 800 pmul))) (* 0.08 (sq (* 1010 pmul)))))
(def ch_env (adsr gate trigger 0.4 (clip (* dec 0.16) 12 260) 0 5))
(def oh_env (adsr gate trigger 0.8 (clip (* dec 1.05) 80 2200) 0 25))
(def ch_raw (* ch_env (+ (* metalv hat_core) (* noisev snapv n_air))))
(def oh_raw (* oh_env (+ (* metalv hat_core) (* noisev 0.62 n_air))))
(def chat (svf ch_raw (+ 5200 (* tonev 11500)) (+ 0.8 (* snapv 1.1)) 2))
(def ohat (svf oh_raw (+ 4300 (* tonev 12500)) 0.9 2))

; Crash/ride cymbal: longer metallic oscillator bank with airy noise.
(def cym_env (adsr gate trigger 1 (clip (* dec 1.55) 180 2500) 0 60))
(def cym_core (+ (* 0.20 (sq (* 205 pmul))) (* 0.18 (sq (* 331 pmul))) (* 0.17 (sq (* 448 pmul))) (* 0.15 (sq (* 612 pmul))) (* 0.12 (sq (* 863 pmul))) (* 0.10 (sq (* 1190 pmul))) (* 0.08 n_air)))
(def cym_raw (* cym_env (+ (* metalv cym_core) (* noisev 0.35 n_air))))
(def cym (svf cym_raw (+ 3600 (* tonev 13000)) 0.8 2))

(def selected (selector v kick snare lotom hitom rim clap tamb chat ohat cym))
(def driven (tanh (* (clip (mod drive) 0.4 8) selected)))
(def steps (+ 10 (* (- 1 gritv) 118)))
(def crushed (/ (floor (* driven steps)) steps))
(def polished (svf crushed (+ 3500 (* tonev 12500)) 0.72 0))
(def signal (* polished velocity (clip (mod gain) 0 1)))
(out signal 1 @name audio)
