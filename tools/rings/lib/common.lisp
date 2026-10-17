; ======================================================================
; Rings common helpers (stmlib primitives)
; ======================================================================
; Conventions:
; - Rings renders 24-sample blocks (kMaxBlockSize). `bstart` is 1 on the
;   first sample of each block, `bphase` counts 0..23. Per-block controls
;   are `(latch x bstart)`: DGen then runs the pure math downstream of them
;   once per block instead of every sample (EventRatePromotionPass).
; - Frequencies are in cycles per sample.

; stmlib SemitonesToRatio: the lookup truncates the fractional semitone to
; 1/256.
(defmacro rg-semi-ratio (semi)
  (def whole (floor semi))
  (exp (* 0.05776226505 (+ whole (/ (floor (* (- semi whole) 256)) 256)))))

; Rings' SemitonesToRatio(note - 69) * a3 (cycles per sample).
(defmacro rg-note-to-freq (note)
  (* (/ 440 samplerate) (rg-semi-ratio (- note 69))))

; Value of `x` on the first sample of each block, held across the block.
; `latch` lets DGen run the pure math downstream once per block (event
; rate), but an event latch may not feed a block-gate: inside the gated
; models use `rg-hold`, a plain history sample-and-hold.
(defmacro rg-hold (x bstart)
  (make-history h)
  (def v (gswitch bstart x (read-history h)))
  (write-history h v)
  v)

; stmlib ParameterInterpolator(&state, target, 24) with the state
; initialised to `init`: state + (target - state) * (i + 1) / 24 on block
; sample i; the state becomes `target` at the end of the block. Histories
; start at 0, so they store offsets from `init`.
(defmacro rg-interp (target bstart bphase init)
  (make-history h_prev)
  (make-history h_cur)
  (def prev (gswitch bstart (read-history h_cur) (read-history h_prev)))
  (def cur (gswitch bstart (- target init) (read-history h_cur)))
  (write-history h_prev prev)
  (write-history h_cur cur)
  (+ init prev (* (- cur prev) (/ (+ bphase 1) 24))))

; OnePole::tan approximations (f in cycles per sample).
(defmacro rg-tan-dirty (f)
  (* f (+ 3.14159265 (* 11.583945 f f))))
(defmacro rg-tan-fast (f)
  (def f2 (* f f))
  (* f (+ 3.14159265 (* f2 (+ 10.108046 (* 55.787389 f2))))))
(defmacro rg-tan-accurate (f)
  (def f2 (* f f))
  (* f (+ 3.14159265 (* f2 (+ 10.335366 (* f2 (+ 40.820700 (* f2 (+ 161.205313 (* f2 (+ 86.462038 (* f2 2799.883476))))))))))))

; stmlib Svf with coefficient g and damping r = 1/q; returns (lp bp hp).
(defmacro rg-svf (in g r)
  (make-history s1)
  (make-history s2)
  (def h (/ 1 (+ 1 (* r g) (* g g))))
  (def st1 (read-history s1))
  (def st2 (read-history s2))
  (def hp (* (- in (* r st1) (* g st1) st2) h))
  (def bp (+ (* g hp) st1))
  (def lp (+ (* g bp) st2))
  (write-history s1 (+ (* g hp) bp))
  (write-history s2 (+ (* g bp) lp))
  (tuple lp bp hp))

; stmlib ONE_POLE(state, in, coefficient).
(defmacro rg-one-pole (in coefficient)
  (make-history h)
  (def v (+ (read-history h) (* coefficient (- in (read-history h)))))
  (write-history h v)
  v)

; stmlib SLOPE(state, in, up, down), state initialised to `init`.
(defmacro rg-slope (in up down init)
  (make-history h)
  (def s (+ init (read-history h)))
  (def err (- in s))
  (def v (+ s (* err (gswitch (gt err 0) up down))))
  (write-history h (- v init))
  v)

; stmlib DCBlocker(pole) on every sample.
(defmacro rg-dc-block (x pole)
  (make-history h_x)
  (make-history h_y)
  (def y (+ (* (read-history h_y) pole) x (- (read-history h_x))))
  (write-history h_x x)
  (write-history h_y y)
  y)

; stmlib SoftLimit.
(defmacro rg-soft-limit (x)
  (/ (* x (+ 27 (* x x))) (+ 27 (* 9 x x))))

; PolyBLEP residuals; t = fraction of the sample elapsed since the edge.
(defmacro rg-this-blep (t)
  (* 0.5 t t))
(defmacro rg-next-blep (t)
  (def u (- 1 t))
  (* -0.5 u u))

; One previous sample of `x`. Identical `(noise)` expressions are merged into
; one node, so extra independent uniform draws come from delayed taps.
(defmacro rg-noise-tap (x)
  (make-history h)
  (def previous (read-history h))
  (write-history h x)
  previous)

; stmlib Interpolate(table, index, size) on a [N 1] tensor.
(defmacro rg-lut (table index size)
  (def x (* index size))
  (def i (floor x))
  (def f (- x i))
  (def a (peek table i 0))
  (def b (peek table (+ i 1) 0))
  (+ a (* (- b a) f)))

; C's int() cast: truncate toward zero.
(defmacro rg-trunc (x)
  (* (sign x) (floor (abs x))))

; Wrap to [0, 1).
(defmacro rg-wrap (x)
  (- x (floor x)))

; Non-negative x modulo n.
(defmacro rg-imod (x n)
  (- x (* n (floor (/ x n)))))

; stmlib CosineOscillator (approximate) recurrence coefficient for a
; frequency in cycles per step.
(defmacro rg-cos-coef (frequency)
  (def f_raw (- frequency 0.25))
  (def f_fold (gswitch (lt f_raw 0) (- f_raw) (gswitch (gt f_raw 0.5) (- f_raw 0.5) f_raw)))
  (def sign_c (gswitch (lt f_raw 0) 16 (gswitch (gt f_raw 0.5) 16 -16)))
  (* sign_c f_fold (- 1 (* 2 f_fold))))

; FxEngine stores its delay memory as int16: clip and truncate every write.
(defmacro fx-store (x)
  (/ (rg-trunc (clip (* x 32768) -32768 32767)) 32768))

; FxEngine LFO: the approximate cosine recurrence, stepped once every 32
; samples (on the 32nd sample, as FxEngine's write pointer counts down).
; Returns 0..1, starting at its peak.
(defmacro fx-lfo (hz)
  (make-history h_y0)
  (make-history h_y1)
  (make-history h_count)
  (make-history h_started)
  (def coef (rg-cos-coef (* hz (/ 32 samplerate))))
  (def started (read-history h_started))
  (def y0 (gswitch started (read-history h_y0) 0.5))
  (def y1 (gswitch started (read-history h_y1) (* coef 0.25)))
  (def count (+ (read-history h_count) 1))
  (def step (eq (% count 32) 0))
  (def y0n (gswitch step (- (* coef y0) y1) y0))
  (def y1n (gswitch step y0 y1))
  ; (max started 1), not 1: a constant write can be scheduled ahead of
  ; the read in another loop, which then never sees the initial 0.
  (write-history h_started (max started 1))
  (write-history h_count (% count 32))
  (write-history h_y0 y0n)
  (write-history h_y1 y1n)
  (+ y1n 0.5))
