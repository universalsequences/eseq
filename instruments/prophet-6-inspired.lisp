; Prophet-6 inspired analog polysynth
; Retuned gain staging so clean tones are reachable at low filter drive,
; while higher drive still pushes into deliberate analog-style saturation.

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

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro sat (x amt)
  (tanh (* x amt)))

(param amp_attack_ms      @default 4     @min 1     @max 4000 @unit ms)
(param amp_decay_ms       @default 150   @min 1     @max 4000 @unit ms)
(param amp_sustain        @default 0.82  @min 0     @max 1)
(param amp_release_ms     @default 320   @min 1     @max 6000 @unit ms)

(param filt_attack_ms     @default 2     @min 1     @max 4000 @unit ms)
(param filt_decay_ms      @default 220   @min 1     @max 4000 @unit ms)
(param filt_sustain       @default 0.10  @min 0     @max 1)
(param filt_release_ms    @default 340   @min 1     @max 6000 @unit ms)

(param osc1_shape         @default 0.92  @min 0     @max 1)
(param osc2_shape         @default 0.24  @min 0     @max 1)
(param osc1_semitones     @default 0     @min -24   @max 24 @unit st @mod true @mod-mode additive)
(param osc2_semitones     @default 0     @min -24   @max 24 @unit st @mod true @mod-mode additive)
(param pulse_width        @default 0.50  @min 0.10  @max 0.90 @mod true @mod-mode additive)
(param osc_mix            @default 0.43  @min 0     @max 1)
(param osc_slop           @default 0.22  @min 0     @max 0.6)
(param osc_detune_cents   @default 8     @min -25   @max 25 @unit ct @mod true @mod-mode additive)
(param shape_drift        @default 0.16  @min 0     @max 0.5)
(param sub_level          @default 0.12  @min 0     @max 0.7)
(param noise_level        @default 0.010 @min 0     @max 0.25)
(param brass              @default 0.38  @min 0     @max 1)

(param cutoff             @default 820   @min 30    @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance          @default 1.75  @min 0.5   @max 3.9 @mod true @mod-mode additive)
(param filter_env_amt     @default 4200  @min -8000 @max 8000 @unit Hz @mod true @mod-mode additive)
(param keytrack           @default 0.46  @min 0     @max 1)
(param vel_to_filter      @default 0.34  @min 0     @max 1)
(param filter_drive       @default 1.35  @min 0.2   @max 5 @mod true @mod-mode additive)
(param filter_tone        @default 0.68  @min 0     @max 1)
(param cutoff_skew        @default 0.16  @min 0     @max 1)

(param lfo_rate_hz        @default 3.6   @min 0.05  @max 20 @unit Hz)
(param lfo_to_pw          @default 0.00  @min 0     @max 0.35 @mod true @mod-mode additive)
(param lfo_to_cutoff      @default 0.00  @min 0     @max 1800 @unit Hz @mod true @mod-mode additive)
(param env_to_pitch       @default 0.00  @min -12   @max 12 @unit st @mod true @mod-mode additive)
(param vibrato            @default 0.00  @min 0     @max 0.08 @mod true @mod-mode additive)

(param stereo_spread      @default 0.08  @min 0     @max 1)
(param gain               @default 0.18  @min 0     @max 1)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

(def lfo_phase (phasor lfo_rate_hz))
(def lfo_tri (triangle lfo_phase))
(def drift_a (triangle (phasor 0.13)))
(def drift_b (triangle (phasor 0.19)))
(def drift_c (triangle (phasor 0.07)))

(def pitch_env_ratio (exp (/ (* (log 2) (* filt_env (mod env_to_pitch))) 12)))
(def vib_ratio (+ 1 (* lfo_tri (mod vibrato))))
(def keytrack_hz (* pitch keytrack))
(def vel_open (* velocity vel_to_filter 3000))

(def slop1 (+ 1.0 (* osc_slop -0.0085)))
(def slop2 (+ 1.0 (* osc_slop 0.0105)))
(def detune_ratio (exp (/ (* (log 2) (/ (clip (mod osc_detune_cents) -25 25) 100.0)) 12)))
(def osc1_ratio (semi_ratio (clip (mod osc1_semitones) -24 24)))
(def osc2_ratio (semi_ratio (clip (mod osc2_semitones) -24 24)))

