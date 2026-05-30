; Digitone-inspired 4-operator PM synthesizer
; Operators: C (carrier), A, B1, B2
; 8 algorithms with X/Y channel mix
; Per-operator mod envelopes with hold-on-release behavior
; Phase modulation with 1-sample delay (history) between all operator connections
; Feedback with onepole lowpass at operator pitch frequency

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))

; ── Algorithm ──
(param algorithm       @default 2    @min 1    @max 8)

; ── Global amp envelope ──
(param amp_attack      @default 4    @min 1    @max 5000 @unit ms)
(param amp_decay       @default 500  @min 1    @max 5000 @unit ms)
(param amp_sustain     @default 0.7  @min 0    @max 1)
(param amp_release     @default 300  @min 1    @max 5000 @unit ms)

; ── Mix between channel X and Y ──
(param mix_xy          @default 0.5  @min 0    @max 1    @mod true @mod-mode additive)

; ── Operator C (primary carrier) ──
(param c_ratio         @default 1.0  @min 0.25 @max 16   @mod true @mod-mode additive)
(param c_detune        @default 0.0  @min -50  @max 50   @unit Hz  @mod true @mod-mode additive)
(param c_level         @default 1.0  @min 0    @max 1    @mod true @mod-mode additive)
(param c_harmonics     @default 0.0  @min 0    @max 5    @mod true @mod-mode additive)
(param c_octave        @default 0    @min -3   @max 3)

; ── Operator A ──
(param a_ratio         @default 1.0  @min 0.25 @max 16   @mod true @mod-mode additive)
(param a_detune        @default 0.0  @min -50  @max 50   @unit Hz  @mod true @mod-mode additive)
(param a_level         @default 0.8  @min 0    @max 1    @mod true @mod-mode additive)
(param a_index         @default 3.0  @min 0    @max 12   @mod true @mod-mode additive)
(param a_harmonics     @default 0.0  @min 0    @max 5    @mod true @mod-mode additive)
(param a_octave        @default 0    @min -3   @max 3)

; ── Operator B (shared controls for B1 and B2) ──
(param b_ratio         @default 1.0  @min 0.25 @max 16   @mod true @mod-mode additive)
(param b_detune        @default 0.0  @min -50  @max 50   @unit Hz  @mod true @mod-mode additive)
(param b_level         @default 0.6  @min 0    @max 1    @mod true @mod-mode additive)
(param b_index         @default 2.0  @min 0    @max 12   @mod true @mod-mode additive)
(param b_harmonics     @default 0.0  @min 0    @max 5    @mod true @mod-mode additive)
(param b_octave        @default 0    @min -3   @max 3)

; ── Feedback (active operator depends on algorithm: A for 1-3/5-8, B2 for 4) ──
(param feedback        @default 0.3  @min 0    @max 2    @mod true @mod-mode additive)

; ── Per-operator mod envelopes (A and B only, C uses amp envelope) ──
; Hold-on-release: release is hardcoded to 30000ms so the envelope effectively
; freezes when gate goes off. The global amp envelope handles the actual fadeout,
; preserving timbral character through the release tail.

; A envelope
(param a_env_attack    @default 4    @min 1    @max 5000 @unit ms)
(param a_env_decay     @default 400  @min 1    @max 5000 @unit ms)
(param a_env_sustain   @default 0.0  @min 0    @max 1)

; B envelope (shared B1 + B2)
(param b_env_attack    @default 4    @min 1    @max 5000 @unit ms)
(param b_env_decay     @default 600  @min 1    @max 5000 @unit ms)
(param b_env_sustain   @default 0.2  @min 0    @max 1)

; ── Multi-mode filter ──
; biquad modes: 0=LP, 1=HP, 2=BP, 3=notch
(param filt_mode       @default 0    @min 0    @max 3)
(param filt_cutoff     @default 8000 @min 20   @max 20000 @unit Hz @mod true @mod-mode additive)
(param filt_res        @default 0.7  @min 0.1  @max 12   @mod true @mod-mode additive)
(param filt_env_depth  @default 0.0  @min -1   @max 1    @mod true @mod-mode additive)

; Filter envelope
(param filt_attack     @default 4    @min 1    @max 5000 @unit ms)
(param filt_decay      @default 500  @min 1    @max 5000 @unit ms)
(param filt_sustain    @default 0.0  @min 0    @max 1)
(param filt_release    @default 300  @min 1    @max 5000 @unit ms)

; ── Velocity and output ──
(param vel_sensitivity @default 0.5  @min 0    @max 1)
(param gain            @default 0.15 @min 0    @max 1)

; ══════════════════════════════════════════════════════════════
; Signal path
; ══════════════════════════════════════════════════════════════

; ── Global amp envelope ──
(def amp_env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))

; ── Per-operator mod envelopes (hold on release = 30s release) ──
(def env_a (adsr gate trigger a_env_attack a_env_decay a_env_sustain 30000))
(def env_b (adsr gate trigger b_env_attack b_env_decay b_env_sustain 30000))

