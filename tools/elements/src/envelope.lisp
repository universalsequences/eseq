; ======================================================================
; MultistageEnvelope (elements/dsp/multistage_envelope.h) in its ADSR
; configuration (set_adsr: quartic attack, exponential decay and release,
; sustain point 2), evaluated once per block. `rising`, `falling` and `gate`
; are the block's exciter flags; a, dr, sustain the block's ADSR settings.
; Returns the envelope value, constant over a block. Per-block phase
; increments are scaled by tb so the times hold at any host rate.
; ======================================================================
(def env_lut_increments (tensor @shape [258 1] @file "lut_env_increments.json"))
(def env_lut_expo (tensor @shape [258 1] @file "lut_env_expo.json"))
(def env_lut_quartic (tensor @shape [258 1] @file "lut_env_quartic.json"))

; Interpolate8 (index * 256, linear).
(defmacro env-lut8 (table x)
  (el-lut table x 256))

(defmacro el-envelope (rising falling gate_f a dr sustain)
  (make-history ready)
  (make-history h_seg)
  (make-history h_start)
  (make-history h_val)
  (make-history h_ph)
  ; Init: segment_ = num_segments_ (done), everything else 0.
  (def seg0 (el-rd h_seg ready 3))
  (def val0 (read-history h_val))
  (def start0 (read-history h_start))
  (def ph0 (read-history h_ph))
  (def c_r rising)
  (def c_f (* (- 1 rising) falling))
  (def c_p (* (- 1 rising) (- 1 falling) (gte ph0 1)))
  (def seg_next (min (+ seg0 1) 3))
  (def lev_next0 (selector (+ seg_next 1) 0 1 sustain 0))
  (def seg1 (gswitch c_r 0 (gswitch c_f 2 (gswitch c_p seg_next seg0))))
  (def start1 (gswitch c_r (gswitch (eq seg0 3) 0 val0)
                (gswitch c_f val0 (gswitch c_p lev_next0 start0))))
  (def ph1 (gswitch (max c_r (max c_f c_p)) 0 ph0))
  (def done (eq seg1 3))
  (def sustained (* (eq seg1 2) gate_f))
  (def time (selector (+ seg1 1) a dr dr 0))
  (def inc (* (- 1 sustained) (- 1 done) tb (env-lut8 env_lut_increments time)))
  (def t (selector (+ seg1 1) (env-lut8 env_lut_quartic ph1) (env-lut8 env_lut_expo ph1)
                   (env-lut8 env_lut_expo ph1) ph1))
  (def target (selector (+ seg1 2) 0 1 sustain 0 0))
  (def val1 (+ start1 (* (- target start1) t)))
  (def ph2 (+ ph1 inc))
  (write-history ready 1)
  (write-history h_seg (gswitch bstart seg1 seg0))
  (write-history h_start (gswitch bstart start1 start0))
  (write-history h_val (gswitch bstart val1 val0))
  (write-history h_ph (gswitch bstart ph2 ph0))
  (gswitch bstart val1 val0))
