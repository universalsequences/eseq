; Digi Syn — independent Drift-inspired subtractive synthesizer.
; Presets are transcribed from the visible reference UI, never decoded files.
; Four independent complete DSP voices implement Mono, Stereo and Unison;
; the host resolves the declared voice allocation roles from the same params.
; This is an original synth implementation, not a claim of native equivalence.
; Pitch modulation uses a four-octave exponential law; matrix gains use 24 dB,
; detune 12 semitones, cutoff nine octaves, rates eight octaves. These explicit
; musical laws preserve the reference control values for later sound tuning.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))
(def note_on (in 10 @name note_on))
(def legato (in 11 @name legato))
(def pressure (in 12 @name pressure))
(def pitch_bend (in 13 @name pitch_bend))
(def modwheel (in 14 @name modwheel))
(def slide (in 15 @name slide))
(def clock_inc (in 16 @name clock_inc))

(param osc1_shape @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param osc1_gain_db @default -6 @min -120 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param osc2_detune @default 0 @min -12 @max 12 @unit st @mod true @mod-mode additive @mod-depth-min -12 @mod-depth-max 12 @mod-unit st)
(param osc2_gain_db @default -6 @min -120 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param noise_gain_db @default -60 @min -120 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param lp_freq @default 2500 @min 10 @max 20000 @unit Hz @mod true @mod-mode additive @mod-depth-min -8000 @mod-depth-max 8000 @mod-unit Hz)
(param lp_res @default 0.2 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param hp_freq @default 20 @min 10 @max 20000 @unit Hz @mod true @mod-mode additive @mod-depth-min -5000 @mod-depth-max 5000 @mod-unit Hz)
(param lfo_amount @default 1 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param drift @default 0.3 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param volume_db @default -12 @min -120 @max 6 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param glide_ms @default 0 @min 0 @max 10000 @unit ms)
(param env1_attack @default 4 @min 0 @max 60000 @unit ms)
(param env1_decay @default 350 @min 0 @max 60000 @unit ms)
(param env1_sustain @default 0.75 @min 0 @max 1)
(param env1_release @default 250 @min 0 @max 60000 @unit ms)
(param env2_mode @default 0 @min 0 @max 1)
(param env2_attack @default 2 @min 0 @max 60000 @unit ms)
(param env2_decay @default 400 @min 0 @max 60000 @unit ms)
(param env2_sustain @default 0.0 @min 0 @max 1)
(param env2_release @default 300 @min 0 @max 60000 @unit ms)
(param cyc_rate_hz @default 2 @min 0.01 @max 20000 @unit Hz)
(param cyc_tilt @default 0.5 @min 0 @max 1)
(param cyc_hold @default 0 @min 0 @max 1)
(param lfo_wave @default 0 @min 0 @max 8)
(param lfo_mode @default 0 @min 0 @max 3)
(param lfo_rate_hz @default 1.2 @min 0.01 @max 20000 @unit Hz)
(param lfo_ratio @default 1 @min 0.01 @max 32)
(param lfo_retrig @default 1 @min 0 @max 1)
(param mm1_src @default 2 @min 0 @max 7)
(param mm1_dest @default 0 @min 0 @max 11)
(param mm1_amt @default 0 @min -1 @max 1)
(param mm2_src @default 1 @min 0 @max 7)
(param mm2_dest @default 6 @min 0 @max 11)
(param mm2_amt @default 0 @min -1 @max 1)
(param mm3_src @default 4 @min 0 @max 7)
(param mm3_dest @default 11 @min 0 @max 11)
(param mm3_amt @default 0 @min -1 @max 1)
(param pitch_mod1_src @default 2 @min 0 @max 7)
(param pitch_mod1_amt @default 0 @min -1 @max 1)
(param pitch_mod2_src @default 0 @min 0 @max 7)
(param pitch_mod2_amt @default 0 @min -1 @max 1)
(param osc1_octave @default 0 @min -3 @max 3 @unit oct)
(param osc2_octave @default -1 @min -3 @max 3 @unit oct)
(param osc1_wave @default 4 @min 0 @max 6)
(param osc1_shape_src @default 2 @min 0 @max 7)
(param osc1_shape_amt @default 0 @min -1 @max 1)
(param osc2_wave @default 3 @min 0 @max 4)
(param osc1_on @default 1 @min 0 @max 1)
(param osc2_on @default 1 @min 0 @max 1)
(param osc1_route @default 1 @min 0 @max 1)
(param osc2_route @default 1 @min 0 @max 1)
(param noise_route @default 1 @min 0 @max 1)
(param filter_type @default 0 @min 0 @max 1)
(param keytrack @default 0.3 @min 0 @max 1)
(param lp_mod1_src @default 0 @min 0 @max 7)
(param lp_mod1_amt @default 0.1666667 @min -1 @max 1)
(param lp_mod2_src @default 2 @min 0 @max 7)
(param lp_mod2_amt @default 0 @min -1 @max 1)
(param vel_to_vol @default 0.35 @min 0 @max 1)
(param voice_pan @default 0 @min -1 @max 1)
(param spread @default 0.2 @min 0 @max 1)
(param voice_mode @default 0 @min 0 @max 3)
(param voice_count @default 16 @min 1 @max 32)
(param legato_on @default 0 @min 0 @max 1)
(param mono_thickness @default 0 @min 0 @max 1)
(param stereo_spread @default 0.7 @min 0 @max 1)
(param unison_strength @default 0.33 @min 0 @max 1)
(param transpose @default 0 @min -48 @max 48 @unit st)
(param note_pitch_bend_on @default 1 @min 0 @max 1)
(param pitch_bend_range @default 2 @min 0 @max 48 @unit st)
(param osc_retrig @default 0 @min 0 @max 1)
(param noise_on @default 0 @min 0 @max 1)
(param lfo_time_ms @default 1000 @min 0.05 @max 60000 @unit ms)
(param lfo_beats @default 1 @min 0.03125 @max 32)
(param lfo_mod_src @default 1 @min 0 @max 7)
(param lfo_mod_amt @default 0 @min -1 @max 1)
(param cyc_mode @default 0 @min 0 @max 3)
(param cyc_time_ms @default 500 @min 0.05 @max 60000 @unit ms)
(param cyc_ratio @default 1 @min 0.01 @max 32)
(param cyc_beats @default 1 @min 0.03125 @max 32)

