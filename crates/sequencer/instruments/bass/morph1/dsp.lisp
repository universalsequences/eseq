; morph1 — korg1-style dual-VCO bass engine into a Vital-style morphing filter.
; The filter is ONE ZDF state-variable core whose LP/BP/HP/notch taps are all
; computed every sample; "filter type" is just a continuous blend of tap
; weights (the Vital trick), so morph is fully modulatable at audio rate.

; Vital-style morphing ZDF SVF.
; morph 0..1 sweeps LP -> mid -> HP; character 0..1 picks the mid tap
; (0 = unity-gain bandpass, like Vital's 12dB blend; 1 = notch, like the
; 24dB blend). q is the raw SVF resonance (0.5 = none, higher = more).
(defmacro morph-svf (input cutoff q morph character)
  (def safe_cutoff (clip cutoff 1.0 (* samplerate 0.49)))
  (def safe_q (max q 0.001))
  (def g (tan (* pi (/ safe_cutoff samplerate))))
  (def k (/ 1.0 safe_q))
  (def a1 (/ 1.0 (+ 1.0 (* g (+ g k)))))
  (def a2 (* g a1))
  (def a3 (* g a2))

  (make-history ic1eq)
  (make-history ic2eq)

  (def ic1 (read-history ic1eq))
  (def ic2 (read-history ic2eq))
  (def v3 (- input ic2))
  (def v1 (+ (* a1 ic1) (* a2 v3)))
  (def v2 (+ ic2 (* a2 ic1) (* a3 v3)))

  (write-history ic1eq (- (* 2.0 v1) ic1))
  (write-history ic2eq (- (* 2.0 v2) ic2))

  (def lp v2)
  (def hp (- input (* k v1) v2))
  (def bp_unity (* k v1))
  (def notch (+ hp lp))

  (def m (clip morph 0 1))
  (def w_lp (clip (- 1.0 (* 2.0 m)) 0 1))
  (def w_hp (clip (- (* 2.0 m) 1.0) 0 1))
  (def w_mid (- 1.0 w_lp w_hp))
  (def c (clip character 0 1))
  (def mid (+ (* (- 1.0 c) bp_unity) (* c notch)))

  (+ (* w_lp lp) (* w_mid mid) (* w_hp hp)))

; Vowel formant growl bank: 3 parallel high-Q unity-gain bandpasses whose
; center frequencies and amplitudes interpolate through a 5-vowel table
; (A E I O U) as vowel_pos sweeps 0..1. scale shifts the whole vowel
; (throat size); this is the classic jungle/garage talking-bass filter.
(defmacro formant-growl (input scale q vowel_pos)
  (def vp (* (clip vowel_pos 0 1) 4.0))
  (def w0 (clip (- 1.0 (abs (- vp 0.0))) 0 1))
  (def w1 (clip (- 1.0 (abs (- vp 1.0))) 0 1))
  (def w2 (clip (- 1.0 (abs (- vp 2.0))) 0 1))
  (def w3 (clip (- 1.0 (abs (- vp 3.0))) 0 1))
  (def w4 (clip (- 1.0 (abs (- vp 4.0))) 0 1))
  (def f1 (* scale (+ (* w0 600) (* w1 400) (* w2 250) (* w3 400) (* w4 350))))
  (def f2 (* scale (+ (* w0 1040) (* w1 1620) (* w2 1750) (* w3 750) (* w4 600))))
  (def f3 (* scale (+ (* w0 2250) (* w1 2400) (* w2 2600) (* w3 2400) (* w4 2400))))
  (def a2 (+ (* w0 0.45) (* w1 0.60) (* w2 0.55) (* w3 0.28) (* w4 0.22)))
  (def a3 (+ (* w0 0.25) (* w1 0.32) (* w2 0.30) (* w3 0.15) (* w4 0.12)))
  ; svf bp tap has center gain q; divide back out for unity-gain formants.
  (def inv_q (/ 1.0 (max q 0.001)))
  (def b1 (* (svf input f1 q 1) inv_q))
  (def b2 (* (svf input f2 q 1) inv_q))
  (def b3 (* (svf input f3 q 1) inv_q))
  (+ b1 (* a2 b2) (* a3 b3)))

