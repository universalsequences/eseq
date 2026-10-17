; ======================================================================
; Reverb (elements/dsp/fx/reverb.h): Griesinger/Dattorro topology on the
; module's FxEngine with 16-bit delay memory. Four input all-passes (the
; first smeared by LFO 1), then two loop halves of lowpass + two all-passes
; + a long delay, the second read modulated by LFO 2.
; Inputs: left right (in place in the C++), amount, input_gain, time,
; diffusion, lp (block values). Returns (tuple left right).
; Host-rate rules: line lengths and LFO rates are kept in time; the loop
; low-pass keeps its 32 kHz time constant.
; ======================================================================

; FxEngine LFO: stmlib's approximate cosine recurrence at `hz`, stepped on
; every 32nd sample (when the engine's write pointer is a multiple of 32:
; samples 31, 63, ...). Returns 0..1, starting at its peak.
(defmacro rev-lfo (hz)
  (make-history h_y0)
  (make-history h_y1)
  (make-history h_count)
  (make-history ready)
  (def coef (res-cos-coef (/ (* hz 32) samplerate)))
  (def started (read-history ready))
  (def y0 (gswitch started (read-history h_y0) 0.5))
  (def y1 (gswitch started (read-history h_y1) (* coef 0.25)))
  (def count (+ (read-history h_count) 1))
  (def step (eq (el-imod count 32) 0))
  (def y0n (gswitch step (- (* coef y0) y1) y0))
  (def y1n (gswitch step y0 y1))
  (write-history ready 1)
  (write-history h_count (el-imod count 32))
  (write-history h_y0 y0n)
  (write-history h_y1 y1n)
  (+ y1n 0.5))

; The FxEngine memory: every line in one ring, at the module's bases (each
; line starts one slot past the previous line's end), one write pointer that
; steps back a slot per sample. Line lengths and offsets are kept in time
; (rounded to whole host samples); the ring holds them up to 6x 32 kHz.
; (The same ring as content/effects/Beads/dsp.lisp's FxEngine reverb.)
(def rv_size 131072)
; rv_size slots
(def rv_mem (tensor @shape [131072 1]))
(def rv_k (/ 1 kr))
(defmacro rv-len (n)
  (floor (+ (* n rv_k) 0.5)))
(def rvl_ap1 (rv-len 150))
(def rvl_ap2 (rv-len 214))
(def rvl_ap3 (rv-len 319))
(def rvl_ap4 (rv-len 527))
(def rvl_dap1a (rv-len 2182))
(def rvl_dap1b (rv-len 2690))
(def rvl_del1 (rv-len 4501))
(def rvl_dap2a (rv-len 2525))
(def rvl_dap2b (rv-len 2197))
(def rvb_ap2 (+ rvl_ap1 1))
(def rvb_ap3 (+ rvb_ap2 rvl_ap2 1))
(def rvb_ap4 (+ rvb_ap3 rvl_ap3 1))
(def rvb_dap1a (+ rvb_ap4 rvl_ap4 1))
(def rvb_dap1b (+ rvb_dap1a rvl_dap1a 1))
(def rvb_del1 (+ rvb_dap1b rvl_dap1b 1))
(def rvb_dap2a (+ rvb_del1 rvl_del1 1))
(def rvb_dap2b (+ rvb_dap2a rvl_dap2a 1))
(def rvb_del2 (+ rvb_dap2b rvl_dap2b 1))

; Ring slot `offset` slots past the write pointer (0 <= offset < rv_size).
(defmacro rv-at (wp offset)
  (def j (+ wp offset))
  (gswitch (gte j rv_size) (- j rv_size) j))

; Read: the whole slot `offset` into a line.
(defmacro rv-read (wp base offset)
  (peek rv_mem (rv-at wp (+ base offset)) 0))

; Interpolate: a fractional `offset`, between two wrapped slots.
(defmacro rv-interp (wp base offset)
  (def i (floor offset))
  (def a (rv-read wp base i))
  (+ a (* (- (rv-read wp base (+ i 1)) a) (- offset i))))

; Write (int16 storage).
(defmacro rv-write (wp base offset x)
  (poke rv_mem (rv-at wp (+ base offset)) 0 (el-store16 x)))

; Read(line TAIL, g) then WriteAllPass(line, -g) on accumulator `acc`.
(defmacro rv-allpass (wp acc base len g)
  (def tail (rv-read wp base (- len 1)))
  (def a1 (+ acc (* tail g)))
  (seq (rv-write wp base 0 a1) (+ (* a1 (- g)) tail)))

(defmacro el-reverb (left right amount input_gain time diffusion lp)
  (make-history h_wp)
  (make-history h_lp1)
  (make-history h_lp2)
  (def wp0 (- (read-history h_wp) 1))
  (def wp (gswitch (lt wp0 0) (+ wp0 rv_size) wp0))
  (write-history h_wp wp)
  (def klp (el-hold (el-coef lp)))
  (def kap diffusion)
  (def lfo1 (rev-lfo 0.5))
  (def lfo2 (rev-lfo 0.3))
  ; Smear AP1 inside the loop: read at 10 + 80 lfo1, rewrite at 100.
  (def smear (rv-interp wp 0 (* (+ 10 (* 80 lfo1)) rv_k)))
  ; (seq ... 0) adds nothing; it keeps the write in the graph
  (def smeared (seq (rv-write wp 0 (rv-len 100) smear) 0))
  ; Diffuse through 4 all-passes.
  (def ap_a (rv-allpass wp (+ smeared (* (+ left right) input_gain)) 0 rvl_ap1 kap))
  (def ap_b (rv-allpass wp ap_a rvb_ap2 rvl_ap2 kap))
  (def ap_c (rv-allpass wp ap_b rvb_ap3 rvl_ap3 kap))
  (def apout (rv-allpass wp ap_c rvb_ap4 rvl_ap4 kap))
  ; half 1: apout + del2 (6211 + 100 lfo2 in) -> lp -> 2 all-passes -> del1
  (def d2 (rv-interp wp rvb_del2 (* (+ 6211 (* 100 lfo2)) rv_k)))
  (def lp1 (+ (read-history h_lp1) (* klp (- (+ apout (* d2 time)) (read-history h_lp1)))))
  (write-history h_lp1 lp1)
  (def wet_l (rv-allpass wp (rv-allpass wp lp1 rvb_dap1a rvl_dap1a (- kap)) rvb_dap1b rvl_dap1b kap))
  (def wet_l_w (seq (rv-write wp rvb_del1 0 wet_l) wet_l))
  ; half 2: apout + del1 tail -> lp -> 2 all-passes -> del2
  (def d1 (rv-read wp rvb_del1 (- rvl_del1 1)))
  (def lp2 (+ (read-history h_lp2) (* klp (- (+ apout (* d1 time)) (read-history h_lp2)))))
  (write-history h_lp2 lp2)
  (def wet_r (rv-allpass wp (rv-allpass wp lp2 rvb_dap2a rvl_dap2a kap) rvb_dap2b rvl_dap2b (- kap)))
  (def wet_r_w (seq (rv-write wp rvb_del2 0 wet_r) wet_r))
  (tuple (+ left (* (- (* 2 wet_l_w) left) amount))
         (+ right (* (- (* 2 wet_r_w) right) amount))))
