; Drift-style subtractive synth modeled after Ableton Drift.
; Two shape-morphing oscillators + noise, per-source filter routing with
; pre/post filter saturation driven by mixer gain staging, dual low-pass
; types (I: 12dB driven SVF, II: 24dB cascaded Sallen-Key-ish), per-voice
; analog drift (latched random pitch/filter offsets + slow wander), two
; envelopes (env2 can cycle), one multi-wave LFO, and a compact mod matrix.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro dbamp (db)
  (exp (* 0.1151292546 db)))

; Latch a signal when trig fires; hold it otherwise (per-voice randoms).
(defmacro latch_on_trigger (sig trig)
  (make-history hold_hist)
  (def held (read-history hold_hist))
  (def latched (gswitch (gt trig 0.5) sig held))
  (write-history hold_hist latched)
  latched)

; Resettable phase accumulator: restarts at 0 when reset fires.
(defmacro retrig_phasor (freq reset)
  (make-history ph_hist)
  (def prev_ph (read-history ph_hist))
  (def next_ph (wrap (+ prev_ph (/ freq samplerate)) 0 1))
  (def ph (gswitch (gt reset 0.5) 0.0 next_ph))
  (write-history ph_hist ph)
  ph)

(param osc1_on @default 1 @min 0 @max 1)
(param osc1_wave @default 4 @min 0 @max 6)
(param osc1_octave @default 0 @min -3 @max 3 @unit oct)
(param osc1_shape @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param osc1_shape_src @default 2 @min 0 @max 4)
(param osc1_shape_amt @default 0 @min -1 @max 1)
(param osc1_gain_db @default -6 @min -36 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param osc1_route @default 1 @min 0 @max 1)

(param osc2_on @default 1 @min 0 @max 1)
(param osc2_wave @default 3 @min 0 @max 4)
(param osc2_octave @default -1 @min -3 @max 3 @unit oct)
(param osc2_detune @default 0 @min -12 @max 12 @unit st @mod true @mod-mode additive @mod-depth-min -12 @mod-depth-max 12 @mod-unit st)
(param osc2_gain_db @default -6 @min -36 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param osc2_route @default 1 @min 0 @max 1)

(param noise_gain_db @default -60 @min -60 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param noise_route @default 1 @min 0 @max 1)

(param pitch_mod1_src @default 2 @min 0 @max 4)
(param pitch_mod1_amt @default 0 @min -24 @max 24 @unit st)
(param pitch_mod2_src @default 0 @min 0 @max 4)
(param pitch_mod2_amt @default 0 @min -24 @max 24 @unit st)

(param filter_type @default 0 @min 0 @max 1)
(param lp_freq @default 2500 @min 20 @max 18000 @unit Hz @mod true @mod-mode additive @mod-depth-min -8000 @mod-depth-max 8000 @mod-unit Hz)
(param lp_res @default 0.2 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param hp_freq @default 20 @min 10 @max 10000 @unit Hz @mod true @mod-mode additive @mod-depth-min -5000 @mod-depth-max 5000 @mod-unit Hz)
(param keytrack @default 0.3 @min 0 @max 1)
(param lp_mod1_src @default 0 @min 0 @max 4)
(param lp_mod1_amt @default 1.5 @min -8 @max 8 @unit oct)
(param lp_mod2_src @default 2 @min 0 @max 4)
(param lp_mod2_amt @default 0 @min -8 @max 8 @unit oct)

(param env1_attack @default 4 @min 1 @max 5000 @unit ms)
(param env1_decay @default 350 @min 5 @max 10000 @unit ms)
(param env1_sustain @default 0.75 @min 0 @max 1)
(param env1_release @default 250 @min 5 @max 12000 @unit ms)

(param env2_mode @default 0 @min 0 @max 1)
(param env2_attack @default 2 @min 1 @max 5000 @unit ms)
(param env2_decay @default 400 @min 5 @max 10000 @unit ms)
(param env2_sustain @default 0.0 @min 0 @max 1)
(param env2_release @default 300 @min 5 @max 12000 @unit ms)
(param cyc_rate_hz @default 2 @min 0.05 @max 40 @unit Hz)
(param cyc_tilt @default 0.5 @min 0 @max 1)
(param cyc_hold @default 0 @min 0 @max 1)

