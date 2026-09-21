; ES Compressor: Punch / Level / Sustain. Independently authored, asset-free.
; Sources, derivations and intentional departures from the literature:
; crates/sequencer/docs/es-compressor-effect-spec.md.
; Punch: feedforward log-domain soft knee + passive two-capacitor timing.
; Level: feedback optical attenuation; coupled photocarrier populations from
; Najnudel et al., DAFx 2023, Eq. (4), with an original normalized driver.
; Sustain: the existing ES history/error-dependent grab-and-settle controller.
; All controllers run continuously; mode selects smoothly crossfaded gains.
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

; PUNCH. Giannoulis/Massberg/Reiss (2012) soft-knee gain computer followed
; by an original passive RC ladder. In normalized capacitor-voltage units:
;   da/dt = (target-a)/tau + (b-a)/memory, db/dt = (a-b)/memory.
; Backward Euler solves both nodes together: nonnegative coefficients, unity
; DC gain, and no explicit-Euler stability limit at low sample rates.
(make-history punch-a)
(make-history punch-b)
(def pa (read-history punch-a))
(def pb (read-history punch-b))
(def punch-threshold (- -6 (* 30 u)))
(def punch-ratio (+ 1 (* 7 u)))
(def punch-over (- level punch-threshold))
(def punch-knee (clip (+ punch-over 3) 0 6))
(def punch-target (* (- 1 (/ 1 punch-ratio))
  (+ (/ (* punch-knee punch-knee) 12) (max 0 (- punch-over 3)))))
(def punch-tau (gswitch (gt punch-target pa) attack-ms release-ms))
(def pd (/ 1000 (* samplerate punch-tau)))
(def pc (/ 1000 (* samplerate (* release-ms 0.25))))
(def punch-next-a (/ (+ (* (+ pa (* pd punch-target)) (+ 1 pc)) (* pc pb))
  (+ 1 pd (* 2 pc) (* pd pc))))
(def punch-next-b (/ (+ pb (* pc punch-next-a)) (+ 1 pc)))
(write-history punch-a punch-next-a)
(write-history punch-b punch-next-b)
(def punch-gain (exp (* (- 0 db-to-ln) punch-next-a)))

; Common audio alignment. Level's feedback is taken AFTER its own attenuator
; and BEFORE makeup, saturation, mode crossfade and output trim.
(def aligned-l (short-delay x-l span))
(def aligned-r (short-delay x-r span))

; LEVEL. Normalized form of Najnudel et al. (2023), Eq. (4):
;   dn/dt = J - kn*(n-p)*n, dp/dt = J - kp*(1+p-n)*p.
; n,p are electron/hole populations; trap capacity is normalized to 1.
; Generation adds equal charge. Sequential implicit recombination steps
; preserve n>=p>=0 and 0<=n-p<=1 without a Newton solver or state clipping.
; These are OUR normalized rates/mobilities, not fitted Vactrol/T4 data.
; Store p and d=n-p rather than subtracting nearly equal carrier counts.
; This keeps the bounded trap occupancy well-conditioned in float32.
(make-history traps)
(make-history holes)
(make-history optical-feedback)
(def light-drive (max 0 (- (* (read-history optical-feedback)
  (exp (* db-to-ln (- detector-trim punch-threshold)))) 1)))
; Finite LED-driver headroom, with a smooth asymptote; no exponent overflow.
(def light-limited (/ light-drive (+ 1 (/ light-drive 8))))
(def generation (* u (/ 1000 (* attack-ms samplerate))
  light-limited light-limited))
(def d0 (read-history traps))
(def p0 (+ (read-history holes) generation))
(def kn (/ 1000 (* release-ms samplerate)))
(def kp (* 4 kn))
; Solve k*d^2 + b*d = old_delta using the cancellation-free positive root.
(def nb (+ 1 (* kn p0)))
(def d1 (/ (* 2 d0) (+ nb (sqrt (+ (* nb nb) (* 4 kn d0))))))
(def empty-traps (- 1 d1))
(def p-floor (max 0 (- p0 empty-traps)))
(def pdiff (min p0 empty-traps))
(def pbase (+ 1 (* kp (abs (- empty-traps p0)))))
(def p1 (+ p-floor (/ (* 2 pdiff)
  (+ pbase (sqrt (+ (* pbase pbase) (* 4 kp pdiff)))))))
(def d2 (- 1 (/ empty-traps (+ 1 (* kp p1)))))
(def n1 (+ p1 d2))
(write-history traps d2)
(write-history holes p1)
; Eq. (7): conductivity is the mobility-weighted sum of populations.
; A series resistor and LDR shunt form an ordinary voltage divider.
(def level-gain (/ 1 (+ 1 (* u (+ n1 (* 0.2 p1))))))
(write-history optical-feedback (* level-gain (max (abs aligned-l) (abs aligned-r))))

(def gain (/ (+ (* punch-weight punch-gain) (* level-weight level-gain)
  (* sustain-weight sustain-gain)) weight-total))
(def compressed-l (* aligned-l gain))
(def compressed-r (* aligned-r gain))

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
