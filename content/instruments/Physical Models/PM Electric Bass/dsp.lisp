;; Plucked stiff string, read by a finite-width velocity-sensitive pickup.
;; 64 modal coordinates; no samples or auxiliary attack oscillator.
;; Equations, approximations and validation: tools/pm-electric-bass/README.md.
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(param position @group pluck @default 0.23 @min 0.05 @max 0.5)
(param softness @group pluck @default 0.7 @min 0 @max 1)
(param velocity_tone @group pluck @default 0.45 @min 0 @max 1)
(param attack_ms @group pluck @default 0 @min 0 @max 80 @unit ms)
(param decay_s @group string @default 3.5 @min 0.2 @max 12 @unit s @mod true @mod-mode additive)
(param damping @group string @default 0.55 @min 0 @max 1 @mod true @mod-mode additive)
(param mute @group string @default 0.38 @min 0 @max 1 @mod true @mod-mode additive)
(param stiffness @group string @default 0.15 @min 0 @max 1 @mod true @mod-mode additive)
(param friction @group string @default 0 @min 0 @max 12 @mod true @mod-mode additive)
(param release_s @group string @default 0.12 @min 0.025 @max 2 @unit s @mod true @mod-mode additive)
(param tune @group string @default 0 @min -100 @max 100 @unit cents @mod true @mod-mode additive)
(param position @group pickup @default 0.22 @min 0.04 @max 0.45 @mod true @mod-mode additive)
(param aperture @group pickup @default 0.035 @min 0.005 @max 0.1 @mod true @mod-mode additive)
(param tone_hz @group output @default 1700 @min 250 @max 10000 @unit Hz @mod true @mod-mode additive)
(param resonance @group output @default 0.8 @min 0.5 @max 1.4 @mod true @mod-mode additive)
(param steep @group output @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param gain @group output @default 0.7 @min 0 @max 1 @mod true @mod-mode additive)
(param hiss @group vinyl @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param rumble @group vinyl @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param slow @group texture @default 0 @min 0 @max 40 @unit % @mod true @mod-mode additive)
(param grain_ms @group texture @default 55 @min 20 @max 200 @unit ms)
(param xfade_ms @group texture @default 8 @min 0.5 @max 30 @unit ms)

(def modes (tensor @shape [64] @data [
  1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
  17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32
  33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48
  49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64]))
(def sub_mask (tensor @shape [64] @data [
  1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
  0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
  0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
  0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0]))
(make-history last_gate)
(def held (gt gate 0.5))
(def onset (max (gt trigger 0.5) (* held (lte (read-history last_gate) 0.5))))
(def key_up (* (lte gate 0.5) (gt (read-history last_gate) 0.5)))
(write-history last_gate held)
(def tick (max (eq (accum 1 0 0 16) 0) (max onset key_up)))
(def hit_velocity (latch (clip velocity 0 1) onset))
;; The latch is zero before the first note; keep unplayed coefficients defined.
(def pluck_position (clip (latch (clip pluck.position 0.05 0.5) onset) 0.05 0.5))
(def finger_width (latch (+ 0.004 (* 0.075 (clip
  (+ pluck.softness (* pluck.velocity_tone (- 1 (clip velocity 0 1)))) 0 1))) onset))

(defmacro bass-coefficient (value update) (latch value update))