(param lfo_wave @default 0 @min 0 @max 6)
(param lfo_mode @default 0 @min 0 @max 1)
(param lfo_rate_hz @default 1.2 @min 0.01 @max 30 @unit Hz)
(param lfo_ratio @default 1 @min 0.01 @max 4)
(param lfo_retrig @default 1 @min 0 @max 1)
(param lfo_amount @default 1 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

(param mm1_src @default 2 @min 0 @max 4)
(param mm1_dest @default 0 @min 0 @max 8)
(param mm1_amt @default 0 @min -1 @max 1)
(param mm2_src @default 1 @min 0 @max 4)
(param mm2_dest @default 5 @min 0 @max 8)
(param mm2_amt @default 0 @min -1 @max 1)
(param mm3_src @default 4 @min 0 @max 4)
(param mm3_dest @default 8 @min 0 @max 8)
(param mm3_amt @default 0 @min -1 @max 1)

(param drift @default 0.3 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param spread @default 0.2 @min 0 @max 1)
(param voice_pan @default 0 @min -1 @max 1)
(param glide_ms @default 0 @min 0 @max 1000 @unit ms)
(param vel_to_vol @default 0.35 @min 0 @max 1)
(param volume_db @default -12 @min -36 @max 6 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)

; ---- glide / base pitch (Hz) ----
(def glide_coeff (exp (/ -1.0 (max 1.0 (* glide_ms 0.001 samplerate)))))
(make-history glide_hist)
(def prev_glide (read-history glide_hist))
(def glide_pitch (+ (* pitch (- 1 glide_coeff)) (* prev_glide glide_coeff)))
(def base_pitch (gswitch (gt glide_ms 0.1) glide_pitch pitch))
(write-history glide_hist base_pitch)

; key follow value: 0 at C4 (261.63 Hz), +/-1 per two octaves
(def key_val (clip (* (/ (log (/ (max base_pitch 8.0) 261.63)) (log 2)) 0.5) -1 1))

; ---- per-voice drift randoms (latched at note start) ----
(def drift_amt (clip (mod drift) 0 1))
(def rnd_pitch (latch_on_trigger (noise) trigger))
(def rnd_filt (latch_on_trigger (noise) trigger))
(def rnd_pan (latch_on_trigger (noise) trigger))
; slow continuous wander: sample & hold noise smoothed toward target
(def wander_ph (phasor 0.17))
(make-history wander_ph_hist)
(def wander_prev_ph (read-history wander_ph_hist))
(def wander_wrapped (lt wander_ph wander_prev_ph))
(write-history wander_ph_hist wander_ph)
(def wander_target (latch_on_trigger (noise) wander_wrapped))
(make-history wander_hist)
(def wander_prev (read-history wander_hist))
(def wander_coeff (/ 4.0 samplerate))
(def wander_val (+ wander_prev (* wander_coeff (- wander_target wander_prev))))
(write-history wander_hist wander_val)
(def drift_pitch_cents (* drift_amt (+ (* rnd_pitch 3.0) (* wander_val 2.0))))
(def drift_filt_oct (* drift_amt rnd_filt 0.25))

; ---- envelopes ----
(def env1 (adsr gate trigger env1_attack env1_decay env1_sustain env1_release))
(def env2_adsr (adsr gate trigger env2_attack env2_decay env2_sustain env2_release))
; cycling envelope: note-retriggered rise/hold/fall shape
(def cyc_ph (retrig_phasor cyc_rate_hz trigger))
(def cyc_hold_f (* (clip cyc_hold 0 1) 0.9))
(def cyc_avail (- 1.0 cyc_hold_f))
(def cyc_rise (clip (* cyc_avail cyc_tilt) 0.01 (- cyc_avail 0.01)))
(def cyc_fall (max 0.01 (- cyc_avail cyc_rise)))
(def cyc_in_rise (lt cyc_ph cyc_rise))
(def cyc_in_hold (* (gte cyc_ph cyc_rise) (lt cyc_ph (+ cyc_rise cyc_hold_f))))
(def cyc_in_fall (gte cyc_ph (+ cyc_rise cyc_hold_f)))
(def cyc_val (+ (* cyc_in_rise (/ cyc_ph cyc_rise))
                cyc_in_hold
                (* cyc_in_fall (clip (- 1.0 (/ (- cyc_ph cyc_rise cyc_hold_f) cyc_fall)) 0 1))))
(def env2 (gswitch (gte env2_mode 0.5) cyc_val env2_adsr))

; ---- LFO ----
(def lfo_freq (clip (gswitch (gte lfo_mode 0.5)
                             (* base_pitch lfo_ratio)
                             lfo_rate_hz)
                    0.01 (* samplerate 0.45)))
(def lfo_reset (* lfo_retrig trigger))
(def lfo_ph (retrig_phasor lfo_freq lfo_reset))
(make-history lfo_ph_prev_hist)
(def lfo_prev_ph (read-history lfo_ph_prev_hist))
(def lfo_wrapped (lt lfo_ph lfo_prev_ph))
(write-history lfo_ph_prev_hist lfo_ph)
(def lfo_sh (latch_on_trigger (noise) (max lfo_wrapped lfo_reset)))
(make-history lfo_wander_hist)
(def lfo_wander_prev (read-history lfo_wander_hist))
(def lfo_wander_coeff (clip (* 8.0 (/ lfo_freq samplerate)) 0.00001 1))
(def lfo_wander (+ lfo_wander_prev (* lfo_wander_coeff (- lfo_sh lfo_wander_prev))))
(write-history lfo_wander_hist lfo_wander)
(def lfo_idx (clip (round lfo_wave) 0 6))
(def lfo_raw (selector (+ lfo_idx 1)
                (sin (* lfo_ph twopi))
                (triangle lfo_ph)
                (- (* lfo_ph 2) 1)
                (- 1 (* lfo_ph 2))
                (scale (lt lfo_ph 0.5) 0 1 -1 1)
                lfo_sh
                lfo_wander))
(def lfo (* lfo_raw (clip (mod lfo_amount) 0 1)))

; ---- mod sources (env1 env2 lfo key vel) ----
(defmacro src_select (idx e1 e2 lf ky vl)
  (selector (+ (clip (round idx) 0 4) 1) e1 e2 lf ky vl))

(def pm1_val (src_select pitch_mod1_src env1 env2 lfo key_val velocity))
(def pm2_val (src_select pitch_mod2_src env1 env2 lfo key_val velocity))
(def fm1_val (src_select lp_mod1_src env1 env2 lfo key_val velocity))
(def fm2_val (src_select lp_mod2_src env1 env2 lfo key_val velocity))
(def shp_val (src_select osc1_shape_src env1 env2 lfo key_val velocity))
(def mm1_val (* mm1_amt (src_select mm1_src env1 env2 lfo key_val velocity)))
(def mm2_val (* mm2_amt (src_select mm2_src env1 env2 lfo key_val velocity)))
(def mm3_val (* mm3_amt (src_select mm3_src env1 env2 lfo key_val velocity)))

; ---- mod matrix destination sums ----
(defmacro mm_dest_sum (d)
  (+ (* (eq (clip (round mm1_dest) 0 8) d) mm1_val)
     (* (eq (clip (round mm2_dest) 0 8) d) mm2_val)
     (* (eq (clip (round mm3_dest) 0 8) d) mm3_val)))

(def mm_o1_gain (mm_dest_sum 0))
(def mm_o1_shape (mm_dest_sum 1))
(def mm_o2_gain (mm_dest_sum 2))
(def mm_o2_det (mm_dest_sum 3))
(def mm_nz_gain (mm_dest_sum 4))
(def mm_lp_freq (mm_dest_sum 5))
(def mm_lp_res (mm_dest_sum 6))
(def mm_hp_freq (mm_dest_sum 7))
(def mm_volume (mm_dest_sum 8))

; ---- oscillator frequencies ----
(def pitch_mod_semis (+ (* pm1_val pitch_mod1_amt) (* pm2_val pitch_mod2_amt)))
(def common_ratio (* (semi_ratio pitch_mod_semis)
                     (semi_ratio (/ drift_pitch_cents 100.0))))
(def f1 (* base_pitch common_ratio
           (semi_ratio (* (clip (round osc1_octave) -3 3) 12))))
(def f2 (* base_pitch common_ratio
           (semi_ratio (+ (* (clip (round osc2_octave) -3 3) 12)
                          (clip (+ (mod osc2_detune) (* mm_o2_det 12)) -24 24)
                          (* drift_amt rnd_pitch -2.5 0.01)))))

; ---- oscillator 1 (shape-morphing) ----
(def shape1 (clip (+ (mod osc1_shape) (* shp_val osc1_shape_amt) mm_o1_shape) 0 1))
(def ph1 (phasor f1))
(def o1_sine (sin (* ph1 twopi)))
; asymmetric triangle: shape moves the peak (also serves shark tooth blend)
(def tri_peak (clip (+ 0.05 (* shape1 0.9)) 0.05 0.95))
(def o1_tri_asym (gswitch (lt ph1 tri_peak)
                   (- (* (/ ph1 tri_peak) 2) 1)
                   (- (* (/ (- 1 ph1) (- 1 tri_peak)) 2) 1)))
(def o1_saw_raw (polyblep_saw ph1 f1))
; shark tooth: saw morphing into bent asymmetric triangle
(def o1_shark (+ (* (- 1 shape1) o1_saw_raw) (* shape1 o1_tri_asym)))
; saturated: driven saw through tanh, shape = drive
(def sat_drive (+ 1.0 (* shape1 5.0)))
(def o1_sat (/ (tanh (* o1_saw_raw sat_drive)) (tanh sat_drive)))
; saw: shape adds gentle drive-brightness
(def o1_saw (/ (tanh (* o1_saw_raw (+ 1.0 (* shape1 1.5)))) (tanh (+ 1.0 (* shape1 1.5)))))
(def pw1 (clip (+ 0.05 (* shape1 0.9)) 0.05 0.95))
(def o1_pulse (polyblep_pulse ph1 pw1 f1))
(def rect_w (clip (+ 0.5 (* (- shape1 0.5) 0.6)) 0.2 0.8))
(def o1_rect (polyblep_pulse ph1 rect_w f1))
(def osc1 (selector (+ (clip (round osc1_wave) 0 6) 1)
            o1_sine o1_tri_asym o1_shark o1_sat o1_saw o1_pulse o1_rect))

; ---- oscillator 2 ----
(def ph2 (phasor f2))
(def o2_saw_raw (polyblep_saw ph2 f2))
(def o2_sat (/ (tanh (* o2_saw_raw 3.0)) (tanh 3.0)))
(def osc2 (selector (+ (clip (round osc2_wave) 0 4) 1)
            (sin (* ph2 twopi))
            (triangle ph2)
            o2_sat
            o2_saw_raw
            (polyblep_pulse ph2 0.5 f2)))

(def nz (noise))

; ---- mixer: gain staging + per-source filter routing ----
(def o1_gain (* (gte osc1_on 0.5)
                (dbamp (clip (+ (mod osc1_gain_db) (* mm_o1_gain 24)) -36 12))))
(def o2_gain (* (gte osc2_on 0.5)
                (dbamp (clip (+ (mod osc2_gain_db) (* mm_o2_gain 24)) -36 12))))
(def nz_gain_db_v (clip (+ (mod noise_gain_db) (* mm_nz_gain 24)) -60 12))
(def nz_gain (* (gt nz_gain_db_v -59.5) (dbamp nz_gain_db_v)))
(def src1 (* osc1 o1_gain))
(def src2 (* osc2 o2_gain))
(def srcn (* nz nz_gain))
(def r1 (gte osc1_route 0.5))
(def r2 (gte osc2_route 0.5))
(def rn (gte noise_route 0.5))
(def routed (+ (* src1 r1) (* src2 r2) (* srcn rn)))
(def dry (+ (* src1 (- 1 r1)) (* src2 (- 1 r2)) (* srcn (- 1 rn))))

; pre-filter saturation: transparent below the -6 dB default region,
; compresses as the mixer pushes past it
(def pre_sat (/ (tanh (* routed 0.8)) 0.8))

; ---- filter ----
(def hp_cut (clip (* (mod hp_freq) (semi_ratio (* mm_hp_freq 72))) 10 12000))
(def hp_out (svf pre_sat hp_cut 0.6 2))
(def lp_oct_mod (+ (* fm1_val lp_mod1_amt) (* fm2_val lp_mod2_amt)
                   (* mm_lp_freq 6) drift_filt_oct))
(def lp_cut (clip (* (mod lp_freq)
                     (pow (/ (max base_pitch 8.0) 261.63) keytrack)
                     (semi_ratio (* lp_oct_mod 12)))
                  20 16000))
(def res (clip (+ (mod lp_res) mm_lp_res) 0 1))
; Type I: 12 dB driven SVF — saturates early, woolly resonance
(def lp_i (tanh (svf (tanh (* hp_out 1.3)) lp_cut (+ 0.5 (* res 8.0)) 0)))
; Type II: 24 dB cascaded with soft clip between stages — cleaner slope
(def lp_ii_a (svf hp_out lp_cut (+ 0.5 (* res 4.0)) 0))
(def lp_ii (svf (/ (tanh (* lp_ii_a 0.9)) 0.9) lp_cut (+ 0.5 (* res 4.0)) 0))
(def lp_out (gswitch (gte filter_type 0.5) lp_ii lp_i))

; post-filter saturation: only bites above the 0 dB region
(def post_sat (tanh lp_out))
(def mixed (+ post_sat dry))

; ---- amp / output ----
(def vel_gain (+ (- 1 vel_to_vol) (* velocity vel_to_vol)))
(def vol (dbamp (clip (+ (mod volume_db) (* mm_volume 24)) -36 6)))
(def amp (* mixed env1 vel_gain vol))
(def pan (clip (+ voice_pan (* rnd_pan spread)) -1 1))
(def left (* amp (clip (- 1 (* pan 0.5)) 0 1.5)))
(def right (* amp (clip (+ 1 (* pan 0.5)) 0 1.5)))

(out (tanh left) 1 @name left)
(out (tanh right) 2 @name right)
