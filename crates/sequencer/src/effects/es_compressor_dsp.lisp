; ES Compressor: Punch / Level / Sustain. Independently authored, asset-free.
; Behavioral design, identification method and limitations:
; crates/sequencer/docs/es-compressor-effect-spec.md.
; Punch: broad soft knee, weighted RMS detector and short recovery reservoir.
; Level: frequency-sensitive detector and two linear-gain relaxation populations.
; Both are independently authored models fitted to black-box audio measurements,
; not transcriptions of reference circuits, code or calibration tables.
; Sustain: the existing ES history/error-dependent grab-and-settle controller.
; All paths stay warm; mode crossfades aligned processed audio.
; 8-384 kHz, fixed per instance. No FFT, external assets or oversampling.
; A common short delay aligns every mode and the conventional dry/wet path.
(effect-latency (max 1 (min 127 (round (* 0.00025 samplerate)))))

(def left (in 1 @name Left))
(def right (in 2 @name Right))
(param mode @options ["Punch" "Level" "Sustain"] @default 0)
(param amount @min 0 @max 100 @default 50)
(param tone @min 0 @max 100 @default 0)
(param attack @min 1 @max 200 @default 40 @unit ms)
(param release @min 20 @max 2000 @default 120 @unit ms)
(param mix @min 0 @max 1 @default 1)
(param drive @min 0 @max 24 @default 0 @unit dB)
(param input-db @min -24 @max 24 @default 0 @unit dB)
(param output-db @min -48 @max 18 @default 0 @unit dB)
(param detector-db @min -36 @max 36 @default 0 @unit dB)

; ------------------------------------------------------------------ TUNING
; Static curve. u is amount mapped to 0..1 ("sustain").
(def threshold-top-db -36)      ; threshold at amount 0
(def threshold-span-db -30)     ; added by amount 100 (-> -66 dB)
(def slope-min 0.6)             ; initial dB reduction per dB over threshold, amount 0
(def slope-span 0.3)            ; added by amount 100 (-> 0.9)
(def knee-db 6)                 ; soft-knee width centred on the threshold
; The reduction curve saturates: reduction = max * (1 - exp(-slope*over/max)).
; A low ceiling lifts quiet tails hard while loud transients get little extra
; reduction, so drums keep their punch. Amount raises the ceiling toward a
; near-linear curve (maximum sustain, less punch).
(def reduction-max-min-db 8)    ; ceiling at amount 0
(def reduction-max-octaves 5)   ; ceiling doubles this many times by amount 100 (-> 256 dB)
(def unity-level-db -23)        ; input level that comes out at the same level
; Detector windows, in milliseconds.
(def peak-window-ms 0.25)       ; sliding-max lookahead
(def history-ms 1.0)            ; reduction memory the envelope compares against
; Envelope. Time constants shrink exponentially with the error against the
; recent reduction, and the aim overshoots the target so the gain grabs and
; settles instead of creeping. Grip (= sustain squared) scales the overshoot.
; Crunch is gated by absolute level: only material inside the hot window near
; full scale gets the fast time constants that let the gain ride inside a
; bass or kick waveform. Quieter material (chords, tails) keeps the slow,
; smooth envelope, so the effect reads as compression, not distortion.
; The attack/release knobs are the EFFECTIVE small-error time constants; the
; overshoot factor is compensated internally so raising grip does not speed
; the envelope up on its own.
(def attack-speedup-db 4)       ; every 4 dB of error makes attack e times faster
(def attack-min-ms 0.05)
(def release-speedup-db 8)
(def overshoot-cap-db 18)       ; the aim never leads the target by more than this
(def overshoot-min 1.5)         ; aim overshoot at grip 0
(def overshoot-max 7.0)         ; aim overshoot at grip 1
(def release-min-calm-ms 20)    ; release time-constant floor outside the hot window
(def release-min-hot-ms 1.0)    ; release time-constant floor at grip 1, fully hot
(def hot-start-db -20)          ; detector level where the hot window opens
(def hot-range-db 12)           ; fully hot this many dB above hot-start
(def hot-speedup 5)             ; time constants shrink by exp(-grip*hot*this)
; Shaper: exactly linear below the knee, then a rational soft clip
; s/(1+s^n)^(1/n) up to the ceiling. Hardness n: 2 is gentle, 8 is nearly a
; brick wall. Slope stays continuous at the knee for any n.
(def shaper-knee 0.75)
(def shaper-ceiling 1.02)
(def shaper-hardness 4)
(def drive-compensation 0.7)    ; fraction of drive dB removed after the shaper
; Original tone design: first-order low shelf + fourth-order Butterworth LP.
; Tone 0 is flat. Tone 100 applies this full response, independently of Mix.
(def shelf-db 1.5)
(def shelf-hz 200)
(def lowpass-hz 11500)
; ------------------------------------------------------------------------

