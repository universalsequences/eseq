; Spectral bloom: a feedback spectral cloud machine.
;
; The wet path is GENERATIVE, not a filter. Each hop the magnitude spectrum is
; combined with a decaying history via max(): new energy punches in instantly,
; old energy sustains and fades. The fed-back magnitudes are pitch-DRIFTED
; (sub-bin lerp-gather resample, so a frozen chord slowly rises/falls) and
; BLOOMED (repeated 9-tap convolution compounds into progressive diffusion --
; tails literally dissolve into air). Phase never comes from the input: a
; per-bin oscillator bank (phase accumulator history at each bin's center
; frequency + static detune) resynthesizes the cloud, with `haze` adding a
; random phase walk (tonal organ -> breathy air) and `width` decorrelating R
; with a static random phase offset for huge stable stereo.
;
; All phase tensors are antisymmetric about Nyquist and the drift index uses a
; FOLDED bin index, so the synthesized spectrum stays conjugate-symmetric
; (see dgenlisp-spectral-mask-symmetry: asymmetric masks fold to mush).
; max() feedback with per-hop gain < 1 is unconditionally stable; freeze=1
; pins the gain at exactly 1 and mutes injection: infinite sustain.

(def in-l (in 1 @name left))
(def in-r (in 2 @name right))
(def in3 (in 3 @modulator 1))
(def in4 (in 4 @modulator 2))
(def in5 (in 5 @modulator 3))
(def in6 (in 6 @modulator 4))
(def input (* 0.5 (+ in-l in-r)))

(param decay  @min 0    @max 1 @default 0.55) ; tail length, ~0.25s .. 20s
(param drift  @min -1   @max 1 @default 0.2)  ; cloud pitch drift, cubic, +/-12 semi/s
(param bloom  @min 0    @max 1 @default 0.25) ; per-hop spectral diffusion
(param haze   @min 0    @max 1 @default 0.3 @mod true @mod-mode additive)  ; phase random-walk: tonal -> airy
(param damp   @min 0    @max 1 @default 0.35) ; highs decay faster
(param freeze @min 0    @max 1 @default 0.0 @mod true @mod-mode additive)  ; 1 = stop injecting, sustain forever
(param width  @min 0    @max 1 @default 0.8)  ; stereo phase decorrelation
(param mix    @min 0    @max 1 @default 0.5)
(param output @min 0.25 @max 2 @default 1.0)

; --- STFT analysis. 2048 frame, hop 512 (required by the 512-frame host
; block), sqrt-Hann * 0.7071 for unity COLA at 75% overlap. Mono analysis;
; stereo is recreated at resynthesis via phase decorrelation.
(def win (* 0.70710678 (sqrt (hann 2048))))
(def frame (* (reshape (buffer input 2048 512) @shape [2048]) win))
(def (re im) (fft frame @N 2048 @backend accelerated))
(def mag (sqrt (+ (* re re) (* im im))))

(def fold-idx  (tensor @shape [2048] @file "fold_index.json"))
(def fold-norm (tensor @shape [2048] @file "fold_norm.json"))
(def bin-sign  (tensor @shape [2048] @file "bin_sign.json"))
(def phase-adv (tensor @shape [2048] @file "phase_advance.json"))
(def phase-off (tensor @shape [2048] @file "phase_offset.json"))

; CRITICAL for CPU: params are frame-rate scalars, and any tensor expression
; touching one is demoted to per-frame execution -- which cascades through the
; whole synthesis chain, ultimately running the 2048-point IFFTs at 44.1kHz
; instead of 86/hop. Hop-hold every param-derived scalar BEFORE tensor math.
(def freeze-h  (hop-hold (mod freeze) 512))
(def width-h   (hop-hold width 512))
(def drift-s   (hop-hold (exp (* -0.0080475 drift drift drift)) 512))
(def rate-h    (hop-hold (* 0.3208 (exp (* -4.382 decay))) 512))
(def damp6-h   (hop-hold (* 6 damp) 512))
(def hazemod (mod haze))
(def jitter-h  (hop-hold (* hazemod hazemod 3.5) 512))
(def bloom-h   (hop-hold bloom 512))

