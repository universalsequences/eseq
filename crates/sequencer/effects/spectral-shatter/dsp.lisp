; Spectral shatter: per-bin spectral delay + probabilistic per-bin freeze.
;
; Where spectral-bloom gives every bin one shared clock (one decay, one drift),
; shatter gives every bin its OWN time. A [96 x 2048] magnitude ring buffer
; (shift register, one row per hop) lets each bin read from a different point
; in the past: `tilt` curves the delay across log-frequency (lows land first,
; highs cascade in over ~1s -- or reversed), `scatter` randomizes per-bin
; delays so transients shatter into confetti, and per-bin feedback re-injects
; each tap at its own period (every partial echoes at its own rate, dampened
; toward Nyquist). `freeze` is PER-BIN and probabilistic: each bin
; independently freezes/thaws on a slow random schedule, so at mid settings
; the sound continuously disintegrates -- some partials hang like glass while
; the rest keep moving.
;
; Resynthesis is bloom's oscillator-bank phase engine (phase never comes from
; the input): per-bin phase accumulator at each bin's center frequency, `haze`
; random-walk for tonal->airy, `width` static R-phase offset for stereo. All
; per-bin curves (delay, scatter, freeze masks) are built from FOLDED indices
; so the magnitude spectrum stays conjugate-symmetric
; (see dgenlisp-spectral-mask-symmetry).
;
; Ring mechanics (harness-proven): shift = gather(prev, lin-2048) -- gather
; clamps negative indices to 0 but row 0 is overwritten by the mask blend;
; per-bin tap = gather(ring, row*2048 + bin) with manual lerp between rows
; (gather truncates fractional indices). Delay is clamped to 94 rows so the
; lerp's +1 row never wraps to a wrong bin via the flat-index clamp.

(def in-l (in 1 @name left))
(def in-r (in 2 @name right))
(def in3 (in 3 @modulator 1))
(def in4 (in 4 @modulator 2))
(def in5 (in 5 @modulator 3))
(def in6 (in 6 @modulator 4))
(def input (* 0.5 (+ in-l in-r)))

(param time    @min 0    @max 1 @default 0.5  @mod true @mod-mode additive) ; cascade spread, 0 .. ~1.1s
(param tilt    @min -1   @max 1 @default 0.6)  ; delay curve: -1 highs-first .. +1 lows-first
(param scatter @min 0    @max 1 @default 0.0  @mod true @mod-mode additive) ; per-bin random delay offsets
(param fb      @min 0    @max 1 @default 0.35 @mod true @mod-mode additive) ; per-bin feedback (each partial echoes at its own period)
(param freeze  @min 0    @max 1 @default 0.0  @mod true @mod-mode additive) ; per-bin probabilistic freeze
(param damp    @min 0    @max 1 @default 0.3)  ; highs lose energy per feedback trip
(param haze    @min 0    @max 1 @default 0.25 @mod true @mod-mode additive) ; phase random-walk: tonal -> airy
(param width   @min 0    @max 1 @default 0.8)  ; stereo phase decorrelation
(param mix     @min 0    @max 1 @default 0.5  @mod true @mod-mode additive)
(param output  @min 0.25 @max 2 @default 1.0)

; --- STFT analysis. 2048 frame, hop 512, sqrt-Hann * 0.7071 for unity COLA
; at 75% overlap. Mono analysis; stereo recreated at resynthesis.
(def win (* 0.70710678 (sqrt (hann 2048))))
(def frame (* (reshape (buffer input 2048 512) @shape [2048]) win))
(def (re im) (fft frame @N 2048 @backend accelerated))
(def mag (sqrt (+ (* re re) (* im im))))

(def fold-idx   (tensor @shape [2048] @file "fold_index.json"))
(def fold-norm  (tensor @shape [2048] @file "fold_norm.json"))
(def bin-sign   (tensor @shape [2048] @file "bin_sign.json"))
(def phase-adv  (tensor @shape [2048] @file "phase_advance.json"))
(def phase-off  (tensor @shape [2048] @file "phase_offset.json"))
(def bin-idx    (tensor @shape [2048] @file "bin_index.json"))
(def curve-norm (tensor @shape [2048] @file "curve_norm.json"))
(def delay-rand (tensor @shape [2048] @file "delay_rand.json"))
(def lin-idx    (tensor @shape [196608] @file "lin_index.json"))
(def row0-mask  (tensor @shape [196608] @file "row0_mask.json"))
(def tile-idx   (tensor @shape [196608] @file "tile_index.json"))

; Hop-hold every param-derived scalar BEFORE tensor math (see bloom: a
; frame-rate scalar demotes the whole tensor chain to per-frame execution).
(def time-h   (hop-hold (mod time) 512))
(def tilt-h   (hop-hold tilt 512))
(def scat-h   (hop-hold (mod scatter) 512))
(def fb-h     (hop-hold (* 0.98 (mod fb)) 512))
(def damp4-h  (hop-hold (* 4 damp) 512))
(def freeze-h (hop-hold (mod freeze) 512))
(def hazemod (mod haze))
(def jitter-h (hop-hold (* hazemod hazemod 3.5) 512))
(def width-h  (hop-hold width 512))

