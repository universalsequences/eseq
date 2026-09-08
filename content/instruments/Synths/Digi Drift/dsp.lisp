; Factory Drift — subtractive synth in the factory macro-vocabulary style
; (eseq-i2pw). Same architecture as core/drift (two morphing oscillators +
; noise, per-source filter routing, two envelopes with cycling env2, one
; multi-wave LFO, compact mod matrix, per-voice analog drift) but built for
; the patch editor:
;   - the top level is section macro nodes wired together, no bare math;
;   - non-modulated params are declared inside the section that owns them,
;     so the top layer carries only the host-modulatable params;
;   - host-modulatable params stay top-level ((mod p) inside a macro body
;     does not compile, and @mod metadata is dropped for macro-local params);
;     (mod p) passed as a macro argument resolves at top level and renders
;     inline as p~ in the patcher.
;
; Filters calibrated from native Drift measurements (tools/drift).
; Native-rate emulation: Type I nonlinear SVF, Type II Sallen-Key cascade.
; Oscillator gains set the input drive; output volume follows saturation.
; No oversampling or exact native-circuit equivalence is claimed.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; ======================================================================
; host-modulatable params (must live at top level; see header)
; ======================================================================

(param osc1_shape @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param osc1_gain_db @default -6 @min -36 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param osc2_detune @default 0 @min -12 @max 12 @unit st @mod true @mod-mode additive @mod-depth-min -12 @mod-depth-max 12 @mod-unit st)
(param osc2_gain_db @default -6 @min -36 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param noise_gain_db @default -60 @min -60 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param lp_freq @default 2500 @min 20 @max 18000 @unit Hz @mod true @mod-mode additive @mod-depth-min -8000 @mod-depth-max 8000 @mod-unit Hz)
(param lp_res @default 0.2 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param hp_freq @default 20 @min 20 @max 10000 @unit Hz @mod true @mod-mode additive @mod-depth-min -5000 @mod-depth-max 5000 @mod-unit Hz)
(param lfo_amount @default 1 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param drift @default 0.3 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param volume_db @default -12 @min -36 @max 6 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)

; ======================================================================
; math / state helpers (leaf macros, collapsed inside section layers)
; ======================================================================

(defmacro semi-ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro db-amp (db)
  (exp (* 0.1151292546 db)))

; Resettable phase accumulator: restarts at 0 when reset fires.
; Not the builtin (accum inc reset 0 1): probe 2026-09-01 showed accum
; differs at reset/wrap samples, and hard phase=0 on the trigger sample is
; what keeps retriggered LFO/cyc-env starts deterministic.
(defmacro retrig-phasor (freq reset)
  (make-history ph_hist)
  (def prev_ph (read-history ph_hist))
  (def next_ph (wrap (+ prev_ph (/ freq samplerate)) 0 1))
  (def ph (gswitch (gt reset 0.5) 0.0 next_ph))
  (write-history ph_hist ph)
  ph)

; 1-sample pulse when a 0..1 phasor wraps around.
(defmacro wrap-trigger (ph)
  (make-history prev_hist)
  (def prev (read-history prev_hist))
  (def wrapped (lt ph prev))
  (write-history prev_hist ph)
  wrapped)

; One-pole smoother toward a target; rate_hz sets the tracking speed.
(defmacro smooth-toward (target rate_hz)
  (make-history sm_hist)
  (def prev (read-history sm_hist))
  (def coeff (clip (/ rate_hz samplerate) 0.00001 1))
  (def v (+ prev (* coeff (- target prev))))
  (write-history sm_hist v)
  v)

