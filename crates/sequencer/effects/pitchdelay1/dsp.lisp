(def in_l (in 1 @name left))
(def in_r (in 2 @name right))

(param shift @default 7.0 @min -24.0 @max 24.0 @unit semitones)
(param fine @default 9.0 @min -50.0 @max 50.0 @unit cents)
(param window_ms @default 95.0 @min 25.0 @max 220.0 @unit ms)
(param delay_ms @default 180.0 @min 0.0 @max 900.0 @unit ms)
(param feedback @default 0.24 @min 0.0 @max 0.82)
(param tone @default 9500.0 @min 1200.0 @max 18000.0 @unit Hz)
(param width @default 1.0 @min 0.0 @max 1.4)
(param mix @default 0.45 @min 0.0 @max 1.0)
(param output @default 1.0 @min 0.25 @max 2.0)

(def window_samps (* window_ms 48.0))
(def base_samps 64.0)
(def echo_samps (* delay_ms 48.0))

(def shift_l (+ shift (/ fine 100.0)))
(def shift_r (- shift (/ fine 100.0)))
(def ratio_l (pow 2.0 (/ shift_l 12.0)))
(def ratio_r (pow 2.0 (/ shift_r 12.0)))

(def diff_l (max (- ratio_l 1.0) (- 1.0 ratio_l)))
(def diff_r (max (- ratio_r 1.0) (- 1.0 ratio_r)))
(def rate_l (max 0.001 (/ (* 1000.0 diff_l) window_ms)))
(def rate_r (max 0.001 (/ (* 1000.0 diff_r) window_ms)))

(def pos_l (min 1.0 (max 0.0 (* 1000.0 (- ratio_l 1.0)))))
(def neg_l (min 1.0 (max 0.0 (* 1000.0 (- 1.0 ratio_l)))))
(def zero_l (- 1.0 (max pos_l neg_l)))
(def pos_r (min 1.0 (max 0.0 (* 1000.0 (- ratio_r 1.0)))))
(def neg_r (min 1.0 (max 0.0 (* 1000.0 (- 1.0 ratio_r)))))
(def zero_r (- 1.0 (max pos_r neg_r)))

(defmacro echo_fb (sig fbk dly)
  (make-history h)
  (write-history h (tanh (+ sig (* fbk (delay (read-history h) dly))))))

(def src_l (echo_fb in_l feedback echo_samps))
(def src_r (echo_fb in_r feedback echo_samps))

(def ph_l (phasor rate_l))
(def ph_l_b (wrap (+ ph_l 0.5) 0.0 1.0))
(def ph_r (phasor rate_r))
(def ph_r_b (wrap (+ ph_r 0.5) 0.0 1.0))

(def env_l_a (sin (* (/ twopi 2.0) ph_l)))
(def env_l_b (sin (* (/ twopi 2.0) ph_l_b)))
(def env_r_a (sin (* (/ twopi 2.0) ph_r)))
(def env_r_b (sin (* (/ twopi 2.0) ph_r_b)))

(def up_l_a (delay src_l (+ base_samps (* window_samps (- 1.0 ph_l)))))
(def up_l_b (delay src_l (+ base_samps (* window_samps (- 1.0 ph_l_b)))))
(def dn_l_a (delay src_l (+ base_samps (* window_samps ph_l))))
(def dn_l_b (delay src_l (+ base_samps (* window_samps ph_l_b))))
(def up_r_a (delay src_r (+ base_samps (* window_samps (- 1.0 ph_r)))))
(def up_r_b (delay src_r (+ base_samps (* window_samps (- 1.0 ph_r_b)))))
(def dn_r_a (delay src_r (+ base_samps (* window_samps ph_r))))
(def dn_r_b (delay src_r (+ base_samps (* window_samps ph_r_b))))

(def up_l (/ (+ (* up_l_a env_l_a) (* up_l_b env_l_b)) (+ env_l_a env_l_b 0.0001)))
(def dn_l (/ (+ (* dn_l_a env_l_a) (* dn_l_b env_l_b)) (+ env_l_a env_l_b 0.0001)))
(def up_r (/ (+ (* up_r_a env_r_a) (* up_r_b env_r_b)) (+ env_r_a env_r_b 0.0001)))
(def dn_r (/ (+ (* dn_r_a env_r_a) (* dn_r_b env_r_b)) (+ env_r_a env_r_b 0.0001)))

(def pitch_l (+ (* zero_l src_l) (* pos_l up_l) (* neg_l dn_l)))
(def pitch_r (+ (* zero_r src_r) (* pos_r up_r) (* neg_r dn_r)))

(def tone_l (biquad pitch_l tone 0.707 1 0))
(def tone_r (biquad pitch_r tone 0.707 1 0))
(def mono_wet (* 0.5 (+ tone_l tone_r)))
(def wide_l (+ (* mono_wet (- 1.0 width)) (* tone_l width)))
(def wide_r (+ (* mono_wet (- 1.0 width)) (* tone_r width)))

(def out_l (* output (+ (* in_l (- 1.0 mix)) (* wide_l mix))))
(def out_r (* output (+ (* in_r (- 1.0 mix)) (* wide_r mix))))

(out out_l 1 @name left)
(out out_r 2 @name right)