; Sample-and-hold is explicit scalar history so each gated component owns
; its held value, with exactly one sample update on a trigger.
(defmacro syn-sample-hold (input reset)
  (make-history held_hist)
  (def result (gswitch (gt reset 0.5) input (read-history held_hist)))
  (write-history held_hist result)
  result)

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

; Free-running oscillator components start at independent phases. Retrigger
; explicitly starts at zero, without a per-note random phase offset.
(defmacro syn-osc-phase (freq reset)
  (make-history phase_hist)
  (make-history ready_hist)
  (def previous (gswitch (eq (read-history ready_hist) 0)
    (* 0.5 (+ 1 (noise))) (read-history phase_hist)))
  (def ph (gswitch (gt reset 0.5) 0 (wrap (+ previous (/ freq samplerate)) 0 1)))
  (write-history phase_hist ph)
  (write-history ready_hist 1)
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

(use-defmacro drift-filter-core)

; Sources: env1, env2, LFO, key, velocity, wheel, pressure, slide.
(defmacro pick-source (idx e1 e2 lf ky vl)
  (selector (+ (clip (round idx) 0 7) 1) e1 e2 lf ky vl modwheel pressure slide))

; One mod-matrix slot routed to destination d: amt*src when dest==d else 0.
(defmacro route-if-dest (dest d amt src_val)
  (* (eq (clip (round dest) 0 11) d) amt src_val))

; ======================================================================
; section macros (one collapsed node each at the top level)
; ======================================================================

(use-defmacro heat-glide)
(use-defmacro heat-unison-onset)

; Key follow value: 0 at C4 (261.63 Hz), +/-1 per two octaves.
(defmacro key-follow (hz)
  (clip (* (/ (log (/ (max hz 8.0) 261.63)) (log 2)) 0.5) -1 1))

; Per-voice analog drift: randoms latched at note start plus a slow wander.
; -> (pitch_cents osc2_extra_cents filter_oct pan_rnd)
(defmacro analog-drift (amount trig)
  (def amt (clip amount 0 1))
  (def rnd_pitch (syn-sample-hold (noise) trig))
  (def rnd_filt (syn-sample-hold (noise) trig))
  (def rnd_pan (syn-sample-hold (noise) trig))
  (def wander_ph (phasor 0.17))
  (def wander_target (syn-sample-hold (noise) (wrap-trigger wander_ph)))
  (def wander_val (smooth-toward wander_target 4.0))
  (tuple (* amt (+ (* rnd_pitch 3.0) (* wander_val 2.0)))
         (* amt rnd_pitch -2.5)
         (* amt rnd_filt 0.25)
         rnd_pan))

; Amp envelope.
(defmacro amp-env (gate_in trig)

  (adsr gate_in trig env1_attack env1_decay env1_sustain env1_release))

(defmacro syn-rate (mode hz ms ratio beats base_hz rate_mod)
  (def base (selector (+ (clip (round mode) 0 3) 1)
    hz (/ 1000 (max 0.05 ms)) (* base_hz ratio)
    (/ (* clock_inc samplerate 4) (max 0.03125 beats))))
  (clip (* base (pow 2 (* rate_mod 8))) 0.0001 (* samplerate 0.45)))

(defmacro env2-select (gate_in trig base_hz rate_mod)
  (def contour (adsr gate_in trig env2_attack env2_decay env2_sustain env2_release))
  (def rate (syn-rate cyc_mode cyc_rate_hz cyc_time_ms cyc_ratio cyc_beats base_hz rate_mod))
  (def ph (retrig-phasor rate trig))
  (def hold_f (* (clip cyc_hold 0 1) 0.9))
  (def avail (- 1 hold_f))
  (def rise (clip (* avail cyc_tilt) 0.0001 (- avail 0.0001)))
  (def fall (max 0.0001 (- avail rise)))
  (def cyclic (gswitch (lt ph rise) (/ ph rise)
    (gswitch (lt ph (+ rise hold_f)) 1
      (clip (- 1 (/ (- ph rise hold_f) fall)) 0 1))))
  (gswitch (gt env2_mode 0.5) cyclic contour))

(defmacro syn-lfo (amount base_hz trig rate_mod e1 e2 old_lfo key vel)
  (def freq (syn-rate lfo_mode lfo_rate_hz lfo_time_ms lfo_ratio lfo_beats base_hz rate_mod))
  (def one_shot (gt lfo_wave 6.5))
  (def reset (* trig (max lfo_retrig one_shot)))
  (make-history phase_hist)
  (make-history started_hist)
  (def next (+ (read-history phase_hist) (/ freq samplerate)))
  (def ph (gswitch (gt reset 0.5) 0
    (gswitch one_shot (min 1 next) (wrap next 0 1))))
  (write-history phase_hist ph)
  (def started (max (read-history started_hist) (gt trig 0.5)))
  (write-history started_hist started)
  (def wrap_event (wrap-trigger ph))
  (make-history old_target_hist)
  (make-history wander_start_hist)
  (def target (syn-sample-hold (noise) (max wrap_event reset)))
  (def previous_target (gswitch (max wrap_event reset)
    (read-history old_target_hist) (read-history wander_start_hist)))
  (write-history wander_start_hist previous_target)
  (write-history old_target_hist target)
  (def ease (* ph ph (- 3 (* 2 ph))))
  (def wander (+ previous_target (* ease (- target previous_target))))
  (def raw (selector (+ (clip (round lfo_wave) 0 8) 1)
    (sin (* ph twopi)) (triangle ph) (- (* ph 2) 1) (- 1 (* ph 2))
    (scale (lt ph 0.5) 0 1 -1 1) target wander
    (* started (- 1 ph)) (* started (gswitch (gte ph 1) 0 (exp (* -6.907755 ph))))))
  (def modulation (pick-source lfo_mod_src e1 e2 old_lfo key vel))
  (* raw (clip (+ amount (* modulation lfo_mod_amt)) 0 1)))

; Three routes summed into audio destinations and the two modulation rates.
; -> (o1_gain o1_shape o2_gain o2_det nz_gain lp_freq lp_res hp_freq volume lfo_rate cyc_rate)
(defmacro mod-matrix (e1 e2 lf ky vl)

  (def v1 (pick-source mm1_src e1 e2 lf ky vl))
  (def v2 (pick-source mm2_src e1 e2 lf ky vl))
  (def v3 (pick-source mm3_src e1 e2 lf ky vl))
  (tuple
    (+ (route-if-dest mm1_dest 1 mm1_amt v1) (route-if-dest mm2_dest 1 mm2_amt v2) (route-if-dest mm3_dest 1 mm3_amt v3))
    (+ (route-if-dest mm1_dest 2 mm1_amt v1) (route-if-dest mm2_dest 2 mm2_amt v2) (route-if-dest mm3_dest 2 mm3_amt v3))
    (+ (route-if-dest mm1_dest 3 mm1_amt v1) (route-if-dest mm2_dest 3 mm2_amt v2) (route-if-dest mm3_dest 3 mm3_amt v3))
    (+ (route-if-dest mm1_dest 4 mm1_amt v1) (route-if-dest mm2_dest 4 mm2_amt v2) (route-if-dest mm3_dest 4 mm3_amt v3))
    (+ (route-if-dest mm1_dest 5 mm1_amt v1) (route-if-dest mm2_dest 5 mm2_amt v2) (route-if-dest mm3_dest 5 mm3_amt v3))
    (+ (route-if-dest mm1_dest 6 mm1_amt v1) (route-if-dest mm2_dest 6 mm2_amt v2) (route-if-dest mm3_dest 6 mm3_amt v3))
    (+ (route-if-dest mm1_dest 7 mm1_amt v1) (route-if-dest mm2_dest 7 mm2_amt v2) (route-if-dest mm3_dest 7 mm3_amt v3))
    (+ (route-if-dest mm1_dest 8 mm1_amt v1) (route-if-dest mm2_dest 8 mm2_amt v2) (route-if-dest mm3_dest 8 mm3_amt v3))
    (+ (route-if-dest mm1_dest 11 mm1_amt v1) (route-if-dest mm2_dest 11 mm2_amt v2) (route-if-dest mm3_dest 11 mm3_amt v3))
    (+ (route-if-dest mm1_dest 9 mm1_amt v1) (route-if-dest mm2_dest 9 mm2_amt v2) (route-if-dest mm3_dest 9 mm3_amt v3))
    (+ (route-if-dest mm1_dest 10 mm1_amt v1) (route-if-dest mm2_dest 10 mm2_amt v2) (route-if-dest mm3_dest 10 mm3_amt v3))))

; Oscillator frequencies from pitch mod, drift, octave/detune. -> (f1 f2)
(defmacro osc-frequencies (base_hz e1 e2 lf ky vl
                           drift_cents o2_extra_cents detune mm_det)

  (def pm1 (pick-source pitch_mod1_src e1 e2 lf ky vl))
  (def pm2 (pick-source pitch_mod2_src e1 e2 lf ky vl))
  (def mod_semis (* 48 (+ (* pm1 pitch_mod1_amt) (* pm2 pitch_mod2_amt))))
  (def common (* base_hz (semi-ratio mod_semis)
                 (semi-ratio (/ drift_cents 100.0))))
  (tuple (* common (semi-ratio (* (clip (round osc1_octave) -3 3) 12)))
         (* common (semi-ratio (+ (* (clip (round osc2_octave) -3 3) 12)
                                  (clip (+ detune (* mm_det 12)) -24 24)
                                  (/ o2_extra_cents 100.0))))))

; Morphing oscillator: wave selects sine / asym-tri / shark / saturated /
; saw / pulse / rect; shape (base + mod source * amt + matrix) morphs within
; the selected wave.
(defmacro morph-osc (freq shape_base e1 e2 lf ky vl mm_shape voice_trigger)

  (def shp_val (pick-source osc1_shape_src e1 e2 lf ky vl))
  (def shape (clip (+ shape_base (* shp_val osc1_shape_amt) mm_shape) 0 1))
  (def ph (syn-osc-phase (clip freq 0.01 (* samplerate 0.45)) (* osc_retrig voice_trigger)))
  (def o_sine (sin (* twopi (+ ph (* shape 0.15 (sin (* ph twopi)))))))
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
(defmacro basic-osc (freq voice_trigger)

  (def ph (syn-osc-phase (clip freq 0.01 (* samplerate 0.45)) (* osc_retrig voice_trigger)))
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

  (def g1 (* (gte osc1_on 0.5) (db-amp (clip (+ gain1_db (* mm_g1 24)) -120 12))))
  (def g2 (* (gte osc2_on 0.5) (db-amp (clip (+ gain2_db (* mm_g2 24)) -120 12))))
  (def nz_db_c (clip (+ nz_db (* mm_nz 24)) -120 12))
  (def gn (* noise_on (gt nz_db_c -119.5) (db-amp nz_db_c)))
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

  (def fm1 (pick-source lp_mod1_src e1 e2 lf ky vl))
  (def fm2 (pick-source lp_mod2_src e1 e2 lf ky vl))
  (def oct_mod (+ (* fm1 lp_mod1_amt 9) (* fm2 lp_mod2_amt 9)
                  (* mm_freq 9) drift_oct))
  (def cut (clip (* freq_hz
                    (pow (/ (max base_hz 8.0) 261.63) keytrack)
                    (semi-ratio (* oct_mod 12)))
                 10 (min 20000 (* samplerate .45))))
  (def hp_cut (clip (* hp_hz (semi-ratio (* mm_hp 108)))
                    10 (min 20000 (* samplerate .45))))
  (def res (clip (+ res_base mm_res) 0 1))
  (drift-filter-core x cut res hp_cut filter_type))

; Amp envelope, velocity, volume (with matrix offset), per-voice pan spread,
; pre-envelope summing saturation, then linear stereo gain. -> (left right)
(defmacro output-stage (filt dry env vel vol_db mm_vol pan_rnd component_pan)

  (def vel_gain (+ (- 1 vel_to_vol) (* vel vel_to_vol)))
  (def vol (db-amp (clip (+ vol_db (* mm_vol 24)) -120 6)))
  (def amp (* (clip (+ filt dry) -.875 .875) env vel_gain vol))
  (def pan (clip (+ voice_pan component_pan (* pan_rnd spread)) -1 1))
  (tuple (* amp (clip (- 1 (* pan 0.5)) 0 1.5))
         (* amp (clip (+ 1 (* pan 0.5)) 0 1.5))))

(defmacro syn-enable (target)
  (make-history ready)
  (make-history level)
  (def old (read-history level))
  (def step (/ 1 (* 0.002 samplerate)))
  (def value (gswitch (eq (read-history ready) 0) target
    (+ old (clip (- target old) (- 0 step) step))))
  (write-history ready 1)
  (write-history level value)
  value)

; Execution window for a section that owns released state. It rises to exact
; one immediately and falls to exact zero only after hold_ms, so a section can
; be kept executing after its audible fade has finished and then be frozen at
; a known-idle state rather than mid-release.
(defmacro syn-run (target hold_ms)
  (make-history ready)
  (make-history level)
  (def old (read-history level))
  (def step (/ 1000 (* (max 1 hold_ms) samplerate)))
  (def value (gswitch (eq (read-history ready) 0) target
    (gswitch (gt target 0.5) 1 (max 0 (- old step)))))
  (write-history ready 1)
  (write-history level value)
  value)

(defmacro syn-voice (voice_gate voice_trigger base_pitch vel cents component_pan
    m_drift m_lfo_amount m_osc2_detune m_shape m_gain1 m_gain2 m_noise m_lp m_res m_hp m_volume)
  (def key_val (key-follow base_pitch))
  (def (drift_cents o2_drift_cents drift_filt_oct pan_rnd) (analog-drift m_drift voice_trigger))
  (def env1 (amp-env voice_gate voice_trigger))
  (make-history env2_hist)
  (make-history lfo_hist)
  (def old_env2 (read-history env2_hist))
  (def old_lfo (read-history lfo_hist))
  (def (unused1 unused2 unused3 unused4 unused5 unused6 unused7 unused8 unused9 rate_lfo rate_cyc)
    (mod-matrix env1 old_env2 old_lfo key_val vel))
  (def env2 (env2-select voice_gate voice_trigger base_pitch rate_cyc))
  (def lfo (syn-lfo m_lfo_amount base_pitch voice_trigger rate_lfo env1 env2 old_lfo key_val vel))
  (write-history env2_hist env2)
  (write-history lfo_hist lfo)
  (def (mm_o1_gain mm_o1_shape mm_o2_gain mm_o2_det mm_nz_gain mm_lp_freq mm_lp_res mm_hp_freq mm_volume unused10 unused11)
    (mod-matrix env1 env2 lfo key_val vel))
  (def (f1 f2) (osc-frequencies base_pitch env1 env2 lfo key_val vel
    (+ drift_cents cents) o2_drift_cents m_osc2_detune mm_o2_det))
  (def osc1 (morph-osc f1 m_shape env1 env2 lfo key_val vel mm_o1_shape voice_trigger))
  (def osc2 (basic-osc f2 voice_trigger))
  (def (to_filter dry) (source-mixer osc1 osc2 m_gain1 m_gain2 m_noise mm_o1_gain mm_o2_gain mm_nz_gain))
  (def filtered (drift-filter to_filter base_pitch env1 env2 lfo key_val vel m_lp m_res m_hp mm_lp_freq mm_lp_res mm_hp_freq drift_filt_oct))
  (output-stage filtered dry env1 vel m_volume mm_volume pan_rnd component_pan))

(def played_octave (/ (log (/ (max pitch 0.01) 261.625565)) (log 2)))
(def glide_mode (* (eq (round voice_mode) 1) 2))
(def glided_octave (heat-glide played_octave note_on legato glide_mode glide_ms 0))
(def base_pitch (* 261.625565 (pow 2 glided_octave)
  (semi-ratio (+ transpose (* note_pitch_bend_on pitch_bend pitch_bend_range)))))
(def copies (selector (+ (clip (round voice_mode) 0 3) 1) 1 4 2 4))
(def release_window (+ 5 env1_attack env1_decay env1_release env2_attack env2_decay env2_release))
(def enabled0 (gt copies 0))
(def (g0 n0 t0 l0 p0 v0)
  (heat-unison-onset gate note_on trigger legato base_pitch velocity enabled0 0))
(def fade0 (syn-enable enabled0))
(def run0 (syn-run enabled0 release_window))
(def position0 -1)
(def detune0 (* (eq (round voice_mode) 3) position0 unison_strength 35))
(def pan0 (* (eq (round voice_mode) 2) position0 stereo_spread))
(def weight0 (gswitch (eq (round voice_mode) 1) 1 1))
(def (left0 right0) (block-gate run0
  (syn-voice g0 t0 p0 v0 detune0 pan0
    (mod drift) (mod lfo_amount) (mod osc2_detune) (mod osc1_shape)
    (mod osc1_gain_db) (mod osc2_gain_db) (mod noise_gain_db)
    (mod lp_freq) (mod lp_res) (mod hp_freq) (mod volume_db))))
(def enabled1 (gt copies 1))
(def (g1 n1 t1 l1 p1 v1)
  (heat-unison-onset gate note_on trigger legato base_pitch velocity enabled1 0))
(def fade1 (syn-enable enabled1))
(def run1 (syn-run enabled1 release_window))
(def position1 (gswitch (eq (round voice_mode) 2) 1 -0.3333333))
(def detune1 (* (eq (round voice_mode) 3) position1 unison_strength 35))
(def pan1 (* (eq (round voice_mode) 2) position1 stereo_spread))
(def weight1 (gswitch (eq (round voice_mode) 1) mono_thickness 1))
(def (left1 right1) (block-gate run1
  (syn-voice g1 t1 p1 v1 detune1 pan1
    (mod drift) (mod lfo_amount) (mod osc2_detune) (mod osc1_shape)
    (mod osc1_gain_db) (mod osc2_gain_db) (mod noise_gain_db)
    (mod lp_freq) (mod lp_res) (mod hp_freq) (mod volume_db))))
(def enabled2 (gt copies 2))
(def (g2 n2 t2 l2 p2 v2)
  (heat-unison-onset gate note_on trigger legato base_pitch velocity enabled2 0))
(def fade2 (syn-enable enabled2))
(def run2 (syn-run enabled2 release_window))
(def position2 0.3333333)
(def detune2 (* (eq (round voice_mode) 3) position2 unison_strength 35))
(def pan2 (* (eq (round voice_mode) 2) position2 stereo_spread))
(def weight2 (gswitch (eq (round voice_mode) 1) mono_thickness 1))
(def (left2 right2) (block-gate run2
  (syn-voice g2 t2 p2 v2 detune2 pan2
    (mod drift) (mod lfo_amount) (mod osc2_detune) (mod osc1_shape)
    (mod osc1_gain_db) (mod osc2_gain_db) (mod noise_gain_db)
    (mod lp_freq) (mod lp_res) (mod hp_freq) (mod volume_db))))
(def enabled3 (gt copies 3))
(def (g3 n3 t3 l3 p3 v3)
  (heat-unison-onset gate note_on trigger legato base_pitch velocity enabled3 0))
(def fade3 (syn-enable enabled3))
(def run3 (syn-run enabled3 release_window))
(def position3 1)
(def detune3 (* (eq (round voice_mode) 3) position3 unison_strength 35))
(def pan3 (* (eq (round voice_mode) 2) position3 stereo_spread))
(def weight3 (gswitch (eq (round voice_mode) 1) mono_thickness 1))
(def (left3 right3) (block-gate run3
  (syn-voice g3 t3 p3 v3 detune3 pan3
    (mod drift) (mod lfo_amount) (mod osc2_detune) (mod osc1_shape)
    (mod osc1_gain_db) (mod osc2_gain_db) (mod noise_gain_db)
    (mod lp_freq) (mod lp_res) (mod hp_freq) (mod volume_db))))
(def normalization (gswitch (eq (round voice_mode) 1) (+ 1 (* mono_thickness 3)) copies))
(out (/ (+ (* fade0 weight0 left0) (* fade1 weight1 left1) (* fade2 weight2 left2) (* fade3 weight3 left3)) normalization) 1 @name left)
(out (/ (+ (* fade0 weight0 right0) (* fade1 weight1 right1) (* fade2 weight2 right2) (* fade3 weight3 right3)) normalization) 2 @name right)
