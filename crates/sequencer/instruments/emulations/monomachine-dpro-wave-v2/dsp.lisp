; Elektron Monomachine DPRO-WAVE-inspired v2
; Uses a file-backed 512 x 32 wavetable bank: shape [samples, waves].

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

(def waves (wavetable @shape [512 32] @file "waves/factory.json"))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(param amp_attack_ms   @default 2    @min 1     @max 5000 @unit ms)
(param amp_decay_ms    @default 120  @min 1     @max 5000 @unit ms)
(param amp_sustain     @default 0.78 @min 0     @max 1)
(param amp_release_ms  @default 90   @min 1     @max 5000 @unit ms)

(param filter_attack_ms  @default 6    @min 1     @max 5000 @unit ms)
(param filter_decay_ms   @default 260  @min 1     @max 5000 @unit ms)
(param filter_sustain    @default 0.18 @min 0     @max 1)
(param filter_release_ms @default 180  @min 1     @max 5000 @unit ms)

(param wave            @default 7    @min 1     @max 32)
(param wp              @default 0    @min 0     @max 127 @mod true @mod-mode additive)
(param sync_mode       @default 0    @min 0     @max 2)
(param sfrq            @default 440  @min 20    @max 8000 @unit Hz @mod true @mod-mode additive)
(param tune_cents      @default 0    @min -100  @max 100 @unit cents @mod true @mod-mode additive)

(param cutoff          @default 7400 @min 80    @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance       @default 0.707 @min 0.5   @max 2.5 @mod true @mod-mode additive)
(param keytrack        @default 0.18 @min 0     @max 2)
(param filter_env_amt  @default 1800 @min -8000 @max 8000 @unit Hz @mod true @mod-mode additive)
(param drive           @default 1.1  @min 0.5   @max 6 @mod true @mod-mode additive)
(param gain            @default 0.14 @min 0     @max 1 @mod true @mod-mode additive)

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filter_attack_ms filter_decay_ms filter_sustain filter_release_ms))
(def tuned_pitch (* pitch (semi_ratio (/ (mod tune_cents) 100))))

(def sync_on (gt sync_mode 0.5))
(def sync_master (phasor tuned_pitch))
(make-history sync_hist)
(def sync_prev (read-history sync_hist))
(def sync_wrap (* (lt sync_master sync_prev) sync_on))
(write-history sync_hist sync_master)

(def wp_steps (clip (mod wp) 0 127))

(def sync_fixed_freq (clip (mod sfrq) 20 8000))
(def sync_keytrack_freq (clip (+ tuned_pitch (mod sfrq)) 20 12000))
(def sync_slave_freq (gswitch (gt sync_mode 1.5) sync_keytrack_freq sync_fixed_freq))
(def osc_freq (gswitch sync_on sync_slave_freq tuned_pitch))
(def osc_phase (phasor osc_freq sync_wrap))
(def base_wave (clip (floor (- wave 1)) 0 31))
(def scan_span (- 31 base_wave))
(def scan_target (+ base_wave (* (/ wp_steps 127) scan_span)))
(make-history scan_hist)
(def scan_prev (read-history scan_hist))
(def scan_slew_coeff (- 1.0 (exp (/ -1.0 (* 0.012 44100.0)))))
(def scan_pos
  (gswitch trigger
    scan_target
    (+ scan_prev (* scan_slew_coeff (- scan_target scan_prev)))))
(write-history scan_hist scan_pos)
(def raw_wave (wavetable-read-512 waves (clip scan_pos 0 31) osc_phase))
(def driven (tanh (* raw_wave (clip (mod drive) 0.5 6))))
(def filter_cutoff (clip (+ (mod cutoff) (* tuned_pitch keytrack) (* filt_env (mod filter_env_amt))) 80 12000))
(def filtered (biquad driven filter_cutoff (clip (mod resonance) 0.5 2.5) 1 0))
(out (* filtered amp_env velocity (clip (mod gain) 0 1)) 1 @name audio)