; Solve a*u - b - r*S(u) = 0 for a > r >= 0, threshold > 0,
; curve > 0. S is odd, linear through threshold, then approaches
; threshold + 1/curve with slope 1/(1+curve*excess)^2.
; The equation is strictly increasing, so its real root is unique.
(defmacro drift-feedback-root (a b r threshold curve)
  (def slope (- a r))
  (def magnitude (abs b))
  (def excess (max (- magnitude (* slope threshold)) 0))
  (def quadratic (* a curve))
  (def middle (- slope (* curve excess)))
  (def discriminant (sqrt (+ (* middle middle) (* 4 quadratic excess))))
  ; Use the cancellation-free quadratic form on each side of middle=0.
  ; The guarded denominator belongs to the inactive branch when cancellation
  ; rounds it to zero; DGen may evaluate both gswitch inputs.
  (def curved-excess (gswitch (gte middle 0)
    (/ (* 2 excess) (max (+ middle discriminant) 1e-20))
    (/ (- discriminant middle) (* 2 quadratic))))
  (def root-magnitude (gswitch (gt excess 0)
    (+ threshold curved-excess) (/ magnitude slope)))
  (* (gswitch (lt b 0) -1 1) root-magnitude))

(defmacro drift-knee (x threshold curve)
  (def magnitude (abs x))
  (def excess (max (- magnitude threshold) 0))
  (* (gswitch (lt x 0) -1 1)
     (+ (min magnitude threshold) (/ excess (+ 1 (* curve excess))))))

(defmacro drift-type1-step (x g resonance s1 s2
                                    pre-th pre-curve post-th post-curve
                                    fb-th fb-curve bias fb-bias low-mix)
  (def pre (- (drift-knee (+ x bias) pre-th pre-curve)
              (drift-knee bias pre-th pre-curve)))
  (def k (/ 1 .423))
  (def c (- 0 low-mix))
  (def offset (* fb-th fb-bias))
  (def scale (+ 1 (* c g)))
  (def a (+ 1 (* g k) (* g g) (* g g k resonance c)))
  (def shift (+ (* c s2) offset))
  (def b (- (+ s1 (* g pre))
             (+ (* g s2) (* g k resonance offset) (* g k resonance c s2))))
  (def root-b (+ (* b scale) (* a shift)))
  (def root-r (* g k resonance scale))
  (def root (drift-feedback-root a root-b root-r fb-th fb-curve))
  ; In the linear region, solve directly for bp to avoid subtracting the
  ; feedback bias from an almost equal root at very low signal levels.
  (def linear-bp (/ (+ s1 (* g (- pre s2)))
                    (+ 1 (* g k (- 1 resonance)) (* g g))))
  (def bp (gswitch (lte (abs root-b) (* (- a root-r) fb-th))
            linear-bp (/ (- root shift) scale)))
  (def lp (+ s2 (* g bp)))
  (tuple (drift-knee (* lp 1.541) post-th post-curve)
         (- (* 2 bp) s1) (- (* 2 lp) s2)))

; Measured 12 dB response with asymmetric input saturation and nonlinear
; feedback. Q=.423/(1-resonance) in the small-signal limit. The low-pass
; contribution inside the feedback shaper changes how loud notes damp resonance.
(defmacro drift-type1 (x cutoff resonance)
  (make-history band_state)
  (make-history low_state)
  (def g (tan (/ (* pi cutoff) samplerate)))
  (def (y next_band next_low)
    (drift-type1-step x g resonance
      (read-history band_state) (read-history low_state)
      .4597608481 51.28837662 .2855820088 .4684337539
      .2377988585 .4106747694 .1626344034 .6038363968 .3779898126))
  (write-history band_state next_band)
  (write-history low_state next_low)
  y)

; Trapezoidal Sallen-Key section with an implicitly solved feedback limiter.
; For this resonance law k<2.03, so (1+g)^2-g*k is positive for every g>0.
; No iterative solve or feedback-state clamp is needed.
(defmacro drift-sallen-key (x g k threshold curve bias)
  (make-history state1)
  (make-history state2)
  (def s1 (read-history state1))
  (def s2 (read-history state2))
  (def a (* (+ 1 g) (+ 1 g)))
  (def r (* g k))
  (def b (+ (* (+ 1 g) s2) (* g s1) (* g g x)))
  (def offset (* threshold bias))
  (def root-b (+ b (* (- a r) offset)))
  (def u (drift-feedback-root a root-b r threshold curve))
  (def lp (gswitch (lte (abs root-b) (* (- a r) threshold))
            (/ b (- a r)) (- u offset)))
  (def feedback (- (drift-knee (+ lp offset) threshold curve) offset))
  (def v1 (/ (+ s1 (* g x) (* k feedback)) (+ 1 g)))
  (write-history state1 (- (* 2 (- v1 (* k feedback))) s1))
  (write-history state2 (- (* 2 lp) s2))
  lp)