; ── Filter envelope ──
(def filt_env (adsr gate trigger filt_attack filt_decay filt_sustain filt_release))

; ── Velocity scaling ──
(def vel_scale (+ (- 1 vel_sensitivity) (* vel_sensitivity velocity)))

; ── Operator frequencies ──
(def c_freq  (max 0.1 (+ (* pitch (pow 2 c_octave) (clip (mod c_ratio) 0.25 16)) (mod c_detune))))
(def a_freq  (max 0.1 (+ (* pitch (pow 2 a_octave) (clip (mod a_ratio) 0.25 16)) (mod a_detune))))
(def b1_freq (max 0.1 (+ (* pitch (pow 2 b_octave) (clip (mod b_ratio) 0.25 16)) (mod b_detune))))
(def b2_freq (max 0.1 (+ (* pitch (pow 2 b_octave) (clip (mod b_ratio) 0.25 16)) (mod b_detune))))

; ── Operator phases ──
(def ph_c  (phasor c_freq))
(def ph_a  (phasor a_freq))
(def ph_b1 (phasor b1_freq))
(def ph_b2 (phasor b2_freq))

; ── History buffers (1-sample delay for all modulation connections) ──
(make-history hist_c)
(make-history hist_a)
(make-history hist_b1)
(make-history hist_b2)

(def prev_c  (read-history hist_c))
(def prev_a  (read-history hist_a))
(def prev_b1 (read-history hist_b1))
(def prev_b2 (read-history hist_b2))

; ── Scaled modulation outputs (prev_frame * index * envelope * velocity) ──
; Each operator's modulation output = its previous output scaled by its index and envelope
(def mod_a  (* prev_a  (clip (mod a_index) 0 12) env_a vel_scale))
(def mod_b1 (* prev_b1 (clip (mod b_index) 0 12) env_b vel_scale))
(def mod_b2 (* prev_b2 (clip (mod b_index) 0 12) env_b vel_scale))

; ── Feedback with onepole lowpass at operator pitch ──
; Only A (algos 1-3,5-8) and B2 (algo 4) ever have feedback
(def fb_a_cutoff  (clip a_freq  20 (* samplerate 0.45)))
(def fb_b2_cutoff (clip b2_freq 20 (* samplerate 0.45)))
(def fb_a  (biquad (* prev_a  (mod feedback)) fb_a_cutoff  0.5 1 0))
(def fb_b2 (biquad (* prev_b2 (mod feedback)) fb_b2_cutoff 0.5 1 0))

; ── Per-operator harmonics (self-PM waveshaping) ──
; sin(phase + harm * sin(phase)) morphs from sine → triangle → saw → square territory
; At 0 = pure sine, increasing values add progressively richer harmonics
(def c_harm  (* (mod c_harmonics) (sin (* twopi ph_c))))
(def a_harm  (* (mod a_harmonics) (sin (* twopi ph_a))))
(def b1_harm (* (mod b_harmonics) (sin (* twopi ph_b1))))
(def b2_harm (* (mod b_harmonics) (sin (* twopi ph_b2))))

; ══════════════════════════════════════════════════════════════
; Unique operator computations (shared across algorithms)
; All operators include their harmonics self-PM term
; ══════════════════════════════════════════════════════════════

; ── C variants ──
(def c_m_a    (sin (+ (* twopi ph_c) c_harm mod_a)))           ; C modulated by A
(def c_m_a_b2 (sin (+ (* twopi ph_c) c_harm mod_a mod_b2)))   ; C modulated by A + B2

; ── A variants ──
(def a_fb        (sin (+ (* twopi ph_a) a_harm fb_a)))             ; A with feedback only
(def a_m_b2_fb   (sin (+ (* twopi ph_a) a_harm mod_b2 fb_a)))     ; A with B2 mod + feedback
(def a_m_b1      (sin (+ (* twopi ph_a) a_harm mod_b1)))           ; A modulated by B1 (no fb, algo 4)
(def a_m_b1b2_fb (sin (+ (* twopi ph_a) a_harm mod_b1 mod_b2 fb_a))) ; A with B1+B2 mod + feedback

; ── B1 variants ──
(def b1_free  (sin (+ (* twopi ph_b1) b1_harm)))                ; B1 free (harmonics only)
(def b1_m_b2  (sin (+ (* twopi ph_b1) b1_harm mod_b2)))        ; B1 modulated by B2
(def b1_m_a   (sin (+ (* twopi ph_b1) b1_harm mod_a)))         ; B1 modulated by A
(def b1_m_a_b2 (sin (+ (* twopi ph_b1) b1_harm mod_a mod_b2))) ; B1 modulated by A + B2

; ── B2 variants ──
(def b2_free (sin (+ (* twopi ph_b2) b2_harm)))                ; B2 free (harmonics only)
(def b2_fb   (sin (+ (* twopi ph_b2) b2_harm fb_b2)))         ; B2 with feedback (algo 4)
(def b2_m_a  (sin (+ (* twopi ph_b2) b2_harm mod_a)))         ; B2 modulated by A

