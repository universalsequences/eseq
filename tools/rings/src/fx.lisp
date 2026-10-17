; ======================================================================
; Effects (rings/dsp/fx): the Griesinger reverb used by model 5 and by
; Disastrous Peace, and Disastrous Peace's chorus and ensemble. They run on
; FxEngine's 16-bit delay memory, emulated by `fx-store` on every write.
; Delay lengths and LFO rates are the module's 48 kHz values scaled to the
; host sample rate.
; ======================================================================

(def fx_lut_sine (tensor @shape [5121 1] @file "lut_sine.json"))

; Delay memory, sized for sample rates up to 96 kHz. One write counter per
; effect; `peek` wraps its index to each line's size and interpolates.
(def rv_ap1 (tensor @shape [512]))
(def rv_ap2 (tensor @shape [512]))
(def rv_ap3 (tensor @shape [1024]))
(def rv_ap4 (tensor @shape [2048]))
(def rv_dap1a (tensor @shape [8192]))
(def rv_dap1b (tensor @shape [8192]))
(def rv_del1 (tensor @shape [16384]))
(def rv_dap2a (tensor @shape [8192]))
(def rv_dap2b (tensor @shape [8192]))
(def rv_del2 (tensor @shape [16384]))
(def fx_line_l (tensor @shape [4096]))
(def fx_line_r (tensor @shape [4096]))

; Sample counter for an effect's delay lines. It wraps at 16384, a multiple
; of every line size, small enough to keep fractional read positions
; precise in float.
(defmacro fx-counter ()
  (make-history h)
  (def c (read-history h))
  (write-history h (% (+ c 1) 16384))
  c)

; FxEngine allpass on an L-sample line written at counter c: read the tail
; (L - 1 samples back), store x + g * tail (as int16), return
; tail - g * (x + g * tail).
(defmacro fx-allpass (buf c x len g)
  (def r (peek buf (- c (- (* len (/ samplerate 48000)) 1))))
  (def w (+ x (* r g)))
  (seq (poke buf c (fx-store w))
       (+ (* w (- g)) r)))