(def freq1 (* pitch pitch_env_ratio vib_ratio slop1 osc1_ratio))
(def freq2 (* pitch pitch_env_ratio vib_ratio slop2 detune_ratio osc2_ratio))
(def sub_freq (* freq1 0.5))

(def ph1 (phasor freq1))
(def ph2 (phasor freq2))
(def phsub (phasor sub_freq))

(def pwm (clip (+ (mod pulse_width) (* lfo_tri (mod lfo_to_pw))) 0.10 0.90))
(def osc1_shape_live (clip (+ osc1_shape (* drift_a shape_drift 0.42)) 0 1))
(def osc2_shape_live (clip (+ osc2_shape (* drift_b shape_drift -0.34)) 0 1))
(def mix_bias (+ osc_mix (* drift_c osc_slop 0.08)))

(def osc1_saw (scale ph1 0 1 -1 1))
(def osc1_pulse (pulse_from_phase ph1 pwm))
(def osc2_saw (scale ph2 0 1 -1 1))
(def osc2_pulse (pulse_from_phase ph2 pwm))
(def sub_pulse (pulse_from_phase phsub 0.5))

(def osc1 (+ (* osc1_saw osc1_shape_live) (* osc1_pulse (- 1 osc1_shape_live))))
(def osc2 (+ (* osc2_saw osc2_shape_live) (* osc2_pulse (- 1 osc2_shape_live))))
(def osc1_bright (sat (+ (* osc1 1.04) (* osc1_saw brass 0.22)) 1.02))
(def osc2_hollow (sat (+ (* osc2 0.94) (* osc2_pulse brass 0.14)) 0.94))

(def asym_mix (+ (* osc1_bright 0.52 (- 1 mix_bias))
                 (* osc2_hollow 0.68 mix_bias)
                 (* sub_pulse (* sub_level 0.72))
                 (* (noise) noise_level)))
(def pre_emph (+ (* asym_mix 0.72)
                 (* (sat asym_mix 1.18) 0.08)
                 (* (biquad asym_mix (clip (+ 1600 (* brass 2600)) 400 9000) 0.7 1 1) brass 0.04)))

(def filter_target (+ (mod cutoff)
                      keytrack_hz
                      (* filt_env (mod filter_env_amt))
                      vel_open
                      (* lfo_tri (mod lfo_to_cutoff))))
(def fcut (clip filter_target 30 14000))
(def q (clip (mod resonance) 0.5 3.9))
(def drive_amt (clip (mod filter_drive) 0.2 5.0))
(def skew cutoff_skew)
(def drive_excess (clip (- drive_amt 1.0) 0 4.0))

(def predriven (sat pre_emph (+ 1.0 (* drive_excess 0.55))))
(def lp1_cut (clip (* fcut (+ 0.78 (* filter_tone 0.16) (* skew -0.10))) 30 14000))
(def lp1 (biquad (* predriven 0.82) lp1_cut (+ 0.08 (* q 0.82)) 1 0))
(def lp2_in (+ (* lp1 0.88) (* predriven 0.08)))
(def lp2_cut (clip (* fcut (+ 0.92 (* filter_tone 0.12) (* skew 0.04))) 30 14500))
(def lp2 (biquad lp2_in lp2_cut (+ 0.18 (* q 0.76)) 1 0))
(def lp3_in (sat (+ (* lp2 0.90) (* lp1 0.08)) (+ 1.0 (* drive_excess 0.22))))
(def lp3_cut (clip (+ (* fcut (+ 1.08 (* filter_tone 0.20) (* skew 0.18))) 220) 40 15000))
(def lp3 (biquad lp3_in lp3_cut (+ 0.08 (* q 0.60)) 1 0))
(def res_air (biquad lp3 (clip (+ 2200 (* filter_tone 5200) (* q 240)) 300 16000) 0.75 1 1))
(def filt_core (+ (* lp1 0.16) (* lp2 0.44) (* lp3 0.56)))
(def filt_out (sat (+ (* filt_core 0.94)
                      (* res_air 0.06)
                      (* (sat filt_core (+ 1.0 (* brass 0.45))) 0.04 brass))
                   (+ 1.0 (* drive_excess 0.30))))

(def voiced (* filt_out amp_env velocity))

(def left (* voiced (- 1 (* stereo_spread 0.20))))
(def right (* voiced (+ 1 (* stereo_spread 0.20))))

(out (* left gain) 1 @name left)
(out (* right gain) 2 @name right)