; 10 ms dezippering, initialized directly to the first parameter value.
(defmacro smooth-control (value)
  (make-history previous)
  (make-history initialized)
  (def next (gswitch (read-history initialized)
    (+ value (* (exp (/ -100 samplerate)) (- (read-history previous) value)))
    value))
  (write-history initialized 1)
  (write-history previous next))

(def db-to-ln 0.11512925464970228) ; ln(10)/20
(def u (/ (smooth-control (clip amount 0 100)) 100))
(def attack-ms (smooth-control (clip attack 1 200)))
(def release-ms (smooth-control (clip release 20 2000)))
(def m (smooth-control (clip mix 0 1)))
(def tone-strength (/ (smooth-control (clip tone 0 100)) 100))
; Smooth one-hot weights, not the mode number: Punch -> Sustain never takes
; a detour through Level. Normalization prevents accumulated unity drift.
(def selected-mode (round (clip mode 0 2)))
(def punch-weight (smooth-control (eq selected-mode 0)))
(def level-weight (smooth-control (eq selected-mode 1)))
(def sustain-weight (smooth-control (eq selected-mode 2)))
(def weight-total (+ punch-weight level-weight sustain-weight))
(def drive-db (smooth-control (clip drive 0 24)))
(def input-gain (exp (* db-to-ln (smooth-control (clip input-db -24 24)))))
(def output-gain (exp (* db-to-ln (smooth-control (clip output-db -48 18)))))
(def detector-trim (smooth-control (clip detector-db -36 36)))
(def x-l (* left input-gain))
(def x-r (* right input-gain))
; Dyadic windows below cover up to 128 peak samples and 511 history samples.
(def span (clip (round (* peak-window-ms samplerate 0.001)) 1 127))
(def width (+ span 1))
(def history-size (clip (round (* history-ms samplerate 0.001)) 1 511))

; Integer delays of 1..256 samples on explicit 512-sample rings. seq orders
; the write and the fresh read; each invocation owns its ring and cursor.
(defmacro short-delay (signal frames)
  (def storage (tensor @shape [512]))
  (make-history cursor)
  (def position (read-history cursor))
  (write-history cursor (wrap (+ position 1) 0 512))
  (seq (poke storage position signal)
       (peek storage (wrap (- position frames) 0 512))))

; Dyadic sliding maximum: each stage grows the covered interval from stride
; to min(2*stride, width). Overlap is harmless for max.
(defmacro extend-max (x stride count)
  (def extra (max 0 (min stride (- count stride))))
  (def shifted (short-delay x (max 1 extra)))
  (gswitch (gt extra 0) (max x shifted) x))
; Stereo link on the louder channel.
(def peak-1 (max (abs x-l) (abs x-r)))
(def peak-2 (extend-max peak-1 1 width))
(def peak-4 (extend-max peak-2 2 width))
(def peak-8 (extend-max peak-4 4 width))
(def peak-16 (extend-max peak-8 8 width))
(def peak-32 (extend-max peak-16 16 width))
(def peak-64 (extend-max peak-32 32 width))
(def peak-128 (extend-max peak-64 64 width))
(def level (+ (* 8.685889638065037 (log (max peak-128 1e-20))) detector-trim))

; Static curve: quadratic soft knee of width knee-db, then a saturating line.
(def threshold (+ threshold-top-db (* threshold-span-db u)))
(def slope (+ slope-min (* slope-span u)))
(def over (- level threshold))
(def knee-x (clip (+ over (/ knee-db 2)) 0 knee-db))
(def kneed-over (+ (/ (* knee-x knee-x) (* 2 knee-db)) (max 0 (- over (/ knee-db 2)))))
(def reduction-max (* reduction-max-min-db (exp (* 0.6931471805599453 reduction-max-octaves u))))
(defmacro saturating-reduction (x)
  (* reduction-max (- 1 (exp (/ (* -1 slope x) reduction-max)))))
(def target (saturating-reduction kneed-over))