; The low-frequency Type-II reference differs from Type I by four gentle
; DC-blocking poles. Keep these separate from the adjustable high-pass.
(defmacro drift-dc-block (x)
  (make-history state)
  (def s (read-history state))
  (def g (tan (/ (* pi 1.6) samplerate)))
  (def v (* (- x s) (/ g (+ 1 g))))
  (def low (+ v s))
  (write-history state (+ low v))
  (- x low))

; Four-pole approximation of the measured Type-II response: two Sallen-Key
; sections, with the high-Q section first and a distinct soft feedback limiter
; in each. This captures MS2-style resonance, not the full native OTA circuit.
(defmacro drift-type2 (x cutoff resonance)
  (def r2 (* resonance resonance))
  (def k1 (/ (+ (* 4.1256140047 resonance) (* 1.8359078001 r2))
             (+ 1 (* 3.2117741295 resonance))))
  (def k2 (/ (- (* 8.0522398087 resonance) (* .06219915383 r2))
             (+ 1 (* 2.9532898642 resonance))))
  (def g1 (tan (/ (* pi cutoff 1.0219682037) samplerate)))
  (def g2 (tan (/ (* pi cutoff 1.0226839218) samplerate)))
  (def pre (- (drift-knee (+ x .1550136067) .4581724596 199.9745374)
              .1550136067))
  (def first (drift-sallen-key pre g2 k2 .2707240824 5.680632221 .07640951001))
  (def second (drift-sallen-key first g1 k1 .1027683762 8.333819518 .07640951001))
  (def shaped (drift-knee (* second 1.2966450151) 1.082975062 36.82033997))
  (drift-dc-block (drift-dc-block (drift-dc-block (drift-dc-block shaped)))))

; Mod source selector: 0=env1 1=env2 2=lfo 3=key 4=vel.
(defmacro pick-source (idx e1 e2 lf ky vl)
  (selector (+ (clip (round idx) 0 4) 1) e1 e2 lf ky vl))

; One mod-matrix slot routed to destination d: amt*src when dest==d else 0.
(defmacro route-if-dest (dest d amt src_val)
  (* (eq (clip (round dest) 0 8) d) amt src_val))

; ======================================================================
; section macros (one collapsed node each at the top level)
; ======================================================================

; Exponential glide toward the played pitch; bypasses when glide time ~0.
(defmacro glide-to (target)
  (param glide_ms @default 0 @min 0 @max 1000 @unit ms)
  (def coeff (exp (/ -1.0 (max 1.0 (* glide_ms 0.001 samplerate)))))
  (make-history gl_hist)
  (def prev (read-history gl_hist))
  (def glided (+ (* target (- 1 coeff)) (* prev coeff)))
  (def held (gswitch (gt glide_ms 0.1) glided target))
  (write-history gl_hist held)
  held)

; Key follow value: 0 at C4 (261.63 Hz), +/-1 per two octaves.
(defmacro key-follow (hz)
  (clip (* (/ (log (/ (max hz 8.0) 261.63)) (log 2)) 0.5) -1 1))

; Per-voice analog drift: randoms latched at note start plus a slow wander.
; -> (pitch_cents osc2_extra_cents filter_oct pan_rnd)
(defmacro analog-drift (amount trig)
  (def amt (clip amount 0 1))
  (def rnd_pitch (latch (noise) trig))
  (def rnd_filt (latch (noise) trig))
  (def rnd_pan (latch (noise) trig))
  (def wander_ph (phasor 0.17))
  (def wander_target (latch (noise) (wrap-trigger wander_ph)))
  (def wander_val (smooth-toward wander_target 4.0))
  (tuple (* amt (+ (* rnd_pitch 3.0) (* wander_val 2.0)))
         (* amt rnd_pitch -2.5)
         (* amt rnd_filt 0.25)
         rnd_pan))

