; ======================================================================
; Elements common helpers (stmlib / elements dsp primitives)
; ======================================================================
; Conventions:
; - Elements runs at 32 kHz in 16-sample (0.5 ms) blocks. This port keeps
;   0.5 ms blocks at any host rate: `blk` = round(16 / kr) samples (16 at
;   32 kHz, 24 at 48 kHz), `bstart` is 1 on the first sample of a block,
;   `bphase` counts 0 .. blk - 1, and `tb` = blk kr / 16 rescales per-block
;   rates for the rounding. Block-rate values are latched with (el-hold x),
;   so DGen schedules their pure math once per block.
; - `kr` = 32000 / samplerate. Frequencies the module computes as cycles
;   per 32 kHz sample ("f32") are kept in that unit for every decision the
;   original makes on them (mode cut-offs, table lookups) and converted to
;   the host rate only where they drive a filter, phase or delay. Filters
;   keep the analog response the 32 kHz filter has (see el-g-host), and
;   per-sample / per-block decay coefficients keep their time constants.

; Elements is nearly all feedback (resonators, strings, reverb) gated by
; model: one sample loop keeps its values in registers instead of passing
; them between many small frame loops through scratch buffers.
(scalar-loop)

(def kr (/ 32000 samplerate))
(def blk (floor (+ (/ 16 kr) 0.5)))
(def tb (/ (* blk kr) 16))

; Value of `x` latched on the first sample of each block. The sample-gate
; computes `x` on that sample only (a bare latch would compute it every
; sample and keep one). Adding (* bstart 0) makes a constant `x` a signal,
; which latch requires.
(defmacro el-hold (x)
  (latch (sample-gate bstart (+ x (* bstart 0))) bstart))

; Value of `x` on the samples where `clock` is set, held in a history. For
; code inside a block-gate: a sample-gate there (as in el-hold) lets part of
; the gated code run while the gate is closed.
(defmacro el-hold-on (x clock)
  (make-history h)
  (def v (gswitch clock x (read-history h)))
  (write-history h v)
  v)

; Block counter step: phase + 1, back to 0 at `blk` (no fmod).
(defmacro el-block-step (phase)
  (def n (+ phase 1))
  (gswitch (gte n blk) 0 n))

; History cell read that starts at `init` (histories start at 0).
(defmacro el-rd (h ready init)
  (gswitch (read-history ready) (read-history h) init))

; C's int() cast: truncate toward zero.
(defmacro el-trunc (x)
  (* (sign x) (floor (abs x))))

; stmlib SemitonesToRatio: the lookup truncates the fractional semitone to
; 1/256 (int cast, so toward zero for the whole part as well).
(defmacro el-semi-ratio (semi)
  (def p (+ semi 128))
  (def whole (floor p))
  (exp (* 0.05776226505 (+ (- whole 128) (/ (floor (* (- p whole) 256)) 256)))))

; stmlib Interpolate(table, index, size) on a [N 1] tensor (index >= 0).
(defmacro el-lut (table index size)
  (def x (* index size))
  (def i (floor x))
  (def f (- x i))
  (def a (peek table i 0))
  (def b (peek table (+ i 1) 0))
  (+ a (* (- b a) f)))

; Table entry at the int cast of index * size.
(defmacro el-lut-int (table index size)
  (peek table (floor (* index size)) 0))

; Wrap to [0, 1).
(defmacro el-wrap (x)
  (- x (floor x)))

; Non-negative integer x modulo n.
(defmacro el-imod (x n)
  (- x (* n (floor (/ x n)))))

; stmlib OnePole::tan approximations (f in cycles per sample).
(defmacro el-tan-fast (f)
  (def f2 (* f f))
  (* f (+ 3.14159265 (* f2 (+ 10.108046 (* 55.787389 f2))))))
(defmacro el-tan-accurate (f)
  (def f2 (* f f))
  (* f (+ 3.14159265 (* f2 (+ 10.3353657 (* f2 (+ 40.8207 (* f2 (+ 161.205313 (* f2 (+ 86.462038 (* f2 2799.883476))))))))))))

; Host-rate SVF coefficient with the analog response of a 32 kHz TPT SVF
; whose coefficient is g32: the same prewarped cut-off, so tan(kr*atan(g)).
(defmacro el-g-host (g32)
  (tan (* kr (atan g32))))

; A per-sample one-pole coefficient defined at 32 kHz, at the host rate.
(defmacro el-coef (c)
  (- 1 (pow (max (- 1 c) 0) kr)))

; A per-block one-pole coefficient defined for 32 kHz blocks.
(defmacro el-coef-block (c)
  (- 1 (pow (max (- 1 c) 0) tb)))

; stmlib Svf (TPT state-variable filter): coefficients g, r = 1/q, h; returns
; (lp bp hp).
(defmacro el-svf (in g r h)
  (make-history s1)
  (make-history s2)
  (def st1 (read-history s1))
  (def st2 (read-history s2))
  (def hp (* (- in (* r st1) (* g st1) st2) h))
  (def bp (+ (* g hp) st1))
  (def lp (+ (* g bp) st2))
  (write-history s1 (+ (* g hp) bp))
  (write-history s2 (+ (* g bp) lp))
  (tuple lp bp hp))

(defmacro el-svf-h (g r)
  (/ 1 (+ 1 (* r g) (* g g))))

; stmlib SoftLimit.
(defmacro el-soft-limit (x)
  (/ (* x (+ 27 (* x x))) (+ 27 (* 9 x x))))

; One previous sample of `x` (independent noise taps of one source).
(defmacro el-noise-tap (x)
  (make-history h)
  (def previous (read-history h))
  (write-history h x)
  previous)

; stmlib ParameterInterpolator over one block, from a state that starts at
; `init`: at the block start the increment becomes (target - state) / blk and
; the state advances by it on every sample (accumulated, as in the C++).
; Histories start at 0, so they store offsets from `init`. With `warm_start`
; (the instrument) the state starts at the first block's target instead, as
; on the running module, where a new note never meets power-on states.
(defmacro el-interp (target init)
  (make-history h_state)
  (make-history h_inc)
  (make-history h_ready)
  (def st0 (gswitch (* warm_start (- 1 (read-history h_ready))) (- target init) (read-history h_state)))
  (write-history h_ready 1)
  (def inc (gswitch bstart (/ (- target init st0) blk) (read-history h_inc)))
  (def st (+ st0 inc))
  (write-history h_inc inc)
  (write-history h_state st)
  (+ init st))

; FxEngine 16-bit delay memory: truncate to int16 / 32768.
(defmacro el-store16 (x)
  (/ (el-trunc (clip (* x 32768) -32768 32767)) 32768))