; ══════════════════════════════════════════════════════════════
; Per-algorithm X/Y channel outputs
; ══════════════════════════════════════════════════════════════

; ── Algorithm 1: B2→A, B2→B1, A→C ──────────────────────────
; B2 modulates both A and B1. A modulates C.
; X=C  Y=B1  FB=A
(def x_1 (* c_m_a    (mod c_level)))
(def y_1 (* b1_m_b2  (mod b_level) env_b))

; ── Algorithm 2: A→C, B2→B1 ────────────────────────────────
; Two independent modulator→carrier pairs
; X=C  Y=B1  FB=A
(def x_2 (* c_m_a    (mod c_level)))
(def y_2 (* b1_m_b2  (mod b_level) env_b))

; ── Algorithm 3: A→C, A→B1, A→B2 ───────────────────────────
; A fans out to modulate all three carriers
; X=C  Y=B1+B2  FB=A
(def x_3 (* c_m_a (mod c_level)))
(def y_3 (* (+ b1_m_a b2_m_a) 0.5 (mod b_level) env_b))

; ── Algorithm 4: B2→B1→A→C (full cascade) ──────────────────
; Deep harmonic stacking. Single carrier output.
; X=C  Y=C  FB=B2
(def x_4 (* c_m_a (mod c_level)))
(def y_4 (* c_m_a (mod c_level)))

; ── Algorithm 5: B1→A, B2→A, A→C ───────────────────────────
; Two modulators feed A, A modulates C. A also outputs to Y as carrier.
; X=C  Y=A  FB=A
(def x_5 (* c_m_a       (mod c_level)))
(def y_5 (* a_m_b1b2_fb (mod a_level) env_a))

; ── Algorithm 6: cross-mod A→(C,B1), B2→(C,B1) ─────────────
; A and B2 both cross-modulate C and B1
; X=C  Y=B1  FB=A
(def x_6 (* c_m_a_b2   (mod c_level)))
(def y_6 (* b1_m_a_b2  (mod b_level) env_b))

; ── Algorithm 7: A→C, B2→B1 (both carriers to both channels) ─
; Same pairs as algo 2 but both carriers appear in both channels.
; Operator envelopes affect different channels (solid vs dotted lines).
; X = C(env) + B1(flat)   Y = C(flat) + B1(env)
; The mix crossfades which carrier's envelope character dominates.
; X=C(env)+B1  Y=C+B1(env)  FB=A
(def x_7 (+ (* c_m_a   (mod c_level)) (* b1_m_b2 (mod b_level) 0.5)))
(def y_7 (+ (* c_m_a   (mod c_level) 0.5)   (* b1_m_b2 (mod b_level) env_b)))

; ── Algorithm 8: A→C, B1+B2 free carriers ──────────────────
; A modulates C. B1 and B2 are independent additive carriers.
; X=C  Y=B1+B2  FB=A
(def x_8 (* c_m_a (mod c_level)))
(def y_8 (* (+ b1_free b2_free) 0.5 (mod b_level) env_b))

; ══════════════════════════════════════════════════════════════
; Algorithm selection + history write
; ══════════════════════════════════════════════════════════════

(def chan_x (selector algorithm x_1 x_2 x_3 x_4 x_5 x_6 x_7 x_8))
(def chan_y (selector algorithm y_1 y_2 y_3 y_4 y_5 y_6 y_7 y_8))

; Write the selected algorithm's raw operator outputs to history for next frame
(write-history hist_c  (selector algorithm c_m_a    c_m_a    c_m_a    c_m_a   c_m_a       c_m_a_b2 c_m_a   c_m_a))
(write-history hist_a  (selector algorithm a_m_b2_fb a_fb    a_fb     a_m_b1  a_m_b1b2_fb a_fb     a_fb    a_fb))
(write-history hist_b1 (selector algorithm b1_m_b2  b1_m_b2  b1_m_a  b1_m_b2 b1_free     b1_m_a_b2 b1_m_b2 b1_free))
(write-history hist_b2 (selector algorithm b2_free  b2_free  b2_m_a  b2_fb   b2_free     b2_free  b2_free b2_free))

; ══════════════════════════════════════════════════════════════
; Mix + filter + output
; ══════════════════════════════════════════════════════════════

(def mixed (mix chan_x chan_y (clip (mod mix_xy) 0 1)))

; ── Multi-mode filter with envelope modulation ──
; Envelope depth scales cutoff: positive = brighter on attack, negative = darker
(def filt_cutoff_mod (clip (+ (mod filt_cutoff) (* (mod filt_env_depth) 10000 filt_env)) 20 20000))
(def filtered (biquad mixed filt_cutoff_mod (clip (mod filt_res) 0.1 12) 1 filt_mode))

; ── Output ──
(def amp_vel (+ (- 1 vel_sensitivity) (* vel_sensitivity velocity)))
(out (* (tanh (* filtered 1.5)) amp_env amp_vel gain) 1 @name audio)