; --- Magnitude cloud with hop-rate feedback.
(make-history bloom-mag @shape [2048] @hop 512)
(def prev (read-history bloom-mag))

; Pitch drift: resample the previous magnitudes by ratio s each hop. gather
; truncates fractional indices, so lerp between floor and floor+1 by hand --
; that both makes sub-bin drift actually move and adds a touch of welcome
; smear. Folded index keeps the result symmetric; drift-down reads into the
; mirror half, which is the correct symmetric continuation.
(def drift-idx (* fold-idx drift-s))
(def idx-lo (floor drift-idx))
(def idx-frac (- drift-idx idx-lo))
(def drifted (+ (* (gather prev idx-lo) (- 1 idx-frac))
                (* (gather prev (+ idx-lo 1)) idx-frac)))

; Diffusion: blending in a normalized blur each hop compounds over time, so
; sustained energy spreads wider and wider (Gaussian growth) -> "bloom".
(def blur-k (tensor @shape [9] @data [0.02 0.04 0.08 0.14 0.44 0.14 0.08 0.04 0.02]))
(def diffused (+ (* (- 1 bloom-h) drifted) (* bloom-h (conv1d drifted blur-k))))

; Per-bin feedback gain. decay maps to T60 0.25s..20s at the 11.6ms hop.
; damp MULTIPLIES the decay rate toward Nyquist (relative tilt: T60 ~ /7 at
; the top at damp=1) -- an additive per-hop term would swamp the base rate
; and shorten every tail. freeze crossfades the whole gain to exactly 1.
(def g-open (exp (* -1 rate-h (+ 1 (* damp6-h fold-norm)))))
(def g (+ freeze-h (* (- 1 freeze-h) g-open)))

; NOTE: write-history's pass-through return is miswired in dgen (downstream
; consumers see the write's INPUT source tensor, not the result) -- so the
; write is a bare statement and consumers hop-hold the value expression.
(def inject (* mag (- 1 freeze-h)))
(def next-mag (max inject (* diffused g)))
(write-history bloom-mag next-mag)
(def cloud-mag (hop-hold next-mag 512))

; --- Oscillator-bank phase engine. Each bin's phase advances by its center
; frequency (+ baked detune) per hop; haze adds an antisymmetric random walk
; that widens each partial into a noise band. wrap keeps the accumulator
; bounded; mod-2pi preserves antisymmetry as an angle class.
(make-history bloom-phase @shape [2048] @hop 512)
(def ph-prev (read-history bloom-phase))
(def jit (* (gather (noise @size 2048 @hop 512) fold-idx) bin-sign jitter-h))
(def ph-next (wrap (+ ph-prev phase-adv jit) 0 twopi))
(write-history bloom-phase ph-next)
(def ph (hop-hold ph-next 512))
(def ph-r (+ ph (* width-h phase-off)))

(def wet-re-l (* cloud-mag (cos ph)))
(def wet-im-l (* cloud-mag (sin ph)))
(def wet-re-r (* cloud-mag (cos ph-r)))
(def wet-im-r (* cloud-mag (sin ph-r)))

; tanh = transparent at normal levels, but caps the worst case (max-hold of
; dense input at decay=1 can stack bins +20 dB over the dry peak).
(def wet-l (tanh (overlap-add (* (ifft wet-re-l wet-im-l @N 2048 @backend accelerated) win) 512)))
(def wet-r (tanh (overlap-add (* (ifft wet-re-r wet-im-r @N 2048 @backend accelerated) win) 512)))

(out (* output (mix in-l wet-l mix)) 1 @name left)
(out (* output (mix in-r wet-r mix)) 2 @name right)