; Mean of the previous history-size reductions as a finite FIR of disjoint
; dyadic blocks (no running-total float drift).
(make-history reduction)
(def previous (read-history reduction))
(def sum-1 previous)
(def sum-2 (+ sum-1 (short-delay sum-1 1)))
(def sum-4 (+ sum-2 (short-delay sum-2 2)))
(def sum-8 (+ sum-4 (short-delay sum-4 4)))
(def sum-16 (+ sum-8 (short-delay sum-8 8)))
(def sum-32 (+ sum-16 (short-delay sum-16 16)))
(def sum-64 (+ sum-32 (short-delay sum-32 32)))
(def sum-128 (+ sum-64 (short-delay sum-64 64)))
(def sum-256 (+ sum-128 (short-delay sum-128 128)))
(defmacro history-part (block stride count)
  (def offset (% count stride))
  (def shifted (short-delay block (max 1 offset)))
  (def aligned (gswitch (gt offset 0) shifted block))
  (* (% (floor (/ count stride)) 2) aligned))
(def history-total (+
  (history-part sum-1 1 history-size)
  (history-part sum-2 2 history-size)
  (history-part sum-4 4 history-size)
  (history-part sum-8 8 history-size)
  (history-part sum-16 16 history-size)
  (history-part sum-32 32 history-size)
  (history-part sum-64 64 history-size)
  (history-part sum-128 128 history-size)
  (history-part sum-256 256 history-size)))
(def error (- target (/ history-total history-size)))
(def rising (gte error 0))
(def abs-error (abs error))

; Program-dependent envelope.
(def grip (* u u))
(def hot (clip (/ (- level hot-start-db) hot-range-db) 0 1))
(def heat (* grip hot))
(def overshoot (+ overshoot-min (* (- overshoot-max overshoot-min) grip)))
(def release-min-ms (+ release-min-calm-ms (* (- release-min-hot-ms release-min-calm-ms) heat)))
(def depth-speed (* (+ 1 overshoot) (exp (* -1 heat hot-speedup))))
(def attack-tau (max attack-min-ms (* attack-ms depth-speed (exp (/ (- 0 abs-error) attack-speedup-db)))))
(def release-tau (max release-min-ms (* release-ms depth-speed (exp (/ (- 0 abs-error) release-speedup-db)))))
(def tau-ms (gswitch rising attack-tau release-tau))
(def aim (max 0 (+ target (clip (* overshoot error) (- 0 overshoot-cap-db) overshoot-cap-db))))
(def pole (exp (/ -1000 (* tau-ms samplerate))))
(def gr (write-history reduction (+ aim (* pole (- previous aim)))))

; Auto makeup: unity gain for a steady input at unity-level-db.
(def makeup (saturating-reduction (max 0 (- unity-level-db threshold))))
(def sustain-gain (exp (* db-to-ln (- makeup gr))))

; Independently authored behavioral controllers fitted to audio I/O measurements.
(defmacro gain-curve (level-db threshold-db slope knee)
  (def over-db (- level-db threshold-db))
  (def knee-x (clip (+ over-db (* 0.5 knee)) 0 knee))
  (* slope (+ (/ (* knee-x knee-x) (* 2 knee))
    (max 0 (- over-db (* 0.5 knee))))))

; Stable 1-exp(-x), including the longest release at 384 kHz. The fourth-
; order branch has <1e-12 absolute truncation error for x<0.01.
(defmacro follower-step (ms)
  (def x (/ 1000 (* samplerate ms)))
  (gswitch (lt x 0.01)
    (* x (+ 1 (* x (+ -0.5 (* x (+ 0.1666666666666667 (* x -0.0416666666666667)))))))
    (- 1 (exp (- 0 x)))))