; Reverb::Process (rings/dsp/fx/reverb.h). Returns (left right).
(defmacro rg-reverb (l r amount krt gain klp)
  (make-history h_lp1)
  (make-history h_lp2)
  (def c (fx-counter))
  (def sr_scale (/ samplerate 48000))
  (def kap 0.625)
  (def lfo1 (fx-lfo 0.5))
  (def lfo2 (fx-lfo 0.3))
  (def ap_a (fx-allpass rv_ap1 c (* (+ l r) gain) 150 kap))
  (def ap_b (fx-allpass rv_ap2 c ap_a 214 kap))
  (def ap_c (fx-allpass rv_ap3 c ap_b 319 kap))
  (def apout (fx-allpass rv_ap4 c ap_c 527 kap))
  ; loop half 1: apout + del2 (written last sample and before) -> lp ->
  ; 2 allpasses -> del1
  (def d2 (peek rv_del2 (- c (* (+ 6261 (* 50 lfo2)) sr_scale))))
  (def lp1 (+ (read-history h_lp1) (* klp (- (+ apout (* d2 krt)) (read-history h_lp1)))))
  (write-history h_lp1 lp1)
  (def w1 (fx-allpass rv_dap1b c (fx-allpass rv_dap1a c lp1 2182 (- kap)) 2690 kap))
  ; loop half 2: apout + del1 (including this sample's write) -> lp ->
  ; 2 allpasses -> del2
  (def d1 (seq (poke rv_del1 c (fx-store w1))
               (peek rv_del1 (- c (* (+ 4460 (* 40 lfo1)) sr_scale)))))
  (def lp2 (+ (read-history h_lp2) (* klp (- (+ apout (* d1 krt)) (read-history h_lp2)))))
  (write-history h_lp2 lp2)
  (def w2 (fx-allpass rv_dap2b c (fx-allpass rv_dap2a c lp2 2525 kap) 2197 (- kap)))
  (def out_r (seq (poke rv_del2 c (fx-store w2))
                  (+ r (* (- (* 2 w2) r) amount))))
  (tuple (+ l (* (- (* 2 w1) l) amount)) out_r))

; stmlib Interpolate(lut_sine, phase, 4096) (`peek` interpolates).
(defmacro fx-sine (phase)
  (peek fx_lut_sine (* phase 4096) 0))

; LFO phase in [0, 1), `inc` per 48 kHz sample.
(defmacro fx-phase (inc)
  (make-history h)
  (def p (rg-wrap (+ (read-history h) (* inc (/ 48000 samplerate)))))
  (write-history h p)
  p)

; Disastrous Peace's Chorus (rings/dsp/fx/chorus.h, `chorus` = 1) or
; Ensemble (rings/dsp/fx/ensemble.h, `chorus` = 0) on shared delay lines:
; the chorus writes (l + r) / 2 to the left line and reads it 4 times, the
; ensemble writes l and r and reads them 6 times. Returns (left right).
(defmacro rg-modfx (l r chorus amount depth)
  (def c (fx-counter))
  (def sr_scale (/ samplerate 48000))
  (def dry (- 1 (* amount 0.5)))
  ; chorus LFOs (interpolated lut_sine)
  (def cp1 (fx-phase 0.00000417))
  (def cp2 (fx-phase 0.000005417))
  (def cdp (* depth 384))
  (def c_t1 (+ (* (fx-sine cp1) cdp) 1200))
  (def c_t2 (+ (* (fx-sine cp2) cdp) 800))
  (def c_t3 (+ (* (fx-sine (+ cp1 0.25)) cdp) 800))
  (def c_t4 (+ (* (fx-sine (+ cp2 0.25)) cdp) 1200))
  ; ensemble LFOs (lut_sine without interpolation)
  (def ep1 (fx-phase 0.0000157))
  (def ep2 (fx-phase 0.000137))
  (def phi1 (floor (* ep1 4096)))
  (def phi2 (floor (* ep2 4096)))
  (def ea (* depth 128))
  (def eb (* depth 12.8))
  (def e_m1 (+ 1024 (* (peek fx_lut_sine phi1 0) ea) (* (peek fx_lut_sine phi2 0) eb)))
  (def e_m2 (+ 1024 (* (peek fx_lut_sine (% (+ phi1 1365) 4096) 0) ea) (* (peek fx_lut_sine (% (+ phi2 1365) 4096) 0) eb)))
  (def e_m3 (+ 1024 (* (peek fx_lut_sine (% (+ phi1 2730) 4096) 0) ea) (* (peek fx_lut_sine (% (+ phi2 2730) 4096) 0) eb)))
  ; read slots: wet_l = s1 + s2 + s3, wet_r = s4 + s5 + s6
  (def t1 (gswitch chorus c_t1 e_m1))
  (def t2 (gswitch chorus c_t2 e_m2))
  (def t4 (gswitch chorus c_t3 e_m1))
  (def t5 (gswitch chorus c_t4 e_m2))
  (def xl (fx-store (gswitch chorus (+ (* l 0.5) (* r 0.5)) l)))
  (def xr (fx-store r))
  (def wl (seq (poke fx_line_l c xl)
               (poke fx_line_r c xr)
               (+ (* (gswitch chorus 0.5 0.33)
                     (+ (peek fx_line_l (- c (* t1 sr_scale))) (peek fx_line_l (- c (* t2 sr_scale)))))
                  (* (- 1 chorus) 0.33 (peek fx_line_r (- c (* e_m3 sr_scale)))))))
  (def wr (+ (* 0.5 chorus (+ (peek fx_line_l (- c (* t4 sr_scale))) (peek fx_line_l (- c (* t5 sr_scale)))))
             (* (- 1 chorus) 0.33 (+ (peek fx_line_r (- c (* e_m1 sr_scale))) (peek fx_line_r (- c (* e_m2 sr_scale)))
                                     (peek fx_line_l (- c (* e_m3 sr_scale)))))))
  (tuple (+ (* wl amount) (* l dry)) (+ (* wr amount) (* r dry))))
