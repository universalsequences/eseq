; ES Compressor — sampler-style "sustain" compressor. Builtin effect whose
; DSP body is dgenlisp (see crates/sequencer/src/effects/es_compressor.rs).
;
; Very low threshold, heavy ratio and large auto makeup keep everything above
; the noise floor under compression, so decaying tails are lifted and feel
; sustained while a lookahead peak detector and a grab-and-settle envelope
; let drum transients through. Makeup drives a soft-knee saturator, and a
; dark minimum-phase tone filter follows. Crunch is gated by absolute level:
; only material near full scale gets the fast time constants that let the
; gain ride inside a bass or kick waveform; quieter material keeps a smooth
; envelope, so the effect reads as compression, not distortion.
;
; Generic techniques throughout (peak detector with short lookahead,
; feedforward one-pole envelope with error- and level-dependent time
; constants, real-cepstrum minimum-phase FIR, overlap-save convolution).
; Every curve is a closed-form formula; the numbers live in the TUNING block.
;
; Supported render rates: 8-384 kHz (fixed per instance). No oversampling.
; Latency: peak window + 255 samples of overlap-save block delay, declared
; below for the host's delay compensation.
(effect-latency (+ 255 (max 1 (min 127 (round (* 0.00025 samplerate))))))

(def left (in 1 @name Left))
(def right (in 2 @name Right))
(param amount @min 0 @max 100 @default 50)
(param attack @min 1 @max 200 @default 40 @unit ms)
(param release @min 20 @max 2000 @default 120 @unit ms)
(param mix @min 0 @max 1 @default 1)
(param drive @min 0 @max 24 @default 0 @unit dB)
(param input-db @min -24 @max 24 @default 0 @unit dB)
(param output-db @min -48 @max 6 @default -6 @unit dB)
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
; Tone: dB target of the minimum-phase FIR (scaled by mix in dB).
(def shelf-db 1.5)              ; low-shelf gain
(def shelf-hz 200)              ; shelf half-gain corner
(def lowpass-hz 11500)          ; -3 dB corner
(def lowpass-order 12)          ; 6 dB/oct per order
(def tone-floor-db -60)         ; stopband floor
(def tone-taper-start 1024)     ; FIR taps beyond this fade with a half-cosine
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
(def drive-db (smooth-control (clip drive 0 24)))
(def input-gain (exp (* db-to-ln (smooth-control (clip input-db -24 24)))))
(def output-gain (exp (* db-to-ln (smooth-control (clip output-db -48 6)))))
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
; Gain-domain mix: mix scales (makeup - reduction) in dB.
(def gain (exp (* db-to-ln m (- makeup gr))))
; Delay the audio by the peak window so the trailing max acts as lookahead.
(def compressed-l (* (short-delay x-l span) gain))
(def compressed-r (* (short-delay x-r span) gain))

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
  (mix x shaped m))

; Tone filter. The dB target is a low shelf plus a Butterworth-magnitude
; lowpass evaluated on a 4096-bin grid at the host rate, converted to a
; minimum-phase impulse through the real cepstrum, tapered, then applied by
; 8192-point overlap-save with a 256-sample hop. Mix changes rebuild the
; kernel per hop; old and new outputs crossfade across the hop.
(def color-rate (hop-hold samplerate 256))
(make-history color-last-mix)
(make-history color-mix-seeded)
(make-history color-clock)
(def color-tick (eq (read-history color-clock) 0))
(write-history color-clock (% (+ (read-history color-clock) 1) 256))
(def color-mix (hop-hold m 256))
(def color-before (hop-hold (gswitch (read-history color-mix-seeded)
  (read-history color-last-mix) m) 256))
(write-history color-last-mix (gswitch color-tick m (read-history color-last-mix)))
(write-history color-mix-seeded 1)
(def color-index (iota 4096))
(def color-positive (min color-index (- 4096 color-index)))
(def color-hz (* color-positive (/ color-rate 4096)))
(def color-lowpass-db (* -10 (log10 (+ 1 (pow (/ color-hz lowpass-hz) (* 2 lowpass-order))))))
(def color-shelf-db (/ shelf-db (+ 1 (pow (/ color-hz shelf-hz) 2))))
(def color-db (max tone-floor-db (+ color-shelf-db color-lowpass-db)))
(def color-log-base (* color-db db-to-ln))
; Textbook causal lifter: keep c[0], double 1..N/2-1, keep c[N/2], zero the rest.
(def color-lifter (+ (eq color-index 0) (eq color-index 2048)
  (* 2 (* (gte color-index 1) (lt color-index 2048)))))
(def color-tail (clip (/ (- color-index tone-taper-start) (- 4096 tone-taper-start)) 0 1))
(def color-window (* 0.5 (+ 1 (cos (* 3.141592653589793 color-tail)))))
(defmacro base-transfer (strength)
  (def log-mag (* color-log-base strength))
  (def cep (ifft log-mag (* log-mag 0) @N 4096 @backend accelerated))
  (def (re im) (fft (* cep color-lifter) @N 4096 @backend accelerated))
  (def mag (exp re))
  (def ir (ifft (* mag (cos im)) (* mag (sin im)) @N 4096 @backend accelerated))
  (def taps (* ir color-window))
  ; Zero padding keeps overlap-save linear: 8192 >= 4096 + 256 - 1.
  (fft (pad taps @padding [0:4096]) @N 8192 @backend accelerated))
(def (color-new-re color-new-im) (base-transfer color-mix))
(def (color-old-re color-old-im) (base-transfer color-before))
(def color-ramp (/ (iota 256) 255))
(defmacro base-filter (audio)
  (def window (reshape (buffer audio 8192 256) @shape [8192]))
  (def (re im) (fft window @N 8192 @backend accelerated))
  (def new-time (ifft
    (- (* re color-new-re) (* im color-new-im))
    (+ (* re color-new-im) (* im color-new-re)) @N 8192 @backend accelerated))
  (def old-time (ifft
    (- (* re color-old-re) (* im color-old-im))
    (+ (* re color-old-im) (* im color-old-re)) @N 8192 @backend accelerated))
  ; Keep the newest 256 samples; the earlier part is circularly contaminated.
  (def new-block (shrink new-time @ranges [7936:8192]))
  (def old-block (shrink old-time @ranges [7936:8192]))
  (overlap-add (+ (* (- 1 color-ramp) old-block) (* color-ramp new-block)) 256))
(out (* output-gain (base-filter (color compressed-l))) 1 @name Left)
(out (* output-gain (base-filter (color compressed-r))) 2 @name Right)