; Oberheim Xpander-style pole-mixing morph on the ZDF Moog ladder: one 4-pole
; core, filter shape = a weight vector over (filter input, pole outputs
; y1..y4). morph 0..1 sweeps LP -> BP -> HP; family 0..1 crossfades the
; 2-pole coefficient set into the 4-pole set (12dB <-> 24dB).
(defmacro ladder-morph (input cutoff res drive morph family)
  (def wd (* twopi (clip cutoff 10 16000)))
  (def T (/ 1 samplerate))
  (def wa (* (/ 2.0 T) (tan (* wd T 0.5))))
  (def g (* wa T 0.5))
  (def G (/ g (+ 1 g)))
  (def G4 (* G G G G))
  (def k (* res 4))
  (def fb_trim 0.5)

  (make-history z1)
  (make-history z2)
  (make-history z3)
  (make-history z4)

  (def hz1 (read-history z1))
  (def hz2 (read-history z2))
  (def hz3 (read-history z3))
  (def hz4 (read-history z4))
  (def inv_1pg (/ 1 (+ 1 g)))
  (def S (+ (* hz1 G G G inv_1pg)
            (* hz2 G G inv_1pg)
            (* hz3 G inv_1pg)
            (* hz4 inv_1pg)))

  (def driven_input (tanh (* drive input)))
  (def u (/ (- driven_input (* k fb_trim S))
            (+ 1 (* k fb_trim G4))))
  (def x1 (- u (* k (tanh (* fb_trim (+ (* G4 u) S))))))

  (def v1 (* (- x1 hz1) G))
  (def y1 (+ v1 hz1))
  (write-history z1 (+ y1 v1))
  (def v2 (* (- y1 hz2) G))
  (def y2 (+ v2 hz2))
  (write-history z2 (+ y2 v2))
  (def v3 (* (- y2 hz3) G))
  (def y3 (+ v3 hz3))
  (write-history z3 (+ y3 v3))
  (def v4 (* (- y3 hz4) G))
  (def y4 (+ v4 hz4))
  (write-history z4 (+ y4 v4))

  ; Pole-mix tables: LP2 [0 0 1 0 0]  BP2 [0 2 -2 0 0]  HP2 [1 -2 1 0 0]
  ;                  LP4 [0 0 0 0 1]  BP4 [0 0 4 -8 4]  HP4 [1 -4 6 -4 1]
  (def m (clip morph 0 1))
  (def w_lp (clip (- 1.0 (* 2.0 m)) 0 1))
  (def w_hp (clip (- (* 2.0 m) 1.0) 0 1))
  (def w_bp (- 1.0 w_lp w_hp))
  (def fam (clip family 0 1))
  (def c0 w_hp)
  (def c1 (+ (* w_bp (mix 2 0 fam)) (* w_hp (mix -2 -4 fam))))
  (def c2 (+ (* w_lp (mix 1 0 fam)) (* w_bp (mix -2 4 fam)) (* w_hp (mix 1 6 fam))))
  (def c3 (+ (* w_bp (* fam -8)) (* w_hp (* fam -4))))
  (def c4 (+ (* w_lp fam) (* w_bp (* fam 4)) (* w_hp fam)))
  (+ (* c0 x1) (* c1 y1) (* c2 y2) (* c3 y3) (* c4 y4)))

; Tuned feedback comb (Vital's Comb type). freq sets the harmonic series
; spacing, feedback comes from resonance, polarity_morph 0..1 crossfades
; comb+ (peaks on the harmonics of freq) into comb- (peaks between them,
; hollow flange), damp_cutoff darkens the loop like tape.
(defmacro comb-morph (input freq feedback polarity_morph damp_cutoff)
  (def safe_freq (clip freq 25 4000))
  ; -1 sample: the feedback history read below costs one sample of loop time.
  (def dsamps (clip (- (/ samplerate safe_freq) 1.0) 1 4700))
  (make-history fb_h)
  (def prev (read-history fb_h))
  (def pol (- 1.0 (* 2.0 (clip polarity_morph 0 1))))
  (def fb (clip feedback 0 0.98))
  (def wet (delay (+ input (* fb pol prev)) dsamps @max-delay 4800))
  (def damped (svf wet (clip damp_cutoff 200 16000) 0.55 0))
  (write-history fb_h damped)
  (* 0.6 (+ input (* pol damped))))

; One first-order allpass stage: H(z) = (g + z^-1) / (1 + g z^-1).
(defmacro allpass1 (x g)
  (make-history x1h)
  (make-history y1h)
  (def xp (read-history x1h))
  (def yp (read-history y1h))
  (def y (+ (* g x) xp (* -1.0 g yp)))
  (write-history x1h x)
  (write-history y1h y)
  y)

; Allpass coefficient for a first-order stage breaking at fc.
(defmacro def_ap_g (fc)
  (def t (tan (* pi (/ fc samplerate))))
  (/ (- t 1.0) (+ t 1.0)))

; Phaser-as-filter (Vital's Phase type): 4 cascaded allpasses with feedback.
; center sets where the notch cluster sits, spread fans the stage
; frequencies geometrically apart, feedback sharpens peaks between notches,
; polarity_morph 0..1 crossfades the notch pattern into its complement.
(defmacro phaser-morph (input center feedback polarity_morph spread)
  (def sp (+ 1.0 (* (clip spread 0 1) 1.4)))
  (def f2c (clip center 40 12000))
  (def f1c (clip (/ f2c sp) 40 12000))
  (def f3c (clip (* f2c sp) 40 14000))
  (def f4c (clip (* f3c sp) 40 15000))
  (def g1 (def_ap_g f1c))
  (def g2 (def_ap_g f2c))
  (def g3 (def_ap_g f3c))
  (def g4 (def_ap_g f4c))
  (make-history wet_h)
  (def prev (read-history wet_h))
  (def fb (clip feedback 0 0.9))
  (def s0 (+ input (* fb prev)))
  (def s1 (allpass1 s0 g1))
  (def s2 (allpass1 s1 g2))
  (def s3 (allpass1 s2 g3))
  (def s4 (allpass1 s3 g4))
  (write-history wet_h s4)
  (def pol (- 1.0 (* 2.0 (clip polarity_morph 0 1))))
  (* 0.5 (+ input (* pol s4))))

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))