(defmacro bass-modes (n update excite level pluck width frequency damping friction mute decay release held stiffness pickup aperture sub_gain)
  (make-tensor-history real_h @shape [64])
  (make-tensor-history imag_h @shape [64])
  ;; Schedule the pure design from its inputs, before transcendental/modal
  ;; work. Holding only the final result leaves its ancestors at audio rate.
  ;; The existing tick includes note onset/off; final latches restore the
  ;; coefficients to audio rate before updating the complex resonators.
  (def pluck-coefficient (event-hold pluck update))
  (def width-coefficient (event-hold width update))
  (def frequency-coefficient (event-hold frequency update))
  (def damping-coefficient (event-hold damping update))
  (def friction-coefficient (event-hold friction update))
  (def mute-coefficient (event-hold mute update))
  (def decay-coefficient (event-hold decay update))
  (def release-coefficient (event-hold release update))
  (def held-coefficient (event-hold held update))
  (def stiffness-coefficient (event-hold stiffness update))
  (def pickup-coefficient (event-hold pickup update))
  (def aperture-coefficient (event-hold aperture update))
  (def b (* 0.002 stiffness-coefficient stiffness-coefficient))
  ;; Divide by sqrt(1+B) so stiffness does not detune the fundamental.
  (def ratios (* n (sqrt (/ (+ 1 (* b n n)) (+ 1 b)))))
  (def hz (* frequency-coefficient ratios))
  (def omega (* twopi (/ (min hz (* 0.49 samplerate)) samplerate)))
  ;; friction adds loss growing with ln(n): a per-partial law measured on
  ;; recorded fingerstyle bass, where partial 2 dies several times faster
  ;; than the fundamental although it is only an octave up.
  (def rate (+ (/ 6.907755 decay-coefficient)
    (* (+ 0.2 (* 24 damping-coefficient damping-coefficient)) (pow (/ hz 1000) 2))
    (* friction-coefficient (log n))
    (* 35 mute-coefficient mute-coefficient (+ 1 (* 0.15 n)))
    (* (- 1 held-coefficient) (/ 6.907755 release-coefficient))))
  (def radius (exp (/ (- rate) samplerate)))
  (def c (bass-coefficient (* radius (cos omega)) update))
  (def s (bass-coefficient (* radius (sin omega)) update))
  ;; Fourier coefficients of a triangular displacement, spatially smoothed
  ;; by the finger's Gaussian contact footprint. Differentiation gives ratio/n².
  (def shape (/ (* 2 (sin (* pi n pluck-coefficient))) (* pi pi n n pluck-coefficient (- 1 pluck-coefficient))))
  (def contact (exp (* -0.5 (pow (* pi n width-coefficient) 2))))
  (def bandlimit (clip (/ (- (* 0.48 samplerate) hz) (* 0.08 samplerate)) 0 1))
  (def weight (bass-coefficient (* shape contact ratios bandlimit) update))
  (def readout (bass-coefficient (* (sin (* pi n pickup-coefficient))
    (/ (sin (* 0.5 pi n aperture-coefficient)) (* 0.5 pi n aperture-coefficient))) update))
  (def x (read-tensor-history real_h))
  (def y (read-tensor-history imag_h))
  ;; A new pluck resets displacement and velocity. Note-off changes only loss,
  ;; leaving the vibrating state continuous rather than chopping its output.
  (def xn (gswitch excite (* level weight) (- (* c x) (* s y))))
  (def yn (gswitch excite 0 (+ (* s x) (* c y))))
  (write-tensor-history real_h xn)
  (write-tensor-history imag_h yn)
  ;; The fundamental is scaled by sub_gain, which ramps up after onset when
  ;; pluck.attack_ms is set: on the reference recordings the octave partial
  ;; is present from the first cycle while the fundamental takes ~40 ms to
  ;; build. The mask selects partial 1 from the mode tensor.
  (def mode_out (* yn readout))
  (- (sum mode_out) (* (- 1 sub_gain) (sum (* mode_out sub_mask)))))

(make-history sub_clock)
(def sub_age (gswitch onset 0 (+ (read-history sub_clock) 1)))
(write-history sub_clock sub_age)
(def sub_span (* 0.001 samplerate (latch pluck.attack_ms onset)))
;; Fundamental gain: 0.3 until ~55% of attack_ms, then a smooth step to 1
;; by attack_ms. Recorded fingerstyle bass shows the fundamental arriving as
;; a late step while the octave partial is full from the first cycle.
(def sub_lin (clip (/ (- (/ sub_age (max 1 sub_span)) 0.55) 0.35) 0 1))
(def sub_gain (gswitch (lt sub_span 0.5) 1
  (+ 0.3 (* 0.7 sub_lin sub_lin (- 3 (* 2 sub_lin))))))
(def string (bass-modes modes tick onset (pow hit_velocity 1.4) pluck_position finger_width
  (clip (* pitch (pow 2 (/ (clip (mod string.tune) -100 100) 1200))) 25 1200)
  (clip (mod string.damping) 0 1) (clip (mod string.friction) 0 12) (clip (mod string.mute) 0 1)
  (clip (mod string.decay_s) 0.2 12) (clip (mod string.release_s) 0.025 2)
  held (clip (mod string.stiffness) 0 1)
  (clip (mod pickup.position) 0.04 0.45) (clip (mod pickup.aperture) 0.005 0.1) sub_gain))

