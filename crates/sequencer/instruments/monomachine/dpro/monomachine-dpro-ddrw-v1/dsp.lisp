; Elektron Monomachine DPRO-DDRW-inspired v1
; Doubledraw oscillator using a file-backed 512 x 64 user wavetable bank.

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

(defmacro bitreduce_12 (sig amount)
  (- (* (/ (floor (* (+ (clip sig -1 1) 1) 0.5 (pow 2 (clip (- 12 amount) 1 12))))
           (pow 2 (clip (- 12 amount) 1 12)))
        2)
     1))

(param amp_attack_ms     @default 3    @min 1     @max 5000 @unit ms)
(param amp_decay_ms      @default 120  @min 1     @max 5000 @unit ms)
(param amp_sustain       @default 0.78 @min 0     @max 1)
(param amp_release_ms    @default 90   @min 1     @max 5000 @unit ms)

(param filter_attack_ms  @default 6    @min 1     @max 5000 @unit ms)
(param filter_decay_ms   @default 260  @min 1     @max 5000 @unit ms)
(param filter_sustain    @default 0.18 @min 0     @max 1)
(param filter_release_ms @default 180  @min 1     @max 5000 @unit ms)

(param wav1              @default 9    @min 1     @max 64 @mod true @mod-mode additive)
(param mix               @default 0.5  @min 0     @max 1 @mod true @mod-mode additive)
(param wav2              @default 38   @min 1     @max 64 @mod true @mod-mode additive)
(param time              @default 18   @min 0     @max 127)
(param br1               @default 0    @min 0     @max 11 @mod true @mod-mode additive)
(param wid               @default 0    @min 0     @max 127 @mod true @mod-mode additive)
(param br2               @default 0    @min 0     @max 11 @mod true @mod-mode additive)
(param tune_cents        @default 0    @min -100  @max 100 @unit cents @mod true @mod-mode additive)

(param cutoff            @default 7200 @min 80    @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance         @default 0.707 @min 0.5   @max 2.5 @mod true @mod-mode additive)
(param keytrack          @default 0.18 @min 0     @max 2)
(param filter_env_amt    @default 1800 @min -8000 @max 8000 @unit Hz @mod true @mod-mode additive)
(param drive             @default 1.0  @min 0.5   @max 6 @mod true @mod-mode additive)
(param gain              @default 0.14 @min 0     @max 1 @mod true @mod-mode additive)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filter_attack_ms filter_decay_ms filter_sustain filter_release_ms))
(def tuned_pitch (* pitch (semi_ratio (/ (mod tune_cents) 100))))

(def target_wav1 (clip (- (mod wav1) 1) 0 63))
(def target_wav2 (clip (- (mod wav2) 1) 0 63))
(def time_ms (* (/ (clip time 0 127) 127) 650))
(def wave_slew_coeff (gswitch (gt time 0.5) (- 1.0 (exp (/ -1.0 (* (+ time_ms 1.0) 0.001 samplerate)))) 1.0))

(make-history wav1_hist)
(make-history wav2_hist)
(def wav1_prev (read-history wav1_hist))
(def wav2_prev (read-history wav2_hist))
(def wav1_pos (+ wav1_prev (* wave_slew_coeff (- target_wav1 wav1_prev))))
(def wav2_pos (+ wav2_prev (* wave_slew_coeff (- target_wav2 wav2_prev))))
(write-history wav1_hist wav1_pos)
(write-history wav2_hist wav2_pos)

(def phase1 (phasor tuned_pitch))
(def width_semi (* (/ (clip (mod wid) 0 127) 127) 24))
(def phase2 (phasor (* tuned_pitch (semi_ratio width_semi))))

(def wave1_raw (wavetable-read waves (clip wav1_pos 0 63) phase1))
(def wave2_raw (wavetable-read waves (clip wav2_pos 0 63) phase2))
(def wave1 (bitreduce_12 wave1_raw (clip (mod br1) 0 11)))
(def wave2 (bitreduce_12 wave2_raw (clip (mod br2) 0 11)))
(def ddrw_mix (clip (mod mix) 0 1))
(def raw_wave (+ (* wave1 (- 1 ddrw_mix)) (* wave2 ddrw_mix)))

(def driven (tanh (* raw_wave (clip (mod drive) 0.5 6))))
(def filter_cutoff (clip (+ (mod cutoff) (* tuned_pitch keytrack) (* filt_env (mod filter_env_amt))) 80 12000))
(def filtered (biquad driven filter_cutoff (clip (mod resonance) 0.5 2.5) 1 0))
(out (* filtered amp_env velocity (clip (mod gain) 0 1)) 1 @name audio)