(param amp_attack @default 2 @min 1 @max 1000 @unit ms)
(param amp_decay @default 160 @min 1 @max 2500 @unit ms)
(param amp_sustain @default 0.70 @min 0 @max 1)
(param amp_release @default 90 @min 1 @max 4000 @unit ms)

(param filt_attack @default 1 @min 1 @max 1000 @unit ms)
(param filt_decay @default 210 @min 1 @max 3000 @unit ms)
(param filt_sustain @default 0.10 @min 0 @max 1)
(param filt_release @default 120 @min 1 @max 4000 @unit ms)

; Shared envelope curve exponents (adsrexp): <1 concave, 1 linear, >1 convex.
; The analog default is a concave attack with a convex decay/release.
(param env_attack_curve @default 0.5 @min 0.1 @max 4)
(param env_fall_curve @default 2.6 @min 0.25 @max 6)

; 0 = morph SVF, 1 = pole-mix ladder, 2 = comb, 3 = phaser.
(param filter_type @default 0 @min 0 @max 3)

(param vco1_saw @default 0.85 @min 0 @max 1)
(param vco1_pulse @default 0.30 @min 0 @max 1)
(param vco2_level @default 0.75 @min 0 @max 1)
(param vco2_interval @default 0 @min -24 @max 24)
(param vco2_fine @default 12 @min -50 @max 50)
(param sub_level @default 0.30 @min 0 @max 1)
(param noise_level @default 0.02 @min 0 @max 1)

(param pulse_width @default 0.50 @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param pwm_amount @default 0.08 @min 0 @max 0.45)
(param pitch_env_amount @default 0 @min -2400 @max 2400)
(param analog_drift @default 3.0 @min 0 @max 35)

(param cutoff @default 950 @min 30 @max 18000 @unit Hz @mod true @mod-mode additive)
(param resonance @default 0.45 @min 0 @max 1 @mod true @mod-mode additive)
(param morph @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param character @default 0 @min 0 @max 1)
(param slope @default 0.65 @min 0 @max 1)
(param vowel_mix @default 0.7 @min 0 @max 1)
(param growl_drive @default 2.5 @min 0.5 @max 8)
(param filter_env_amount @default 2800 @min -9000 @max 9000)
(param morph_env_amount @default 0 @min -1 @max 1)
(param keytrack @default 0.30 @min 0 @max 4)
(param filter_drive @default 2.0 @min 0.5 @max 10)

(param lfo_rate @default 5.5 @min 0.05 @max 40 @unit Hz)
(param lfo_filter_amount @default 0 @min -5000 @max 5000)
(param lfo_morph @default 0 @min -1 @max 1)
(param lfo_pitch @default 0 @min -1200 @max 1200)

(param input_drive @default 2.0 @min 0.5 @max 10)
(param output_bite @default 1.3 @min 0.5 @max 6)
(param gain @default 0.45 @min 0 @max 1)

(def amp_env (adsrexp gate trigger amp_attack amp_decay amp_sustain amp_release
                      env_attack_curve env_fall_curve))
(def filt_env (adsrexp gate trigger filt_attack filt_decay filt_sustain filt_release
                       env_attack_curve env_fall_curve))

(def lfo_phase (phasor lfo_rate))
(def lfo (sin (* lfo_phase twopi)))

(def drift_a (sin (* (phasor 0.137) twopi)))
(def drift_b (sin (* (phasor 0.211) twopi)))
(def drift_c (sin (* (phasor 0.073) twopi)))

(def pitch_snap (* filt_env pitch_env_amount))
(def vib (* lfo lfo_pitch))
(def cents1 (+ pitch_snap vib (* drift_a analog_drift)))
(def cents2 (+ pitch_snap vib vco2_fine (* drift_b analog_drift -1.35)))
(def cents_sub (* drift_c analog_drift 0.35))