; A charge reservoir remembers exposure; during release it supplies a decaying
; floor to the fast envelope. On attack the fast envelope follows the target
; directly. Both updates are convex combinations of nonnegative values.
(defmacro memory-envelope (target attack-ms release-ms charge-ms memory-ms weight)
  (make-history reservoir)
  (make-history envelope)
  (make-history reservoir-fraction)
  (make-history envelope-fraction)
  (def memory-whole (read-history reservoir))
  (def memory-fraction (read-history reservoir-fraction))
  (def envelope-whole (read-history envelope))
  (def envelope-fraction-old (read-history envelope-fraction))
  (def old-memory (/ (+ memory-whole memory-fraction) 16384))
  (def old-envelope (/ (+ envelope-whole envelope-fraction-old) 16384))
  (def memory-tau (gswitch (gt target old-memory) charge-ms memory-ms))
  ; Explicit integer/fraction accumulation, not Kahan cancellation (which
  ; fast-math may erase). Whole parts are exact float32 integers; fractional
  ; increments accumulate near zero and carry via round. Reconstruct BOTH
  ; parts for output: this does not quantize the audible gain to 1/16384.
  ; Finite detector dB targets are below 512, so whole parts stay below 2^23.
  (def memory-sum (+ memory-fraction (* 16384 (follower-step memory-tau) (- target old-memory))))
  (def memory-carry (round memory-sum))
  (def memory-next-whole (+ memory-whole memory-carry))
  (def memory-next-fraction (- memory-sum memory-carry))
  (write-history reservoir memory-next-whole)
  (write-history reservoir-fraction memory-next-fraction)
  (def memory (/ (+ memory-next-whole memory-next-fraction) 16384))
  (def rising (gt target old-envelope))
  (def aim (gswitch rising target
    (min old-envelope (+ target (* weight (max 0 (- memory target)))))))
  (def tau (gswitch rising attack-ms release-ms))
  (def sum (+ envelope-fraction-old (* 16384 (follower-step tau) (- aim old-envelope))))
  (def carry (round sum))
  (def next-whole (+ envelope-whole carry))
  (def next-fraction (- sum carry))
  (write-history envelope next-whole)
  (write-history envelope-fraction next-fraction)
  (/ (+ next-whole next-fraction) 16384))

(defmacro detector-low (x g)
  (make-history z)
  (def v (/ (* g (- x (read-history z))) (+ 1 g)))
  (def low (+ v (read-history z)))
  (write-history z (+ low v))
  low)

(defmacro detector-lowpass (x g)
  (make-history z1)
  (make-history z2)
  (def band (/ (+ (read-history z1) (* g (- x (read-history z2))))
    (+ 1 (* g (+ g 1.4142135623730951)))))
  (def low (+ (read-history z2) (* g band)))
  (write-history z1 (- (* 2 band) (read-history z1)))
  (write-history z2 (- (* 2 low) (read-history z2)))
  low)

(defmacro rms-level (left right ms)
  (make-history power)
  (def p (max (* left left) (* right right)))
  (def smooth (+ p (* (exp (/ -1000 (* samplerate ms))) (- (read-history power) p))))
  (write-history power smooth)
  ; Peak-equivalent dB for a sine. A full-scale sine is 0 dB, not -3 dB.
  (+ (* 4.342944819032518 (log (max (* 2 smooth) 1e-30))) detector-trim))

; PUNCH: broad knee with a decisive, almost linear-in-dB onset and a short
; recovery reservoir. Amount changes threshold and slope monotonically.
(def punch-threshold (+ -9.9652 (* -43.6853 u) (* 16.487 u u)))
(def punch-slope (* 0.80984 (- 1 (exp (* -3.70731 (pow u 1.38568))))))
(def punch-detector-g (tan (/ (* 3.141592653589793 (min 1879.05 (* 0.4 samplerate))) samplerate)))
(def punch-detector-gain (exp (* db-to-ln 2.95957)))
(def punch-reference-w (tan (/ (* 3.141592653589793 1000) samplerate)))
(def punch-detector-scale (* (exp (* db-to-ln -0.4144))
  (sqrt (/ (+ (* punch-detector-g punch-detector-g) (* punch-reference-w punch-reference-w))
    (+ (* punch-detector-g punch-detector-g)
      (* punch-detector-gain punch-detector-gain punch-reference-w punch-reference-w))))))
(defmacro punch-detector (x)
  (* punch-detector-scale (+ (* punch-detector-gain x)
    (* (- 1 punch-detector-gain) (detector-low x punch-detector-g)))))
(def punch-level (rms-level (punch-detector x-l) (punch-detector x-r) 0.5))
(def punch-target (gain-curve punch-level punch-threshold punch-slope 19.5))
(def punch-gr (memory-envelope punch-target (* 1.0475 attack-ms) (* 0.8444 release-ms)
  (* 1.4787 release-ms) (* 2.4024 release-ms) 0.15335))
(def punch-gain (exp (* (- 0 db-to-ln) punch-gr)))