; Amp envelope.
(defmacro amp-env (gate_in trig)
  (param env1_attack @default 4 @min 1 @max 5000 @unit ms)
  (param env1_decay @default 350 @min 5 @max 10000 @unit ms)
  (param env1_sustain @default 0.75 @min 0 @max 1)
  (param env1_release @default 250 @min 5 @max 12000 @unit ms)
  (adsr gate_in trig env1_attack env1_decay env1_sustain env1_release))

; env2: ADSR or note-retriggered rise/hold/fall cycling envelope.
(defmacro env2-select (gate_in trig)
  (param env2_mode @default 0 @min 0 @max 1)
  (param env2_attack @default 2 @min 1 @max 5000 @unit ms)
  (param env2_decay @default 400 @min 5 @max 10000 @unit ms)
  (param env2_sustain @default 0.0 @min 0 @max 1)
  (param env2_release @default 300 @min 5 @max 12000 @unit ms)
  (param cyc_rate_hz @default 2 @min 0.05 @max 40 @unit Hz)
  (param cyc_tilt @default 0.5 @min 0 @max 1)
  (param cyc_hold @default 0 @min 0 @max 1)
  (def env_adsr (adsr gate_in trig env2_attack env2_decay env2_sustain env2_release))
  (def ph (retrig-phasor cyc_rate_hz trig))
  (def hold_f (* (clip cyc_hold 0 1) 0.9))
  (def avail (- 1.0 hold_f))
  (def rise (clip (* avail cyc_tilt) 0.01 (- avail 0.01)))
  (def fall (max 0.01 (- avail rise)))
  (def in_rise (lt ph rise))
  (def in_hold (* (gte ph rise) (lt ph (+ rise hold_f))))
  (def in_fall (gte ph (+ rise hold_f)))
  (def cyc (+ (* in_rise (/ ph rise))
              in_hold
              (* in_fall (clip (- 1.0 (/ (- ph rise hold_f) fall)) 0 1))))
  (gswitch (gte env2_mode 0.5) cyc env_adsr))

; Multi-wave LFO: sine / tri / saw-up / saw-down / square / s&h / wander,
; free-running or note-ratio rate, optional retrigger, scaled by amount.
(defmacro drift-lfo (amount base_hz trig)
  (param lfo_wave @default 0 @min 0 @max 6)
  (param lfo_mode @default 0 @min 0 @max 1)
  (param lfo_rate_hz @default 1.2 @min 0.01 @max 30 @unit Hz)
  (param lfo_ratio @default 1 @min 0.01 @max 4)
  (param lfo_retrig @default 1 @min 0 @max 1)
  (def freq (clip (gswitch (gte lfo_mode 0.5) (* base_hz lfo_ratio) lfo_rate_hz)
                  0.01 (* samplerate 0.45)))
  (def reset (* lfo_retrig trig))
  (def ph (retrig-phasor freq reset))
  (def wrapped (wrap-trigger ph))
  (def sh (latch (noise) (max wrapped reset)))
  (def wander (smooth-toward sh (* freq 8.0)))
  (def raw (selector (+ (clip (round lfo_wave) 0 6) 1)
             (sin (* ph twopi))
             (triangle ph)
             (- (* ph 2) 1)
             (- 1 (* ph 2))
             (scale (lt ph 0.5) 0 1 -1 1)
             sh
             wander))
  (* raw (clip amount 0 1)))

