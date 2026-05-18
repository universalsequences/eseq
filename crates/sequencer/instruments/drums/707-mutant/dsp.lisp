; 707 Mutant Drum Lab
; Selectable 707-family drum voices with deep synthetic mutation controls.
; Note pitch now tracks the host pitch input by default via keytrack.

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
(def ext1 (in 11 @name ext1 @modulator 7))
(def ext2 (in 12 @name ext2 @modulator 8))
(def ext3 (in 13 @name ext3 @modulator 9))
(def ext4 (in 14 @name ext4 @modulator 10))

(defmacro sq (freq)
  (- (* (lt (phasor freq) 0.5) 2) 1))

(defmacro oscshape (phase freq shape width)
  (selector shape
    (sin (* phase twopi))
    (polyblep_saw phase freq)
    (polyblep_pulse phase width freq)
    (tanh (* 3.0 (sin (* phase twopi))))))

(param voice @default 1 @min 1 @max 10)
(param body_wave @default 0 @min 0 @max 3)
(param filter_mode @default 0 @min 0 @max 5)

(param amp_attack @default 1 @min 0.2 @max 80 @unit ms)
(param decay @default 420 @min 10 @max 3500 @unit ms @mod true @mod-mode additive)
(param amp_release @default 20 @min 2 @max 1000 @unit ms)

(param tune @default 0 @min -36 @max 36 @mod true @mod-mode additive)
(param keytrack @default 1.0 @min 0 @max 1)
(param pitch_sweep @default 20 @min -72 @max 96 @mod true @mod-mode additive)
(param sweep_decay @default 42 @min 2 @max 600 @unit ms)
(param sweep_curve @default 1.0 @min 0.25 @max 4.0)
(param pitch_wobble @default 0.0 @min 0 @max 60)
(param wobble_rate @default 38 @min 0.5 @max 240 @unit Hz)

(param body_level @default 0.80 @min 0 @max 1.5 @mod true @mod-mode additive)
(param sub_level @default 0.45 @min 0 @max 1.5 @mod true @mod-mode additive)
(param noise_level @default 0.50 @min 0 @max 1.5 @mod true @mod-mode additive)
(param metal_level @default 0.35 @min 0 @max 1.5 @mod true @mod-mode additive)
(param click_level @default 0.45 @min 0 @max 1.5 @mod true @mod-mode additive)
(param snap @default 0.52 @min 0 @max 1 @mod true @mod-mode additive)

(param body_ratio @default 1.00 @min 0.25 @max 8.0 @mod true @mod-mode additive)
(param partial_spread @default 0.48 @min 0 @max 2.0 @mod true @mod-mode additive)
(param membrane_fm @default 0.25 @min 0 @max 8 @mod true @mod-mode additive)
(param cross_ring @default 0.15 @min 0 @max 1 @mod true @mod-mode additive)
(param pulse_width @default 0.50 @min 0.05 @max 0.95 @mod true @mod-mode additive)

(param noise_color @default 0.62 @min 0 @max 1 @mod true @mod-mode additive)
(param noise_res @default 1.0 @min 0.5 @max 5.0 @mod true @mod-mode additive)
(param metal_tune @default 1.00 @min 0.25 @max 4.0 @mod true @mod-mode additive)
(param metal_spread @default 0.55 @min 0 @max 2.0 @mod true @mod-mode additive)

(param tone @default 0.66 @min 0 @max 1 @mod true @mod-mode additive)
(param resonance @default 0.85 @min 0.5 @max 5.0 @mod true @mod-mode additive)
(param drive @default 1.55 @min 0.2 @max 12 @mod true @mod-mode additive)
(param fold @default 0.10 @min 0 @max 1 @mod true @mod-mode additive)
(param crush @default 0.08 @min 0 @max 1 @mod true @mod-mode additive)
(param gain @default 0.42 @min 0 @max 1 @mod true @mod-mode additive)

(def v (clip (floor voice) 1 10))
(def wave (clip (floor body_wave) 0 3))
(def fmode (clip (floor filter_mode) 0 5))

(def dec (clip (mod decay) 10 3500))
(def tune_mul (pow 2 (/ (clip (mod tune) -36 36) 12)))
(def key_mul (pow (clip (/ pitch 261.6256) 0.125 8.0) keytrack))
(def pmul (* tune_mul key_mul))
(def bratio (clip (mod body_ratio) 0.25 8.0))
(def spread (clip (mod partial_spread) 0 2.0))
(def pw (clip (mod pulse_width) 0.05 0.95))
(def snapv (clip (mod snap) 0 1))
(def bodyv (clip (mod body_level) 0 1.5))
(def subv (clip (mod sub_level) 0 1.5))
(def noisev (clip (mod noise_level) 0 1.5))
(def metalv (clip (mod metal_level) 0 1.5))
(def clickv (clip (mod click_level) 0 1.5))
(def nc (clip (mod noise_color) 0 1))
(def nq (clip (mod noise_res) 0.5 5.0))
(def mtune (clip (mod metal_tune) 0.25 4.0))
(def mspread (clip (mod metal_spread) 0 2.0))
(def xring (clip (mod cross_ring) 0 1))
(def fmamt (clip (mod membrane_fm) 0 8))

