; ======================================================================
; Plaits common helpers (stmlib / plaits dsp primitives)
; ======================================================================
; Conventions shared by every engine macro:
; - Plaits renders 12-sample blocks (kBlockSize). `bstart` is 1 on the first
;   sample of each block, `bphase` counts 0..11. Engine parameters (note,
;   timbre, morph, harmonics, accent, trigger flags) are block-latched by the
;   caller: constant across a block, like EngineParameters.
; - Frequencies are in cycles per sample. `pl_ref_tune` is 1 in the
;   instrument; engine test harnesses set it to 48000/47872.34 so the port's
;   pitch matches the module's corrected a0 when rendered at 48 kHz.

; stmlib SemitonesToRatio: the lookup truncates the fractional semitone to
; 1/256.
(defmacro pl-semi-ratio (semi)
  (def whole (floor semi))
  (exp (* 0.05776226505 (+ whole (/ (floor (* (- semi whole) 256)) 256)))))

; plaits NoteToFrequency (cycles per sample).
(defmacro pl-note-to-freq (note)
  (* (/ 13.75 samplerate) pl_ref_tune (pl-semi-ratio (clip (- note 9) -128 127))))

; C's int() cast: truncate toward zero.
(defmacro pl-trunc (x)
  (* (sign x) (floor (abs x))))

; Value of `x` latched on the first sample of each block.
(defmacro pl-hold (x bstart)
  (make-history h)
  (def v (gswitch bstart x (read-history h)))
  (write-history h v)
  v)

; stmlib ParameterInterpolator(&state, target, kBlockSize) with the state
; initialised to `init`: returns state + (target - state) * (i + 1) / 12 on
; block sample i; the state becomes `target` at the end of the block.
; Histories start at 0, so they store offsets from `init`.
(defmacro pl-interp (target bstart bphase init)
  (make-history h_prev)
  (make-history h_cur)
  (def prev (gswitch bstart (read-history h_cur) (read-history h_prev)))
  (def cur (gswitch bstart (- target init) (read-history h_cur)))
  (write-history h_prev prev)
  (write-history h_cur cur)
  (+ init prev (* (- cur prev) (/ (+ bphase 1) 12))))

; OnePole::tan approximations (f in cycles per sample).
(defmacro pl-tan-dirty (f)
  (* f (+ 3.14159265 (* 11.583945 f f))))
(defmacro pl-tan-fast (f)
  (def f2 (* f f))
  (* f (+ 3.14159265 (* f2 (+ 10.108046 (* 55.787389 f2))))))
(defmacro pl-tan-accurate (f)
  (def f2 (* f f))
  (* f (+ 3.14159265 (* f2 (+ 10.335366 (* f2 (+ 40.820700 (* f2 (+ 161.205313 (* f2 (+ 86.462038 (* f2 2799.883476))))))))))))
(defmacro pl-tan-exact (f)
  (tan (* 3.14159265 (min f 0.497))))

; stmlib Svf with coefficient g and damping r = 1/q; returns (lp bp hp).
; BAND_PASS_NORMALIZED is bp * r.
(defmacro pl-svf (in g r)
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

; stmlib ONE_POLE(state, in, coefficient): state += coefficient * (in - state).
(defmacro pl-one-pole (in coefficient)
  (make-history h)
  (def v (+ (read-history h) (* coefficient (- in (read-history h)))))
  (write-history h v)
  v)

; stmlib SLOPE(state, in, up, down).
(defmacro pl-slope (in up down)
  (make-history h)
  (def s (read-history h))
  (def err (- in s))
  (def v (+ s (* err (gswitch (gt err 0) up down))))
  (write-history h v)
  v)

; PolyBLEP residuals (stmlib/dsp/polyblep.h); t = fraction of the sample
; elapsed since the discontinuity.
(defmacro pl-this-blep (t)
  (* 0.5 t t))
(defmacro pl-next-blep (t)
  (def u (- 1 t))
  (* -0.5 u u))
(defmacro pl-next-integrated-blep (t)
  (def t1 (* 0.5 t))
  (def t2 (* t1 t1))
  (+ (- 0.1875 t1) (* 1.5 t2) (- (* t2 t2))))
(defmacro pl-this-integrated-blep (t)
  (pl-next-integrated-blep (- 1 t)))

; One previous sample of `x`. Identical `(noise)` expressions are merged into
; one node, so extra independent uniform draws come from delayed taps of one
; noise source: (pl-noise-tap u0), (pl-noise-tap (pl-noise-tap u0)), ...
(defmacro pl-noise-tap (x)
  (make-history h)
  (def previous (read-history h))
  (write-history h x)
  previous)

; stmlib Interpolate(table, index, size) on a [N 1] tensor: linear lookup at
; index * size (index >= 0).
(defmacro pl-lut (table index size)
  (def x (* index size))
  (def i (floor x))
  (def f (- x i))
  (def a (peek table i 0))
  (def b (peek table (+ i 1) 0))
  (+ a (* (- b a) f)))

; Wrap to [0, 1).
(defmacro pl-wrap (x)
  (- x (floor x)))

; plaits Downsampler (4x FIR, lut_4x_downsampler_fir = 0.02442415 0.09297315
; 0.16712938 0.21547332): pass the four oversampled values s0..s3 of one
; output sample.
(defmacro pl-downsample4 (s0 s1 s2 s3)
  (make-history h_tail)
  (def head (+ (read-history h_tail)
               (* s0 0.21547332) (* s1 0.16712938) (* s2 0.09297315) (* s3 0.02442415)))
  (write-history h_tail (+ (* s0 0.02442415) (* s1 0.09297315) (* s2 0.16712938) (* s3 0.21547332)))
  head)