; LEVEL: bass/treble-sensitive detector and two relaxation populations.
; Filter response is normalized at 1 kHz at every rate.
(def detector-low-g (tan (/ (* 3.141592653589793 214.482) samplerate)))
(def detector-high-g (tan (/ (* 3.141592653589793 (min 3502.0182 (* 0.4 samplerate))) samplerate)))
(def detector-lp-g (tan (/ (* 3.141592653589793 (min 12551.2309 (* 0.45 samplerate))) samplerate)))
(def detector-low-gain (exp (* db-to-ln 6.3252)))
(def detector-high-gain (exp (* db-to-ln 11.8211)))
(def detector-w (tan (/ (* 3.141592653589793 1000) samplerate)))
(def detector-w2 (* detector-w detector-w))
(def detector-gl2 (* detector-low-g detector-low-g))
(def detector-gh2 (* detector-high-g detector-high-g))
(def detector-gp2 (* detector-lp-g detector-lp-g))
(def detector-low-power (/ (+ (* detector-low-gain detector-low-gain detector-gl2) detector-w2)
  (+ detector-gl2 detector-w2)))
(def detector-high-power (/ (+ detector-gh2 (* detector-high-gain detector-high-gain detector-w2))
  (+ detector-gh2 detector-w2)))
(def detector-lp-power (/ (* detector-gp2 detector-gp2)
  (+ (pow (- detector-gp2 detector-w2) 2) (* 2 detector-gp2 detector-w2))))
(def detector-normalization (/ 1 (sqrt (* detector-low-power detector-high-power detector-lp-power))))
(defmacro level-detector (x)
  (def low (+ x (* (- detector-low-gain 1) (detector-low x detector-low-g))))
  (def shelf (+ (* detector-high-gain low)
    (* (- 1 detector-high-gain) (detector-low low detector-high-g))))
  (* detector-normalization (detector-lowpass shelf detector-lp-g)))
; Independent approximation of the measured audio-path conditioning:
; a gentle DC blocker and a sub-dB high-frequency dip, not a saturator.
(def level-dc-g (tan (/ (* 3.141592653589793 4) samplerate)))
(def level-source-l (- x-l (detector-low x-l level-dc-g)))
(def level-source-r (- x-r (detector-low x-r level-dc-g)))
(def level-detector-l (level-detector level-source-l))
(def level-detector-r (level-detector level-source-r))
(def level-input (rms-level level-detector-l level-detector-r 0.5))
(def level-threshold (- 3.7361 (* 33.7184 u)))
(def level-target (gain-curve level-input level-threshold (* 0.75 (min 1 (* 4 u))) 7))

; Two linear-gain populations. Their weighted sum is a convex combination
; of positive gains; prolonged signals charge the slow population, producing
; exposure-dependent recovery without an artificial dB floor or gain slew cap.
; Store gains directly (initialized at unity), not 1-gain: strong reduction
; must not lose precision by subtracting nearly equal float32 values.
(defmacro gain-population (target attack-ms release-ms)
  (make-history stored)
  (make-history initialized)
  (make-history fraction)
  (def whole (gswitch (read-history initialized) (read-history stored) 16384))
  (def old-fraction (read-history fraction))
  (def previous (/ (+ whole old-fraction) 16384))
  (def tau (gswitch (lt target previous) attack-ms release-ms))
  ; Same split representation as the Punch reservoir. Gain is in (0,1], so
  ; its integer part is at most 16384 even under the longest release.
  (def sum (+ old-fraction (* 16384 (follower-step tau) (- target previous))))
  (def carry (round sum))
  (def next-whole (+ whole carry))
  (def next-fraction (- sum carry))
  (write-history stored next-whole)
  (write-history fraction next-fraction)
  (write-history initialized 1)
  (/ (+ next-whole next-fraction) 16384))
(def level-target-gain (exp (* (- 0 db-to-ln) level-target)))
; Soft onset near threshold, bounded at every Amount and sample rate.
(def level-attack-scale (+ 1 (/ 17.0739 (+ 1 (pow (/ level-target 4.26775) 7.53749)))))
(def level-fast (gain-population level-target-gain
  (* 0.0817833 attack-ms level-attack-scale) (* 2.30597 release-ms)))
(def level-slow (gain-population level-target-gain
  (* 0.901311 attack-ms level-attack-scale) (* 43.3357 release-ms)))
