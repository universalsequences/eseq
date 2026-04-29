; Oberheim SEM inspired analog synthesizer
; 2-pole state variable filter with continuous LP→BP→HP→notch morph
; Two oscillators (saw + variable pulse), noise, dedicated filter envelope

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))
(def mod5     (in 9  @name mod5 @modulator 5))
(def mod6     (in 10 @name mod6 @modulator 6))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

; ── Amp ADSR ──
(param amp_attack_ms   @default 6    @min 1    @max 5000 @unit ms)
(param amp_decay_ms    @default 180  @min 1    @max 5000 @unit ms)
(param amp_sustain     @default 0.75 @min 0    @max 1)
(param amp_release_ms  @default 200  @min 1    @max 5000 @unit ms)

; ── Filter ADSR ──
(param filt_attack_ms  @default 6    @min 1    @max 5000 @unit ms)
(param filt_decay_ms   @default 280  @min 1    @max 5000 @unit ms)
(param filt_sustain    @default 0.0  @min 0    @max 1)
(param filt_release_ms @default 220  @min 1    @max 5000 @unit ms)

; ── Filter ──
; The SEM SVF outputs LP, BP, HP, notch simultaneously — mode morphs between them
(param cutoff          @default 1800 @min 30   @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance       @default 1.0  @min 0.35 @max 4.8   @mod true @mod-mode additive)
(param filter_env_amt  @default 2200 @min -5000 @max 5000 @unit Hz @mod true @mod-mode additive)
(param filter_mode     @default 0.0  @min 0    @max 3)    ; 0=LP, 1=BP, 2=HP, 3=notch (continuous)
(param keytrack        @default 0.3  @min 0    @max 2)
(param filter_vel_amt  @default 0.3  @min 0    @max 1)
(param filter_drive    @default 1.2  @min 0.5  @max 4     @mod true @mod-mode additive)

; ── Oscillators ──
(param osc_a_semi      @default 0    @min -24  @max 24  @unit st @mod true @mod-mode semitone)
(param osc_b_semi      @default 0    @min -36  @max 36  @unit st)
(param osc_b_detune    @default 4    @min -50  @max 50  @unit cents @mod true @mod-mode additive)
(param osc_b_keytrack  @default 1.0  @min 0    @max 1)  ; 0 = osc B free-running at fixed freq

(param osc_a_saw       @default 1.0  @min 0    @max 1   @mod true @mod-mode additive)
(param osc_a_pulse     @default 0.0  @min 0    @max 1   @mod true @mod-mode additive)
(param osc_b_saw       @default 1.0  @min 0    @max 1   @mod true @mod-mode additive)
(param osc_b_pulse     @default 0.0  @min 0    @max 1   @mod true @mod-mode additive)
(param pulse_width     @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param noise_level     @default 0.03 @min 0    @max 0.5  @mod true @mod-mode additive)

(param osc_a_level     @default 0.8  @min 0    @max 1   @mod true @mod-mode additive)
(param osc_b_level     @default 0.6  @min 0    @max 1   @mod true @mod-mode additive)

; ── Drift & other ──
(param vintage         @default 0.08 @min 0    @max 1)  ; analog instability per note
(param amp_vel_amt     @default 0.3  @min 0    @max 1)
(param gain            @default 0.14 @min 0    @max 1)

; ── Signal path ──
(def amp_env    (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env   (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))
(def ln2        (log 2))

; Per-note analog drift
(def drift_a    (* (latch (noise) trigger) vintage 4))  ; cents
(def drift_b    (* (latch (noise) trigger) vintage 4))
(def drift_cut  (* (latch (noise) trigger) vintage 120))

; Oscillator A
(def osc_a_ratio (exp (* ln2 (/ (+ (mod osc_a_semi) (/ drift_a 100)) 12))))
(def ph_a        (phasor (* pitch osc_a_ratio)))
(def pw          (clip (mod pulse_width) 0.05 0.95))
(def osc_a_sig   (+ (* (mod osc_a_saw)   (scale ph_a 0 1 -1 1))
                    (* (mod osc_a_pulse) (scale (lt ph_a pw) 0 1 -1 1))))

; Oscillator B (can be detuned from pitch or free-running)
(def osc_b_pitch  (* (mix 440.0 pitch (clip osc_b_keytrack 0 1))
                     (exp (* ln2 (/ (+ osc_b_semi (/ (+ (mod osc_b_detune) drift_b) 100)) 12)))))
(def ph_b         (phasor osc_b_pitch))
(def osc_b_sig    (+ (* (mod osc_b_saw)   (scale ph_b 0 1 -1 1))
                     (* (mod osc_b_pulse) (scale (lt ph_b pw) 0 1 -1 1))))

(def mixer  (+ (* osc_a_sig (clip (mod osc_a_level) 0 1))
               (* osc_b_sig (clip (mod osc_b_level) 0 1))
               (* (noise)   (clip (mod noise_level) 0 0.5))))

; ── State variable filter: 2-pole, all modes simultaneously available ──
; Continuous morph: 0=LP → 1=BP → 2=HP → 3=notch
(def filter_vel   (+ (- 1 filter_vel_amt) (* filter_vel_amt velocity)))
(def safe_cutoff  (clip (+ (mod cutoff)
                            (* pitch keytrack)
                            (* filt_env (mod filter_env_amt) filter_vel)
                            drift_cut)
                         30 10500))
(def safe_res     (clip (mod resonance) 0.35 4.8))
(def driven       (tanh (* mixer (mod filter_drive))))

(def svf_lp    (biquad driven safe_cutoff safe_res 1 0))
(def svf_bp    (biquad driven safe_cutoff safe_res 1 2))
(def svf_hp    (biquad driven safe_cutoff safe_res 1 1))
(def svf_notch (biquad driven safe_cutoff safe_res 1 3))

; Segment-based morph between the four modes
(def t        (clip filter_mode 0 3))
(def seg0     (mix svf_lp    svf_bp    (clip t 0 1)))            ; LP→BP
(def seg1     (mix svf_bp    svf_hp    (clip (- t 1) 0 1)))      ; BP→HP
(def seg2     (mix svf_hp    svf_notch (clip (- t 2) 0 1)))      ; HP→notch
(def in_seg0  (* (lt t 1.0) (gte t 0.0)))
(def in_seg12 (gte t 1.0))
(def seg_hi   (mix seg1 seg2 (clip (- t 1.0) 0 1)))
(def filtered (mix seg0 seg_hi (clip t 0 1)))

(def amp_vel  (+ (- 1 amp_vel_amt) (* amp_vel_amt velocity)))
(out (* filtered amp_env amp_vel gain) 1 @name audio)