(def amp_env (adsr gate trigger amp_attack dec 0 amp_release))
(def short_env (adsr gate trigger 0.5 (clip (* dec 0.09) 5 240) 0 5))
(def snap_env (adsr gate trigger 0.2 (clip (* dec (+ 0.035 (* 0.22 snapv))) 4 420) 0 4))
(def sweep_env_raw (adsr gate trigger 0.5 sweep_decay 0 4))
(def sweep_env (pow (clip sweep_env_raw 0 1) sweep_curve))
(def sweep_mul (pow 2 (/ (* (clip (mod pitch_sweep) -72 96) sweep_env) 12)))
(def wobble (sin (* (phasor wobble_rate) twopi)))
(def wobble_mul (pow 2 (/ (* pitch_wobble wobble sweep_env) 12)))

(def n (noise))
(def n_bp (svf n (+ 420 (* nc 9200)) nq 1))
(def n_lp (svf n (+ 450 (* nc 11500)) (+ 0.6 (* nq 0.25)) 0))
(def n_hp (svf n (+ 900 (* nc 13500)) (+ 0.6 (* nq 0.20)) 2))
(def n_mix (+ (* (- 1 nc) n_lp) (* nc n_hp) (* 0.65 n_bp)))

(def metal_bank
  (+ (* 0.20 (sq (* 205 mtune (+ 1 (* 0.10 mspread)) pmul)))
     (* 0.18 (sq (* 306 mtune (+ 1 (* 0.28 mspread)) pmul)))
     (* 0.16 (sq (* 384 mtune (+ 1 (* 0.53 mspread)) pmul)))
     (* 0.15 (sq (* 522 mtune (+ 1 (* 0.77 mspread)) pmul)))
     (* 0.12 (sq (* 801 mtune (+ 1 (* 1.13 mspread)) pmul)))
     (* 0.10 (sq (* 1013 mtune (+ 1 (* 1.61 mspread)) pmul)))))

; KICK
(def kick_f (clip (* 52 pmul bratio sweep_mul wobble_mul) 20 900))
(def kick_phase (phasor kick_f))
(def kick_mod (sin (* (phasor (* kick_f (+ 1.85 (* spread 1.75)))) twopi)))
(def kick_osc (oscshape kick_phase kick_f wave pw))
(def kick_body (* amp_env (sin (+ (* kick_phase twopi) (* fmamt short_env kick_mod)))))
(def kick_sub (* amp_env (sin (* (phasor (clip (* kick_f 0.5) 12 220)) twopi))))
(def kick_click (* snap_env (+ (* n_hp 0.75) (* metal_bank 0.25))))
(def kick (+ (* bodyv 1.25 kick_body) (* subv 0.65 kick_sub) (* clickv snapv 0.55 kick_click) (* xring 0.45 kick_osc kick_mod amp_env)))

; SNARE
(def sn_env (adsr gate trigger 0.5 (clip (* dec 0.72) 22 1800) 0 amp_release))
(def sn_f1 (clip (* 182 pmul bratio sweep_mul) 55 2200))
(def sn_f2 (* sn_f1 (+ 1.55 (* spread 1.30))))
(def snp1 (oscshape (phasor sn_f1) sn_f1 wave pw))
(def snp2 (sin (+ (* (phasor sn_f2) twopi) (* fmamt 0.35 sn_env snp1))))
(def sn_body (* sn_env (+ (* 0.65 snp1) (* 0.35 snp2) (* xring 0.35 snp1 snp2))))
(def sn_noise (* n_mix (adsr gate trigger 0.5 (clip (* dec 0.58) 20 1300) 0 7)))
(def snare (+ (* bodyv 0.75 sn_body) (* noisev (+ 0.28 (* 0.95 snapv)) sn_noise) (* clickv 0.25 snapv n_hp snap_env)))