;; SP-303-style tempo stretch. The string plays at its authored pitch into a
;; delay line whose read tap is a staircase: constant inside each grain, then
;; stepped back by grain_len * slow% at every grain boundary, exactly as a
;; slice-repeat stretch reads the same source region again. The grain grid runs
;; free, so boundaries fall at arbitrary phases of the note, as they do when a
;; sampled loop is stretched. Each step restarts the waveform at a new phase;
;; because the crossfade is only a few milliseconds, that phase jump shows up
;; as a level dip and a faint click instead of a pitch bend. Dip depth depends
;; on pitch and step size, so different notes wobble differently. The tap
;; resets to zero at note onset so note timing is unchanged.
(def grain_len (max 2 (floor (* 0.001 samplerate (clip texture.grain_ms 20 200)))))
(make-history grain_clock)
(def grain_pos (wrap (+ (read-history grain_clock) 1) 0 grain_len))
(write-history grain_clock grain_pos)
(def boundary (* (eq grain_pos 0) (- 1 onset)))
(def step (floor (* grain_len 0.01 (clip (mod texture.slow) 0 40))))
(make-history tap_now)
(make-history tap_before)
(def tap_prev (read-history tap_now))
(def tap_cur (gswitch onset 0
  (gswitch boundary (min 95000 (+ tap_prev step)) tap_prev)))
(def switched (max onset boundary))
(def tap_old (gswitch switched tap_prev (read-history tap_before)))
(write-history tap_now tap_cur)
(write-history tap_before tap_old)
(make-history switch_clock)
(def switch_age (gswitch switched 0 (+ (read-history switch_clock) 1)))
(write-history switch_clock switch_age)
(def xfade_lin (clip (/ switch_age (max 1 (* 0.001 samplerate (clip texture.xfade_ms 0.5 30)))) 0 1))
;; Raised-cosine crossfade: a linear ramp of a few ms still reads as a click
;; on a 40-70 Hz fundamental.
(def xfade (* 0.5 (- 1 (cos (* pi xfade_lin)))))
(def stretched (+ (* (- 1 xfade) (delay string tap_old @max-delay 96000))
  (* xfade (delay string tap_cur @max-delay 96000))))
;; The tap lags wall clock by slow% of the note, so a note-off inside the
;; string would be heard late. Apply the note-off decay again after the
;; stretch, in wall-clock time; while held this gain is 1.
(make-history rel_env)
(def rel_coef (exp (/ -6.907755 (* samplerate (clip (mod string.release_s) 0.025 2)))))
(def rel_gain (gswitch (max onset held) 1 (* rel_coef (read-history rel_env))))
(write-history rel_env rel_gain)
(def gated (* stretched rel_gain))
(def tone_cut (min (* 0.4 samplerate) (clip (mod output.tone_hz) 120 10000)))
(def tone_q (clip (mod output.resonance) 0.5 1.4))
(def tone1 (svf gated tone_cut tone_q 0))
;; A second identical pole pair, blended in by steep, gives the 20+ dB/octave
;; roll-off above ~300 Hz seen on vinyl-sourced bass; 0 is the original tone.
(def tone (mix tone1 (svf tone1 tone_cut tone_q 0) (clip (mod output.steep) 0 1)))
;; Record-player floor: hiss (pinkish, band-limited) and low rumble, faded in
;; and out with the gate so idle voices are silent.
(make-history floor_env)
(def floor_gain (+ (read-history floor_env) (* 0.002 (- held (read-history floor_env)))))
(write-history floor_env floor_gain)
(def floor_noise (noise))
;; Hiss: a broad bandpass gives the flat-per-octave floor measured on the
;; reference vinyl bass samples; hiss 0.5 sits at their level.
(def hiss (* 0.011 (clip (mod vinyl.hiss) 0 1) floor_gain (svf floor_noise 2500 0.4 1)))
(def rumble (* 0.012 (clip (mod vinyl.rumble) 0 1) floor_gain (svf floor_noise 45 0.9 0)))
;; Pickup output calibration leaves headroom across the factory register.
(def signal (+ (* 0.65 tone (clip (mod output.gain) 0 1)) hiss rumble))
(out signal 1 @name left)
(out signal 2 @name right)
