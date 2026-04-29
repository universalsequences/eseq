; Minimoog Model D inspired mono synth
; 3 oscillators, hard sync, Moog 4-pole transistor ladder filter approximation

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
(param amp_attack_ms   @default 4    @min 1    @max 5000 @unit ms)
(param amp_decay_ms    @default 300  @min 1    @max 5000 @unit ms)
(param amp_sustain     @default 0.75 @min 0    @max 1)
(param amp_release_ms  @default 60   @min 1    @max 5000 @unit ms)

; ── Filter ADSR ──
(param filt_attack_ms  @default 4    @min 1    @max 5000 @unit ms)
(param filt_decay_ms   @default 400  @min 1    @max 5000 @unit ms)
(param filt_sustain    @default 0.0  @min 0    @max 1)
(param filt_release_ms @default 100  @min 1    @max 5000 @unit ms)

; ── Filter ──
(param cutoff          @default 900  @min 30   @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance       @default 1.2  @min 0.35 @max 4.8   @mod true @mod-mode additive)
(param filter_env_amt  @default 2800 @min -6000 @max 6000 @unit Hz @mod true @mod-mode additive)
(param keytrack        @default 0.4  @min 0    @max 2)
(param filter_vel_amt  @default 0.3  @min 0    @max 1)
(param filter_drive    @default 1.4  @min 0.5  @max 5     @mod true @mod-mode additive)

; ── Oscillators ──
(param osc1_semi       @default 0    @min -24  @max 24  @unit st)
(param osc2_semi       @default 0    @min -24  @max 24  @unit st)
(param osc3_semi       @default -12  @min -36  @max 12  @unit st)
(param osc2_detune     @default 3    @min -50  @max 50  @unit cents @mod true @mod-mode additive)
(param osc3_detune     @default -5   @min -50  @max 50  @unit cents @mod true @mod-mode additive)

; Waveforms
(param osc1_saw        @default 1.0  @min 0    @max 1)
(param osc1_pulse      @default 0.0  @min 0    @max 1)
(param osc2_saw        @default 1.0  @min 0    @max 1)
(param osc2_pulse      @default 0.0  @min 0    @max 1)
(param osc3_saw        @default 0.0  @min 0    @max 1)
(param osc3_tri        @default 1.0  @min 0    @max 1)
(param pulse_width     @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)

; Mixer
(param osc1_level      @default 0.75 @min 0    @max 1   @mod true @mod-mode additive)
(param osc2_level      @default 0.65 @min 0    @max 1   @mod true @mod-mode additive)
(param osc3_level      @default 0.5  @min 0    @max 1   @mod true @mod-mode additive)
(param noise_level     @default 0.02 @min 0    @max 0.5 @mod true @mod-mode additive)

; ── Other ──
(param sync_1_to_3     @default 0.0  @min 0    @max 1)  ; hard sync osc1 reset by osc3
(param osc3_fm_amt     @default 0.0  @min 0    @max 12  @mod true @mod-mode additive)  ; osc3 audio FM → filter
(param glide_time      @default 0    @min 0    @max 500 @unit ms)
(param amp_vel_amt     @default 0.35 @min 0    @max 1)
(param gain            @default 0.12 @min 0    @max 1)

; ── Signal path ──
(def amp_env   (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env  (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))
(def ln2       (log 2))

; Glide
(def glide_alpha (- 1.0 (exp (/ -3.0 (max 0.1 (* glide_time 44.1))))))
(make-history glide_hist)
(def glide_pitch (+ (* glide_alpha pitch)
                    (* (- 1.0 glide_alpha) (read-history glide_hist))))
(write-history glide_hist glide_pitch)

; Osc frequencies
(def f1  (* glide_pitch (semi_ratio osc1_semi)))
(def f2  (* glide_pitch (semi_ratio (+ osc2_semi (/ (mod osc2_detune) 100)))))
(def f3  (* glide_pitch (semi_ratio (+ osc3_semi (/ (mod osc3_detune) 100)))))
(def pw  (clip (mod pulse_width) 0.05 0.95))

; Osc3 runs as master (for sync + FM)
(def ph3      (phasor f3))
(make-history ph3_hist)
(def ph3_prev (read-history ph3_hist))
(def ph3_wrap (lt ph3 ph3_prev))
(write-history ph3_hist ph3)
(def sync_trig (* (gt sync_1_to_3 0.5) ph3_wrap))

; Osc1 with optional sync to osc3
(def ph1 (phasor f1 sync_trig))
(def ph2 (phasor f2))

(def o1 (+ (* osc1_saw   (scale ph1 0 1 -1 1))
           (* osc1_pulse (pulse_from_phase ph1 pw))))
(def o2 (+ (* osc2_saw   (scale ph2 0 1 -1 1))
           (* osc2_pulse (pulse_from_phase ph2 pw))))
(def o3 (+ (* osc3_saw   (scale ph3 0 1 -1 1))
           (* osc3_tri   (triangle ph3))))

(def mixer (+ (* o1 (clip (mod osc1_level) 0 1))
              (* o2 (clip (mod osc2_level) 0 1))
              (* o3 (clip (mod osc3_level) 0 1))
              (* (noise) (clip (mod noise_level) 0 0.5))))

; Osc3 audio-rate FM into filter cutoff (Minimoog classic trick)
(def osc3_fm (* o3 (mod osc3_fm_amt) 700))

; ── Moog ladder filter: 2x cascaded biquad LP + inter-stage saturation ──
; Cascaded 4-pole response approximation with characteristic ladder resonance
(def filter_vel       (+ (- 1 filter_vel_amt) (* filter_vel_amt velocity)))
(def filter_env_scaled (* filt_env (mod filter_env_amt) filter_vel))
(def safe_cutoff      (clip (+ (mod cutoff)
                                (* glide_pitch keytrack)
                                filter_env_scaled
                                osc3_fm)
                             30 10500))
(def safe_res (clip (mod resonance) 0.35 4.8))
(def driven   (tanh (* mixer (mod filter_drive))))
(def lp1      (biquad driven safe_cutoff safe_res 1 0))
(def lp2      (biquad (tanh (* lp1 1.12)) (* safe_cutoff 0.98) (* safe_res 0.85) 1 0))
(def amp_vel  (+ (- 1 amp_vel_amt) (* amp_vel_amt velocity)))

(out (* (tanh (* lp2 1.25)) amp_env amp_vel gain) 1 @name audio)