(def level-attenuation (+ (* 0.815406 level-fast) (* 0.184594 level-slow)))
(def level-gr (/ (- 0 (log (max level-attenuation 1e-30))) db-to-ln))
; Nominal gain measured at the reference's chosen operating point, independent
; of Amount. Output is an additional trim; Mix 0 is the exact dry path.
(def level-makeup 8.5)
(def level-gain (* (exp (* db-to-ln level-makeup)) level-attenuation))

; Common alignment, including dry. Controller/filter states remain warm.
(def aligned-l (short-delay x-l span))
(def aligned-r (short-delay x-r span))
(def level-audio-g (tan (/ (* 3.141592653589793 (min 11500 (* 0.4 samplerate))) samplerate)))
(def level-audio-cut (- (exp (* db-to-ln -0.65)) 1))
(defmacro level-audio (x)
  (make-history z1)
  (make-history z2)
  (def band (/ (+ (read-history z1) (* level-audio-g (- x (read-history z2))))
    (+ 1 (* level-audio-g (+ level-audio-g 1.5)))))
  (def low (+ (read-history z2) (* level-audio-g band)))
  (write-history z1 (- (* 2 band) (read-history z1)))
  (write-history z2 (- (* 2 low) (read-history z2)))
  (+ x (* level-audio-cut 1.5 band)))
(def level-l (level-audio (* level-gain (short-delay level-source-l span))))
(def level-r (level-audio (* level-gain (short-delay level-source-r span))))
(def uncolored-gain (+ (* punch-weight punch-gain) (* sustain-weight sustain-gain)))
(def compressed-l (/ (+ (* aligned-l uncolored-gain) (* level-weight level-l)) weight-total))
(def compressed-r (/ (+ (* aligned-r uncolored-gain) (* level-weight level-r)) weight-total))

; Shaper: unity below the knee, then the rational soft clip toward the
; ceiling. Odd-symmetric (odd harmonics only). Drive adds gain in front of it
; and takes most of it back afterwards.
(def shaper-headroom (- shaper-ceiling shaper-knee))
(def drive-pre (exp (* db-to-ln drive-db)))
(def drive-post (exp (* db-to-ln (- 0 (* drive-compensation drive-db)))))
(defmacro color (x)
  (def driven (* x drive-pre))
  (def a (abs driven))
  (def s (/ (max 0 (- a shaper-knee)) shaper-headroom))
  (def bent (/ s (pow (+ 1 (pow s shaper-hardness)) (/ 1 shaper-hardness))))
  (def shaped (* drive-post (sign driven) (+ (min a shaper-knee) (* shaper-headroom bent))))
  ; Sustain retains its original unity-drive shaper. Punch and Level are
  ; uncolored at Drive 0; raising Drive introduces the shaper continuously.
  (def strength (/ (+ sustain-weight
    (* (+ punch-weight level-weight) (clip (/ drive-db 6) 0 1))) weight-total))
  (mix x shaped strength))

; Scalar topology-preserving-transform filters. Fixed poles, smoothed blend:
; no coefficient interpolation, kernel rebuild, block delay or FFT service.
(def shelf-g (tan (/ (* 3.141592653589793 shelf-hz) samplerate)))
(def lp-g (tan (/ (* 3.141592653589793 (min lowpass-hz (* 0.45 samplerate))) samplerate)))
(defmacro low-shelf (x)
  (make-history z)
  (def v (/ (* shelf-g (- x (read-history z))) (+ 1 shelf-g)))
  (def low (+ v (read-history z)))
  (write-history z (+ low v))
  (+ x (* (- (exp (* shelf-db db-to-ln)) 1) low)))
(defmacro tone-lowpass (x damping)
  (make-history z1)
  (make-history z2)
  (def band (/ (+ (read-history z1) (* lp-g (- x (read-history z2))))
    (+ 1 (* lp-g (+ lp-g damping)))))
  (def low (+ (read-history z2) (* lp-g band)))
  (write-history z1 (- (* 2 band) (read-history z1)))
  (write-history z2 (- (* 2 low) (read-history z2)))
  low)
(defmacro finish (wet dry)
  (def colored (color wet))
  (def filtered (tone-lowpass (tone-lowpass (low-shelf colored) 1.847759065) 0.765366865))
  (* output-gain (mix dry (mix colored filtered tone-strength) m)))
(out (finish compressed-l aligned-l) 1 @name Left)
(out (finish compressed-r aligned-r) 2 @name Right)
