; Waveguide flute — stable-pitch core with a parametric "free jazz" axis.
;
; STK-style jet + bore loop (Cook slide-flute topology): breath pressure minus
; reflected bore signal drives a jet delay and cubic jet nonlinearity, which is
; injected back into the bore delay. Two fixes over the old stable-flute-v3:
;   1. The bore reflection lowpass's phase delay is computed analytically at
;      the target frequency and subtracted from the bore delay length, so
;      brightness no longer detunes the instrument.
;   2. A keytracked band-pass "lock" resonator sits in the loop (zero phase at
;      resonance) and pulls the oscillation onto the target pitch.
; The free-jazz axis (chaos / overblow / growl / flutter) perturbs pressure,
; embouchure, and pitch around that stable center, so it gets wild without
; losing the note.

(def gate     (in 1 @name gate))
(def pitch    (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger  (in 4 @name trigger))
(def clock    (in 5 @name clock))
(def mod1     (in 6 @name mod1 @modulator 1))
(def mod2     (in 7 @name mod2 @modulator 2))
(def mod3     (in 8 @name mod3 @modulator 3))
(def mod4     (in 9 @name mod4 @modulator 4))

(param attack       @default 35   @min 1    @max 500  @unit ms)
(param release      @default 180  @min 5    @max 2000 @unit ms)
(param pressure     @default 0.72 @min 0.2  @max 1.4  @mod true @mod-mode additive)
(param breath       @default 0.25 @min 0    @max 1    @mod true @mod-mode additive)
(param chiff        @default 0.35 @min 0    @max 1)
(param vib_rate     @default 4.8  @min 0.1  @max 12   @unit Hz)
(param vib_depth    @default 8    @min 0    @max 60   @unit ct @mod true @mod-mode additive)
(param jet_ratio    @default 0.5  @min 0.2  @max 0.8)
(param brightness   @default 0.55 @min 0    @max 1    @mod true @mod-mode additive)
(param lock         @default 0.5  @min 0    @max 1)
(param refl         @default 0.55 @min 0.3  @max 0.8)
(param chaos        @default 0    @min 0    @max 1    @mod true @mod-mode additive)
(param chaos_rate   @default 3.5  @min 0.1  @max 25   @unit Hz)
(param overblow     @default 0    @min 0    @max 1    @mod true @mod-mode additive)
(param growl        @default 0    @min 0    @max 1    @mod true @mod-mode additive)
(param growl_ratio  @default 1.5  @min 0.25 @max 3)
(param flutter      @default 0    @min 0    @max 1    @mod true @mod-mode additive)
(param tune         @default 0    @min -100 @max 100  @unit ct)
(param vel_to_press @default 0.5  @min 0    @max 1)
(param gain         @default 0.4  @min 0    @max 1)

(defmacro cents_ratio (c)
  (exp (* 0.0005776226505 c)))

; Smoothed sample-and-hold noise in roughly [-1,1]: new random target each
; cycle of `rate`, slewed so the wander is continuous.
(defmacro sh_wander (rate)
  (make-history ph_h)
  (def ph_prev (read-history ph_h))
  (def ph (wrap (+ ph_prev (/ rate samplerate)) 0 1))
  (def ph_wrapped (lt ph ph_prev))
  (write-history ph_h ph)
  (make-history tgt_h)
  (def tgt_prev (read-history tgt_h))
  (def tgt (gswitch (gt ph_wrapped 0.5) (noise) tgt_prev))
  (write-history tgt_h tgt)
  (make-history sm_h)
  (def sm_prev (read-history sm_h))
  (def sm_c (clip (* 6.0 (/ rate samplerate)) 0.000001 0.5))
  (def sm (+ sm_prev (* sm_c (- tgt sm_prev))))
  (write-history sm_h sm)
  sm)

; ── Breath envelope ──
(make-history h_env)
(def env_prev (read-history h_env))
(def att_c (exp (/ -1.0 (max 1.0 (* attack samplerate 0.001)))))
(def rel_c (exp (/ -1.0 (max 1.0 (* release samplerate 0.001)))))
(def env (mix gate env_prev (gswitch (gt gate env_prev) att_c rel_c)))
(write-history h_env env)

; ── Attack chiff: noise burst latched on trigger, ~25 ms decay ──
(make-history h_chiff)
(def chiff_prev (read-history h_chiff))
(def chiff_dec (exp (/ -1.0 (* 0.025 samplerate))))
(def chiff_env (gswitch (gt trigger 0.5) 1.0 (* chiff_prev chiff_dec)))
(write-history h_chiff chiff_env)

; ── Chaos wander sources ──
(def chaos_amt (clip (mod chaos) 0 1))
(def wander_pitch (sh_wander chaos_rate))
(def wander_emb   (sh_wander (* chaos_rate 1.7)))
(def wander_press (sh_wander (* chaos_rate 0.61)))

; ── Pitch ──
(def vib (* (sin (* twopi (phasor vib_rate))) (max 0.0 (mod vib_depth))))
(def pitch_hz (clip (* (max 30.0 pitch)
                       (cents_ratio (+ tune vib (* chaos_amt wander_pitch 70.0))))
                    30 4000))
(def period (/ samplerate pitch_hz))

; ── Bore reflection lowpass coefficient + its phase delay at pitch_hz ──
(def bright (clip (mod brightness) 0 1))
(def lp_c (- 0.93 (* bright 0.88)))
(def ob (clip (mod overblow) 0 1))
; evaluate loop phase corrections at the intended sounding frequency: the
; fundamental normally, sliding toward the second register under overblow
(def w0 (* twopi (/ pitch_hz samplerate)))
(def w_eff (* w0 (+ 1.0 ob)))
(def lp_del (/ (atan2 (* lp_c (sin w_eff)) (- 1.0 (* lp_c (cos w_eff)))) w_eff))
; loop = bore delay + lowpass phase delay + 1 sample feedback history;
; the DC blocker's phase lead shortens the loop by ~c/w^2 samples
; (constant fitted against harness f0 measurements across c3..c6)
(def dc_comp (/ 0.004868 (* w_eff w_eff)))
; small linear-in-f residual (delay interpolation phase error at short delays)
(def hi_comp (- (* pitch_hz 0.000328) 0.19))
(def bore_len (max 2.0 (+ (- period lp_del 1.326) dc_comp hi_comp)))

; ── Embouchure / overblow ──
(def jr (clip (+ jet_ratio (* ob -0.22) (* chaos_amt wander_emb 0.18)) 0.12 0.88))

; ── Breath pressure signal ──
(def press_base (clip (mod pressure) 0 1.6))
(def vel_g (+ (- 1.0 vel_to_press) (* velocity vel_to_press)))
(def flut (clip (mod flutter) 0 1))
(def flut_lfo (* 0.5 (+ 1.0 (sin (* twopi (phasor 27.0))))))
(def flut_g (- 1.0 (* flut 0.45 flut_lfo)))
(def growl_amt (clip (mod growl) 0 1))
(def growl_lfo (sin (* twopi (phasor (clip (* pitch_hz growl_ratio) 20 8000)))))
(def growl_sig (* growl_amt 0.2 growl_lfo))
(def breath_amt (clip (mod breath) 0 1))
(make-history h_bnoise)
(def bn_prev (read-history h_bnoise))
(def bnoise (+ bn_prev (* 0.22 (- (noise) bn_prev))))
(write-history h_bnoise bnoise)
(def press (* env vel_g flut_g
              (+ (* press_base
                    (+ 1.0 (* ob 0.5))
                    (+ 1.0 (* chaos_amt wander_press 0.5)))
                 (* bnoise breath_amt 0.35)
                 (* (noise) chiff chiff_env 0.6)
                 growl_sig)))

; ── Bore loop ──
(make-history h_bore)
(def bore_prev (read-history h_bore))

; reflection lowpass
(make-history h_lp)
(def lp_prev (read-history h_lp))
(def lp_out (+ lp_prev (* (- 1.0 lp_c) (- bore_prev lp_prev))))
(write-history h_lp lp_out)

; DC block
(make-history h_dc)
(def dc_prev (read-history h_dc))
(def dc_lp (+ dc_prev (* 0.005 (- lp_out dc_prev))))
(write-history h_dc dc_lp)
(def refl_sig (- lp_out dc_lp))

; keytracked band-pass lock: zero phase at resonance, attenuates off-pitch
; energy, so it pulls the oscillation onto pitch_hz without detuning it
(def lockg (clip lock 0 1))
; overblow slides the lock resonance toward the second register, helping the
; oscillation crack up the octave under high drive
(def lock_hz (clip (* pitch_hz (+ 1.0 ob)) 30 8000))
; the svf bp output has gain Q at resonance; scale by 1/Q for unity so the
; lock is an equal-gain crossfade — loop gain at pitch stays refl_g (< 1,
; so notes always decay), locking comes purely from off-pitch attenuation
(def bp (* (svf refl_sig lock_hz 6.0 1) 0.1666667))
; keep a small lock floor: with zero band-pass mix the hot jet pickup can
; latch onto the bore's 3rd mode instead of the fundamental
(def lock_m (+ 0.12 (* lockg 0.73)))
(def refl_lock (+ (* refl_sig (- 1.0 lock_m)) (* bp lock_m)))

; jet: pressure difference -> jet delay -> cubic nonlinearity.
; The reflected pickup is gated by the breath envelope: with no airflow the
; jet cannot amplify, so the loop gain drops below unity and the note stops
; (without this the jet + reflection paths sum past 1 and self-oscillate).
; 0.9 reflected pickup + 1.2 jet injection (below): harness-tuned operating
; point where the jet nonlinearity stays smooth — weaker pickup makes the
; bore wave square off (crest ~1.0, buzzy) at every pressure level
(def press_diff (- press (* 0.9 refl_lock)))
(def jet_delayed (delay press_diff (max 1.0 (* period jr))))
; keep drive increments small: past |x|=1 the input clip makes the cubic
; die to zero, so heavy drive chokes the tone instead of thickening it
; (tanh input soft-limits and raw STK output clips both wreck the clean
; default operating point — measured, don't revisit)
(def jet_drive (+ 1.0 (* chaos_amt 0.4) (* ob 0.25)))
(def jx (clip (* jet_delayed jet_drive) -1.0 1.0))
(def jet (* jx (* env (- (* jx jx) 1.0))))

(def refl_g (clip refl 0.3 0.8))
(def bore_in (tanh (+ (* jet 1.2) (* refl_g refl_lock))))
(def bore_out (delay bore_in bore_len))
(write-history h_bore bore_out)

; ── Output ──
; growl: voiced hum amplitude-modulates the tone (inharmonic sidebands the
; bore comb would otherwise filter out); flutter-tongue chops the output too
(def growl_am (+ 1.0 (* growl_amt 0.5 growl_lfo)))
; the overblown register carries much less loop energy; make up for it
(def ob_makeup (+ 1.0 (* ob 1.2)))
(def out_sig (* (tanh bore_out) growl_am flut_g ob_makeup
                0.7 (+ 0.4 (* 0.6 velocity)) gain))
(out out_sig 1 @name audio)