(def freq1 (* pitch (pow 2 (/ cents1 1200))))
(def freq2 (* pitch (pow 2 (/ (+ (* vco2_interval 100) cents2) 1200))))
(def freq_sub (* pitch 0.5 (pow 2 (/ cents_sub 1200))))

(def phase1 (phasor freq1))
(def phase2 (phasor freq2))
(def phase_sub (phasor freq_sub))

(def pw (clip (+ (mod pulse_width) (* lfo pwm_amount) (* drift_c 0.012)) 0.05 0.95))

(def saw1 (polyblep_saw phase1 freq1))
(def pulse1 (polyblep_pulse phase1 pw freq1))
(def saw2 (polyblep_saw phase2 freq2))
(def pulse2 (polyblep_pulse phase2 (- 1 pw) freq2))
(def sub (polyblep_pulse phase_sub 0.5 freq_sub))
(def hiss (noise))

(def vco1 (+ (* vco1_saw saw1) (* vco1_pulse pulse1)))
(def vco2 (* vco2_level (+ (* 0.62 saw2) (* 0.38 pulse2))))
(def raw_mix (+ vco1 vco2 (* sub_level sub) (* noise_level hiss)))
(def pre_drive (tanh (* raw_mix input_drive 0.62)))

(def cut (clip (+ (mod cutoff)
                  (* filter_env_amount filt_env velocity)
                  (* lfo lfo_filter_amount)
                  (* keytrack pitch))
               30 18000))
(def q (+ 0.62 (* (clip (mod resonance) 0 1) 5.2)))
(def morph_pos (clip (+ (mod morph)
                        (* morph_env_amount filt_env)
                        (* lfo lfo_morph))
                     0 1))

; Two cascaded morph cores; slope crossfades 12dB -> 24dB. The interstage
; tanh is level-compensated so filter_drive changes color, not volume.
(def drive_comp (/ 1.0 (tanh filter_drive)))
(def stage1 (morph-svf pre_drive cut q morph_pos character))
(def stage1_sat (* (tanh (* stage1 filter_drive 0.72)) drive_comp))
(def stage2 (morph-svf stage1_sat cut q morph_pos character))
(def svf_path (+ (* (- 1.0 slope) stage1) (* slope stage2)))

; Alternate clean-path engines. All reuse the same macro knobs: morph is the
; shape sweep, resonance the intensity, character the engine's flavor axis
; (SVF mid-tap / comb damping / phaser spread), slope the pole family.
(def ladder_path (ladder-morph pre_drive cut (clip (mod resonance) 0 1)
                               filter_drive morph_pos slope))
; DGen bug workaround: a delay-op time input whose dependency cone contains
; an adsr/adsrexp corrupts that envelope's history state (its release snaps
; to zero). Keep filt_env out of the comb frequency; keytrack and LFO are
; safe and are what a tuned comb wants anyway.
(def cut_env_free (clip (+ (mod cutoff) (* lfo lfo_filter_amount) (* keytrack pitch)) 30 18000))
(def comb_freq (clip (* cut_env_free 0.25) 25 4000))
(def comb_fb (* (clip (mod resonance) 0 1) 0.97))
(def comb_damp (mix 9500 1400 character))
(def comb_path (comb-morph pre_drive comb_freq comb_fb morph_pos comb_damp))
(def phaser_fb (* (clip (mod resonance) 0 1) 0.88))
(def phaser_path (phaser-morph pre_drive cut phaser_fb morph_pos character))

(def clean_path (+ (* (eq filter_type 0) svf_path)
                   (* (eq filter_type 1) ladder_path)
                   (* (eq filter_type 2) comb_path)
                   (* (eq filter_type 3) phaser_path)))

; Vowel path: morph sweeps A->E->I->O->U; the env/LFO-swept cutoff shifts
; the whole vowel (the "wow"), resonance sets formant Q, growl_drive
; saturates into the bank so the formants have harmonics to chew on.
(def formant_scale (clip (* cut 0.001) 0.3 4.0))
(def formant_q (+ 4.0 (* (clip (mod resonance) 0 1) 14.0)))
(def growl_in (tanh (* pre_drive growl_drive)))
(def growl (formant-growl growl_in formant_scale formant_q morph_pos))
; Parallel narrow bands pass far less energy than the LP path; make up
; roughly 3x so the growl knob morphs timbre instead of dropping volume.
(def vowel_path (tanh (* growl 6.0)))

(def filt_out (+ (* (- 1.0 vowel_mix) clean_path) (* vowel_mix vowel_path)))

(def post (tanh (* filt_out output_bite)))
(def signal (* post amp_env velocity gain))

(out signal 1 @name audio)