; 3-slot mod matrix summed into the 9 destinations.
; -> (o1_gain o1_shape o2_gain o2_det nz_gain lp_freq lp_res hp_freq volume)
(defmacro mod-matrix (e1 e2 lf ky vl)
  (param mm1_src @default 2 @min 0 @max 4)
  (param mm1_dest @default 0 @min 0 @max 8)
  (param mm1_amt @default 0 @min -1 @max 1)
  (param mm2_src @default 1 @min 0 @max 4)
  (param mm2_dest @default 5 @min 0 @max 8)
  (param mm2_amt @default 0 @min -1 @max 1)
  (param mm3_src @default 4 @min 0 @max 4)
  (param mm3_dest @default 8 @min 0 @max 8)
  (param mm3_amt @default 0 @min -1 @max 1)
  (def v1 (pick-source mm1_src e1 e2 lf ky vl))
  (def v2 (pick-source mm2_src e1 e2 lf ky vl))
  (def v3 (pick-source mm3_src e1 e2 lf ky vl))
  (tuple
    (+ (route-if-dest mm1_dest 0 mm1_amt v1) (route-if-dest mm2_dest 0 mm2_amt v2) (route-if-dest mm3_dest 0 mm3_amt v3))
    (+ (route-if-dest mm1_dest 1 mm1_amt v1) (route-if-dest mm2_dest 1 mm2_amt v2) (route-if-dest mm3_dest 1 mm3_amt v3))
    (+ (route-if-dest mm1_dest 2 mm1_amt v1) (route-if-dest mm2_dest 2 mm2_amt v2) (route-if-dest mm3_dest 2 mm3_amt v3))
    (+ (route-if-dest mm1_dest 3 mm1_amt v1) (route-if-dest mm2_dest 3 mm2_amt v2) (route-if-dest mm3_dest 3 mm3_amt v3))
    (+ (route-if-dest mm1_dest 4 mm1_amt v1) (route-if-dest mm2_dest 4 mm2_amt v2) (route-if-dest mm3_dest 4 mm3_amt v3))
    (+ (route-if-dest mm1_dest 5 mm1_amt v1) (route-if-dest mm2_dest 5 mm2_amt v2) (route-if-dest mm3_dest 5 mm3_amt v3))
    (+ (route-if-dest mm1_dest 6 mm1_amt v1) (route-if-dest mm2_dest 6 mm2_amt v2) (route-if-dest mm3_dest 6 mm3_amt v3))
    (+ (route-if-dest mm1_dest 7 mm1_amt v1) (route-if-dest mm2_dest 7 mm2_amt v2) (route-if-dest mm3_dest 7 mm3_amt v3))
    (+ (route-if-dest mm1_dest 8 mm1_amt v1) (route-if-dest mm2_dest 8 mm2_amt v2) (route-if-dest mm3_dest 8 mm3_amt v3))))

; Oscillator frequencies from pitch mod, drift, octave/detune. -> (f1 f2)
(defmacro osc-frequencies (base_hz e1 e2 lf ky vl
                           drift_cents o2_extra_cents detune mm_det)
  (param pitch_mod1_src @default 2 @min 0 @max 4)
  (param pitch_mod1_amt @default 0 @min -24 @max 24 @unit st)
  (param pitch_mod2_src @default 0 @min 0 @max 4)
  (param pitch_mod2_amt @default 0 @min -24 @max 24 @unit st)
  (param osc1_octave @default 0 @min -3 @max 3 @unit oct)
  (param osc2_octave @default -1 @min -3 @max 3 @unit oct)
  (def pm1 (pick-source pitch_mod1_src e1 e2 lf ky vl))
  (def pm2 (pick-source pitch_mod2_src e1 e2 lf ky vl))
  (def mod_semis (+ (* pm1 pitch_mod1_amt) (* pm2 pitch_mod2_amt)))
  (def common (* base_hz (semi-ratio mod_semis)
                 (semi-ratio (/ drift_cents 100.0))))
  (tuple (* common (semi-ratio (* (clip (round osc1_octave) -3 3) 12)))
         (* common (semi-ratio (+ (* (clip (round osc2_octave) -3 3) 12)
                                  (clip (+ detune (* mm_det 12)) -24 24)
                                  (/ o2_extra_cents 100.0))))))