; --- Per-bin delay curve, in ring rows [0, 94].
; Base is centered: tilt swings bins toward now/past along log-frequency;
; scatter adds symmetric per-bin random offsets. Clamp keeps lerp row+1 <= 95.
(def dnorm (min (max (+ 0.5
                        (* 0.5 tilt-h (- (* 2 curve-norm) 1))
                        (* scat-h (- delay-rand 0.5)))
                     0)
                1))
(def d (min (max (* time-h 94 dnorm) 0) 94))

; --- Magnitude ring: [96 x 2048] flat, one row per hop, shift-register style.
(make-history ring @shape [196608] @hop 512)
(def prev (read-history ring))

; Per-bin taps with manual row lerp (gather truncates; floor is a no-op on
; runtime values in this dgen build -- use x - x%1).
(def d-lo (- d (% d 1)))
(def d-frac (- d d-lo))
(def tap-idx (+ (* d-lo 2048) bin-idx))
(def tap-lo (gather prev tap-idx))
(def tap-hi (gather prev (+ tap-idx 2048)))
(def delayed (+ (* tap-lo (- 1 d-frac)) (* tap-hi d-frac)))

; Per-bin feedback: each tap re-enters at row 0, so a bin at delay d echoes
; every d hops. Linear gain < 1 is geometrically stable; damp multiplies an
; extra per-trip HF attenuation.
(def g (* fb-h (exp (* -1 damp4-h fold-norm))))
(def ring-in (+ mag (* delayed g)))

; Shift + inject row 0. Negative shift indices clamp to element 0, but the
; row-0 mask overwrites that region with the injected spectrum.
(def shifted (gather prev (- lin-idx 2048)))
(def tiled-in (gather ring-in tile-idx))
(def next-ring (+ (* row0-mask tiled-in) (* (- 1 row0-mask) shifted)))
(write-history ring next-ring)

; --- Per-bin probabilistic freeze. Each bin holds a slow random value r
; (re-decided with prob ~0.02/hop => every ~0.6s); bins with 1-r < freeze are
; frozen. 1-r so the zero-initialized state reads as unfrozen. Frozen bins
; hold their last magnitude in the ice history; thawed bins track the tap.
; GOTCHA: identical (noise @size N @hop H) expressions are deduped to ONE
; node -- two draws return the same values. Draw a single wide tensor and
; gather each consumer from a disjoint (still folded-symmetric) index range.
(def nz (noise @size 4096 @hop 512))
(make-history icemask @shape [2048] @hop 512)
(def r-prev (read-history icemask))
(def n-upd (* 0.5 (+ 1 (gather nz fold-idx))))
(def n-val (* 0.5 (+ 1 (gather nz (+ fold-idx 1560)))))
(def upd (lt n-upd 0.02))
(def r-next (+ (* upd n-val) (* (- 1 upd) r-prev)))
(write-history icemask r-next)

(def f-mask (lt (- 1 r-next) freeze-h))
(make-history icemag @shape [2048] @hop 512)
(def held-prev (read-history icemag))
(def held-next (+ (* f-mask held-prev) (* (- 1 f-mask) delayed)))
(write-history icemag held-next)
(def wet-mag (hop-hold held-next 512))

; --- Oscillator-bank phase engine (bloom's): per-bin phase accumulator at
; bin center frequency + baked detune; haze widens partials into noise bands;
; width decorrelates R with a static random offset.
(make-history sh-phase @shape [2048] @hop 512)
(def ph-prev (read-history sh-phase))
(def jit (* (gather nz (+ fold-idx 2900)) bin-sign jitter-h))
(def ph-next (wrap (+ ph-prev phase-adv jit) 0 twopi))
(write-history sh-phase ph-next)
(def ph (hop-hold ph-next 512))
(def ph-r (+ ph (* width-h phase-off)))

(def wet-re-l (* wet-mag (cos ph)))
(def wet-im-l (* wet-mag (sin ph)))
(def wet-re-r (* wet-mag (cos ph-r)))
(def wet-im-r (* wet-mag (sin ph-r)))

; tanh caps the feedback worst case (fb=1 sustained input can stack +30 dB).
(def wet-l (tanh (overlap-add (* (ifft wet-re-l wet-im-l @N 2048 @backend accelerated) win) 512)))
(def wet-r (tanh (overlap-add (* (ifft wet-re-r wet-im-r @N 2048 @backend accelerated) win) 512)))

(def mix-m (mod mix))
(out (* output (mix in-l wet-l mix-m)) 1 @name left)
(out (* output (mix in-r wet-r mix-m)) 2 @name right)
