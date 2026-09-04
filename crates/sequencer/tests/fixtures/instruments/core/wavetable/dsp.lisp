; Wavetable — Ableton Wavetable-inspired 2-oscillator wavetable synth.
; Bank: 28 sets x 16 waves x 512 samples in waves/bank.json (wave-major).
; Per osc: set select, wave position morph, Möbius phase warp, triangle
; wavefold — warp/fold math mirrored exactly by the wavetable-viewer widget.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(def bank (tensor @shape [512 448] @file "waves/bank.json"))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

; oscillator 1
(param osc1_set    @default 0  @min 0 @max 27)
(param osc1_wave   @default 0  @min 0 @max 15 @mod true @mod-mode additive)
(param osc1_warp   @default 0  @min 0 @max 1 @mod true @mod-mode additive)
(param osc1_fold   @default 0  @min 0 @max 1 @mod true @mod-mode additive)
(param osc1_semi   @default 0  @min -24 @max 24 @unit st)
(param osc1_detune @default 0  @min -50 @max 50 @unit cents @mod true @mod-mode additive)
(param osc1_gain_db @default -6 @min -60 @max 6 @unit dB @mod true @mod-mode additive)

; oscillator 2
(param osc2_on     @default 0  @min 0 @max 1)
(param osc2_set    @default 0  @min 0 @max 27)
(param osc2_wave   @default 0  @min 0 @max 15 @mod true @mod-mode additive)
(param osc2_warp   @default 0  @min 0 @max 1 @mod true @mod-mode additive)
(param osc2_fold   @default 0  @min 0 @max 1 @mod true @mod-mode additive)
(param osc2_semi   @default -12 @min -24 @max 24 @unit st)
(param osc2_detune @default 0  @min -50 @max 50 @unit cents @mod true @mod-mode additive)
(param osc2_gain_db @default -60 @min -60 @max 6 @unit dB @mod true @mod-mode additive)

; filter
(param filter_mode @default 0    @min 0 @max 2)
(param cutoff      @default 9000 @min 40 @max 16000 @unit Hz @mod true @mod-mode additive)
(param resonance   @default 0.6  @min 0.5 @max 6 @mod true @mod-mode additive)
(param keytrack    @default 0.2  @min 0 @max 2)
(param filter_env_amt @default 1200 @min -10000 @max 10000 @unit Hz)

; envelopes
(param amp_attack_ms  @default 3   @min 1 @max 8000 @unit ms)
(param amp_decay_ms   @default 200 @min 1 @max 8000 @unit ms)
(param amp_sustain    @default 0.8 @min 0 @max 1)
(param amp_release_ms @default 140 @min 1 @max 8000 @unit ms)

(param filt_attack_ms  @default 3   @min 1 @max 8000 @unit ms)
(param filt_decay_ms   @default 320 @min 1 @max 8000 @unit ms)
(param filt_sustain    @default 0.2 @min 0 @max 1)
(param filt_release_ms @default 200 @min 1 @max 8000 @unit ms)

; output
(param volume_db @default -8 @min -60 @max 6 @unit dB @mod true @mod-mode additive)

(defmacro db_gain (db)
  (pow 10 (/ db 20)))

(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

; pre-resolve all host-modulated values (mod must not appear inside macros)
(def o1_wavepos (clip (mod osc1_wave) 0 15))
(def o1_warp (clip (mod osc1_warp) 0 1))
(def o1_fold (clip (mod osc1_fold) 0 1))
(def o1_detune (clip (mod osc1_detune) -50 50))
(def o1_gain (db_gain (clip (mod osc1_gain_db) -60 6)))
(def o2_wavepos (clip (mod osc2_wave) 0 15))
(def o2_warp (clip (mod osc2_warp) 0 1))
(def o2_fold (clip (mod osc2_fold) 0 1))
(def o2_detune (clip (mod osc2_detune) -50 50))
(def o2_gain (db_gain (clip (mod osc2_gain_db) -60 6)))

; oscillator 1
(def o1_freq (* pitch (semi_ratio (+ osc1_semi (/ o1_detune 100)))))
(def o1_phase_raw (phasor o1_freq trigger))
(def o1_k (+ 1 (* 6 o1_warp)))
(def o1_phase (/ (* o1_k o1_phase_raw) (+ 1 (* (- o1_k 1) o1_phase_raw))))
(make-history o1_scan_hist)
(def o1_scan_prev (read-history o1_scan_hist))
(def scan_coeff (- 1.0 (exp (/ -1.0 (* 0.008 samplerate)))))
(def o1_scan
  (gswitch trigger
    o1_wavepos
    (+ o1_scan_prev (* scan_coeff (- o1_wavepos o1_scan_prev)))))
(write-history o1_scan_hist o1_scan)
(def o1_idx (+ (* (clip (floor osc1_set) 0 27) 16) (clip o1_scan 0 15)))
(def o1_raw (wavetable-read bank o1_idx o1_phase))
(def o1_foldg (+ 1 (* 6 o1_fold)))
(def o1_out (- 1 (abs (- (wrap (+ (* o1_raw o1_foldg) 1) 0 4) 2))))

; oscillator 2
(def o2_freq (* pitch (semi_ratio (+ osc2_semi (/ o2_detune 100)))))
(def o2_phase_raw (phasor o2_freq trigger))
(def o2_k (+ 1 (* 6 o2_warp)))
(def o2_phase (/ (* o2_k o2_phase_raw) (+ 1 (* (- o2_k 1) o2_phase_raw))))
(make-history o2_scan_hist)
(def o2_scan_prev (read-history o2_scan_hist))
(def o2_scan
  (gswitch trigger
    o2_wavepos
    (+ o2_scan_prev (* scan_coeff (- o2_wavepos o2_scan_prev)))))
(write-history o2_scan_hist o2_scan)
(def o2_idx (+ (* (clip (floor osc2_set) 0 27) 16) (clip o2_scan 0 15)))
(def o2_raw (wavetable-read bank o2_idx o2_phase))
(def o2_foldg (+ 1 (* 6 o2_fold)))
(def o2_out (- 1 (abs (- (wrap (+ (* o2_raw o2_foldg) 1) 0 4) 2))))

(def mix (+ (* o1_out o1_gain) (* o2_out o2_gain (gt osc2_on 0.5))))

(def filter_cutoff
  (clip (+ (mod cutoff) (* pitch keytrack) (* filt_env filter_env_amt)) 40 16000))
(def filtered (svf mix filter_cutoff (clip (mod resonance) 0.5 6) (clip (floor filter_mode) 0 2)))

(def master (db_gain (clip (mod volume_db) -60 6)))
(out (* filtered amp_env master) 1 @name audio)