; Morphing oscillator: wave selects sine / asym-tri / shark / saturated /
; saw / pulse / rect; shape (base + mod source * amt + matrix) morphs within
; the selected wave.
(defmacro morph-osc (freq shape_base e1 e2 lf ky vl mm_shape)
  (param osc1_wave @default 4 @min 0 @max 6)
  (param osc1_shape_src @default 2 @min 0 @max 4)
  (param osc1_shape_amt @default 0 @min -1 @max 1)
  (def shp_val (pick-source osc1_shape_src e1 e2 lf ky vl))
  (def shape (clip (+ shape_base (* shp_val osc1_shape_amt) mm_shape) 0 1))
  (def ph (phasor freq))
  (def o_sine (sin (* ph twopi)))
  (def tri_peak (clip (+ 0.05 (* shape 0.9)) 0.05 0.95))
  (def o_tri_asym (gswitch (lt ph tri_peak)
                    (- (* (/ ph tri_peak) 2) 1)
                    (- (* (/ (- 1 ph) (- 1 tri_peak)) 2) 1)))
  (def o_saw_raw (polyblep_saw ph freq))
  (def o_shark (+ (* (- 1 shape) o_saw_raw) (* shape o_tri_asym)))
  (def sat_drive (+ 1.0 (* shape 5.0)))
  (def o_sat (/ (tanh (* o_saw_raw sat_drive)) (tanh sat_drive)))
  (def saw_drive (+ 1.0 (* shape 1.5)))
  (def o_saw (/ (tanh (* o_saw_raw saw_drive)) (tanh saw_drive)))
  (def pw (clip (+ 0.05 (* shape 0.9)) 0.05 0.95))
  (def o_pulse (polyblep_pulse ph pw freq))
  (def rect_w (clip (+ 0.5 (* (- shape 0.5) 0.6)) 0.2 0.8))
  (def o_rect (polyblep_pulse ph rect_w freq))
  (selector (+ (clip (round osc1_wave) 0 6) 1)
    o_sine o_tri_asym o_shark o_sat o_saw o_pulse o_rect))

; Simple oscillator: sine / tri / saturated saw / saw / square.
(defmacro basic-osc (freq)
  (param osc2_wave @default 3 @min 0 @max 4)
  (def ph (phasor freq))
  (def o_saw_raw (polyblep_saw ph freq))
  (def o_sat (/ (tanh (* o_saw_raw 3.0)) (tanh 3.0)))
  (selector (+ (clip (round osc2_wave) 0 4) 1)
    (sin (* ph twopi))
    (triangle ph)
    o_sat
    o_saw_raw
    (polyblep_pulse ph 0.5 freq)))

; Mixer: on/off + dB gain staging (with matrix offsets) and per-source
; routing into the filter or around it. Native oscillator-unit scaling is .4.
; -> (to_filter dry)
(defmacro source-mixer (o1 o2 gain1_db gain2_db nz_db mm_g1 mm_g2 mm_nz)
  (param osc1_on @default 1 @min 0 @max 1)
  (param osc2_on @default 1 @min 0 @max 1)
  (param osc1_route @default 1 @min 0 @max 1)
  (param osc2_route @default 1 @min 0 @max 1)
  (param noise_route @default 1 @min 0 @max 1)
  (def g1 (* (gte osc1_on 0.5) (db-amp (clip (+ gain1_db (* mm_g1 24)) -36 12))))
  (def g2 (* (gte osc2_on 0.5) (db-amp (clip (+ gain2_db (* mm_g2 24)) -36 12))))
  (def nz_db_c (clip (+ nz_db (* mm_nz 24)) -60 12))
  (def gn (* (gt nz_db_c -59.5) (db-amp nz_db_c)))
  (def s1 (* .4 o1 g1))
  (def s2 (* .4 o2 g2))
  (def sn (* .4 (noise) gn))
  (def r1 (gte osc1_route 0.5))
  (def r2 (gte osc2_route 0.5))
  (def rn (gte noise_route 0.5))
  (tuple (+ (* s1 r1) (* s2 r2) (* sn rn))
         (+ (* s1 (- 1 r1)) (* s2 (- 1 r2)) (* sn (- 1 rn)))))