; LOW TOM
(def lot_env (adsr gate trigger 0.6 (clip (* dec 0.95) 45 2400) 0 amp_release))
(def lot_f (clip (* 112 pmul bratio (pow 2 (/ (* 0.45 (clip (mod pitch_sweep) -72 96) sweep_env) 12))) 34 1200))
(def lot_m (sin (* (phasor (* lot_f (+ 1.42 (* spread 0.82)))) twopi)))
(def lot_body (* lot_env (sin (+ (* (phasor lot_f) twopi) (* fmamt 0.55 lot_env lot_m)))))
(def lotom (+ (* bodyv 1.05 lot_body) (* subv 0.35 lot_env (sin (* (phasor (* lot_f 0.5)) twopi))) (* noisev 0.22 snapv n_bp snap_env) (* xring 0.32 lot_body lot_m)))

; HIGH TOM
(def hit_env (adsr gate trigger 0.6 (clip (* dec 0.70) 30 1700) 0 amp_release))
(def hit_f (clip (* 176 pmul bratio (pow 2 (/ (* 0.38 (clip (mod pitch_sweep) -72 96) sweep_env) 12))) 55 1800))
(def hit_m (sin (* (phasor (* hit_f (+ 1.55 (* spread 1.05)))) twopi)))
(def hit_body (* hit_env (oscshape (phasor hit_f) hit_f wave pw)))
(def hitom (+ (* bodyv hit_env (sin (+ (* (phasor hit_f) twopi) (* fmamt 0.45 hit_env hit_m)))) (* noisev 0.26 snapv n_bp snap_env) (* xring 0.38 hit_body hit_m)))

; RIM
(def rim_env (adsr gate trigger 0.2 (clip (* dec 0.13) 6 300) 0 4))
(def rim_f (* 520 pmul bratio))
(def rim_a (oscshape (phasor rim_f) rim_f wave pw))
(def rim_b (sin (+ (* (phasor (* rim_f (+ 1.9 (* spread 0.9)))) twopi) (* fmamt 0.20 rim_env rim_a))))
(def rim_c (sin (* (phasor (* rim_f (+ 3.25 (* spread 1.8)))) twopi)))
(def rim (+ (* bodyv rim_env (+ (* 0.56 rim_a) (* 0.31 rim_b) (* 0.18 rim_c))) (* noisev snapv 0.25 n_hp rim_env) (* xring 0.40 rim_a rim_b rim_env)))

; CLAP
(def clap_a (adsr gate trigger 0.2 (clip (* dec 0.08) 8 170) 0 4))
(def clap_b (adsr gate trigger 7 (clip (* dec 0.48) 35 1000) 0 15))
(def clap_c (adsr gate trigger 20 (clip (* dec 0.30) 25 700) 0 10))
(def clap_pulse (+ clap_a (* 0.62 clap_b) (* 0.38 clap_c)))
(def clap (+ (* noisev 1.10 n_mix clap_pulse) (* metalv 0.22 metal_bank clap_a) (* clickv 0.20 n_hp snap_env)))

; TAMBOURINE
(def tamb_env (adsr gate trigger 0.2 (clip (* dec 0.35) 18 1100) 0 7))
(def tamb (+ (* metalv 1.10 metal_bank tamb_env) (* noisev 0.70 n_hp tamb_env) (* xring 0.25 metal_bank n_bp tamb_env)))

; CLOSED HAT
(def ch_env (adsr gate trigger 0.1 (clip (* dec 0.15) 8 340) 0 4))
(def chat (+ (* metalv metal_bank ch_env) (* noisev snapv n_hp ch_env) (* xring 0.18 n_bp metal_bank ch_env)))

; OPEN HAT
(def oh_env (adsr gate trigger 0.3 (clip (* dec 1.05) 55 3000) 0 30))
(def ohat (+ (* metalv metal_bank oh_env) (* noisev 0.60 n_hp oh_env) (* xring 0.16 n_bp metal_bank oh_env)))

; CYMBAL
(def cym_env (adsr gate trigger 0.8 (clip (* dec 1.75) 160 3500) 0 70))
(def cym (+ (* metalv 1.05 metal_bank cym_env) (* noisev 0.38 n_hp cym_env) (* bodyv 0.20 n_bp cym_env) (* xring 0.24 metal_bank n_mix cym_env)))

(def selected (selector v kick snare lotom hitom rim clap tamb chat ohat cym))
(def pre_drive (+ selected (* (clip (mod fold) 0 1) (sin (* selected (+ 4 (* 20 (clip (mod fold) 0 1))))))))
(def driven (tanh (* (clip (mod drive) 0.2 12) pre_drive)))
(def steps (+ 6 (* (- 1 (clip (mod crush) 0 1)) 250)))
(def crushed (/ (floor (* driven steps)) steps))
(def tonev (clip (mod tone) 0 1))
(def filt_cut (+ 220 (* tonev 14500)))
(def filtered (svf crushed filt_cut (clip (mod resonance) 0.5 5.0) fmode))
(def signal (* filtered velocity (clip (mod gain) 0 1)))
(out signal 1 @name audio)