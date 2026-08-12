; Elektron Monomachine DPRO-DENS-inspired v1
; Ensemble oscillator: one user waveform, four chord voices, and chorus spread.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

(def waves (tensor @shape [512 64] @file "waves/user-bank.json"))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro pch_ratio (pch)
  (gswitch (lt pch -64)
    0
    (gswitch (gt pch 67)
      1.5
      (gswitch (gt pch 66)
        1.333333333
        (gswitch (gt pch 65)
          1.25
          (gswitch (gt pch 64)
            1.2
            (semi_ratio pch)))))))

(defmacro pch_on (pch)
  (gt pch -64))

(defmacro dens_voice (freq wave_pos phase_offset detune)
  (wavetable-read waves wave_pos (wrap (+ (phasor (* freq detune)) phase_offset) 0 1)))

(param amp_attack_ms     @default 4    @min 1     @max 5000 @unit ms)
(param amp_decay_ms      @default 180  @min 1     @max 5000 @unit ms)
(param amp_sustain       @default 0.78 @min 0     @max 1)
(param amp_release_ms    @default 140  @min 1     @max 5000 @unit ms)

(param filter_attack_ms  @default 8    @min 1     @max 5000 @unit ms)
(param filter_decay_ms   @default 420  @min 1     @max 5000 @unit ms)
(param filter_sustain    @default 0.24 @min 0     @max 1)
(param filter_release_ms @default 240  @min 1     @max 5000 @unit ms)

(param wave              @default 12   @min 1     @max 64 @mod true @mod-mode additive)
(param pch2              @default 7    @min -128  @max 68 @mod true @mod-mode additive)
(param pch3              @default 12   @min -128  @max 68 @mod true @mod-mode additive)
(param pch4              @default -128 @min -128  @max 68 @mod true @mod-mode additive)
(param chrl              @default 0.24 @min 0     @max 1 @mod true @mod-mode additive)
(param chrw              @default 0.32 @min 0     @max 1 @mod true @mod-mode additive)
(param tune_cents        @default 0    @min -100  @max 100 @unit cents @mod true @mod-mode additive)

(param cutoff            @default 6800 @min 80    @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance         @default 0.707 @min 0.5  @max 2.5 @mod true @mod-mode additive)
(param keytrack          @default 0.18 @min 0     @max 2)
(param filter_env_amt    @default 2200 @min -8000 @max 8000 @unit Hz @mod true @mod-mode additive)
(param drive             @default 1.0  @min 0.5   @max 6 @mod true @mod-mode additive)
(param gain              @default 0.18 @min 0     @max 1 @mod true @mod-mode additive)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filter_attack_ms filter_decay_ms filter_sustain filter_release_ms))
(def tuned_pitch (* pitch (semi_ratio (/ (mod tune_cents) 100))))
(def wave_pos (clip (- (mod wave) 1) 0 63))

(def p2 (clip (mod pch2) -128 68))
(def p3 (clip (mod pch3) -128 68))
(def p4 (clip (mod pch4) -128 68))
(def r2 (pch_ratio p2))
(def r3 (pch_ratio p3))
(def r4 (pch_ratio p4))
(def on2 (pch_on p2))
(def on3 (pch_on p3))
(def on4 (pch_on p4))

(def chorus_level (clip (mod chrl) 0 1))
(def chorus_width (clip (mod chrw) 0 1))
(def detune_amt (* chorus_width chorus_level 0.012))

(def v1a (dens_voice tuned_pitch wave_pos 0.00 (- 1 detune_amt)))
(def v1b (dens_voice tuned_pitch wave_pos 0.31 (+ 1 detune_amt)))
(def v2a (* on2 (dens_voice (* tuned_pitch r2) wave_pos 0.11 (- 1 (* detune_amt 0.74)))))
(def v2b (* on2 (dens_voice (* tuned_pitch r2) wave_pos 0.43 (+ 1 (* detune_amt 0.74)))))
(def v3a (* on3 (dens_voice (* tuned_pitch r3) wave_pos 0.23 (- 1 (* detune_amt 1.17)))))
(def v3b (* on3 (dens_voice (* tuned_pitch r3) wave_pos 0.57 (+ 1 (* detune_amt 1.17)))))
(def v4a (* on4 (dens_voice (* tuned_pitch r4) wave_pos 0.37 (- 1 (* detune_amt 1.41)))))
(def v4b (* on4 (dens_voice (* tuned_pitch r4) wave_pos 0.71 (+ 1 (* detune_amt 1.41)))))

(def voice_count (+ 1 on2 on3 on4))
(def dry (+ (dens_voice tuned_pitch wave_pos 0.0 1)
            (* on2 (dens_voice (* tuned_pitch r2) wave_pos 0.0 1))
            (* on3 (dens_voice (* tuned_pitch r3) wave_pos 0.0 1))
            (* on4 (dens_voice (* tuned_pitch r4) wave_pos 0.0 1))))
(def ens (* (/ (+ v1a v1b v2a v2b v3a v3b v4a v4b) (* voice_count 2)) 1.55))
(def raw_wave (+ (* dry (/ (- 1 chorus_level) voice_count)) (* ens chorus_level)))

(def driven (tanh (* raw_wave (clip (mod drive) 0.5 6))))
(def filter_cutoff (clip (+ (mod cutoff) (* tuned_pitch keytrack) (* filt_env (mod filter_env_amt))) 80 12000))
(def filtered (biquad driven filter_cutoff (clip (mod resonance) 0.5 2.5) 1 0))
(out (* filtered amp_env velocity (clip (mod gain) 0 1)) 1 @name audio)