; Keytracked/modulated cutoff, oscillator-level-driven low-pass, then the
; measured resonant high-pass. Coefficients use the host sample rate.
(defmacro drift-filter (x base_hz e1 e2 lf ky vl
                        freq_hz res_base hp_hz
                        mm_freq mm_res mm_hp drift_oct)
  (param filter_type @default 0 @min 0 @max 1)
  (param keytrack @default 0.3 @min 0 @max 1)
  (param lp_mod1_src @default 0 @min 0 @max 4)
  (param lp_mod1_amt @default 1.5 @min -8 @max 8 @unit oct)
  (param lp_mod2_src @default 2 @min 0 @max 4)
  (param lp_mod2_amt @default 0 @min -8 @max 8 @unit oct)
  (def fm1 (pick-source lp_mod1_src e1 e2 lf ky vl))
  (def fm2 (pick-source lp_mod2_src e1 e2 lf ky vl))
  (def oct_mod (+ (* fm1 lp_mod1_amt) (* fm2 lp_mod2_amt)
                  (* mm_freq 6) drift_oct))
  (def cut (clip (* freq_hz
                    (pow (/ (max base_hz 8.0) 261.63) keytrack)
                    (semi-ratio (* oct_mod 12)))
                 20 (min 16000 (* samplerate .36))))
  (def hp_cut (clip (* hp_hz (semi-ratio (* mm_hp 72)))
                    20 (min 12000 (* samplerate .36))))
  (def res (clip (+ res_base mm_res) 0 1))
  (def lp_i (drift-type1 x cut res))
  (def lp_ii (drift-type2 x cut res))
  (def lowpass (gswitch (gte filter_type 0.5) lp_ii lp_i))
  (svf lowpass hp_cut 1.469 2))

; Amp envelope, velocity, volume (with matrix offset), per-voice pan spread,
; pre-envelope summing saturation, then linear stereo gain. -> (left right)
(defmacro output-stage (filt dry env vel vol_db mm_vol pan_rnd)
  (param vel_to_vol @default 0.35 @min 0 @max 1)
  (param voice_pan @default 0 @min -1 @max 1)
  (param spread @default 0.2 @min 0 @max 1)
  (def vel_gain (+ (- 1 vel_to_vol) (* vel vel_to_vol)))
  (def vol (db-amp (clip (+ vol_db (* mm_vol 24)) -36 6)))
  (def amp (* (clip (+ filt dry) -.875 .875) env vel_gain vol))
  (def pan (clip (+ voice_pan (* pan_rnd spread)) -1 1))
  (tuple (* amp (clip (- 1 (* pan 0.5)) 0 1.5))
         (* amp (clip (+ 1 (* pan 0.5)) 0 1.5))))

; ======================================================================
; voice: section nodes only
; ======================================================================

(def base_pitch (glide-to pitch))
(def key_val (key-follow base_pitch))

(def (drift_cents o2_drift_cents drift_filt_oct pan_rnd)
     (analog-drift (mod drift) trigger))

(def env1 (amp-env gate trigger))
(def env2 (env2-select gate trigger))
(def lfo (drift-lfo (mod lfo_amount) base_pitch trigger))

(def (mm_o1_gain mm_o1_shape mm_o2_gain mm_o2_det mm_nz_gain
      mm_lp_freq mm_lp_res mm_hp_freq mm_volume)
     (mod-matrix env1 env2 lfo key_val velocity))

(def (f1 f2) (osc-frequencies base_pitch env1 env2 lfo key_val velocity
                              drift_cents o2_drift_cents (mod osc2_detune)
                              mm_o2_det))

(def osc1 (morph-osc f1 (mod osc1_shape) env1 env2 lfo key_val velocity
                     mm_o1_shape))
(def osc2 (basic-osc f2))

(def (to_filter dry)
     (source-mixer osc1 osc2
                   (mod osc1_gain_db) (mod osc2_gain_db) (mod noise_gain_db)
                   mm_o1_gain mm_o2_gain mm_nz_gain))

(def lp_out (drift-filter to_filter base_pitch env1 env2 lfo key_val velocity
                          (mod lp_freq) (mod lp_res)
                          (mod hp_freq)
                          mm_lp_freq mm_lp_res mm_hp_freq drift_filt_oct))

(def (left right) (output-stage lp_out dry env1 velocity
                                (mod volume_db) mm_volume pan_rnd))

(out left 1 @name left)
(out right 2 @name right)
