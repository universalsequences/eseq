; Spectral xenovox: the spectral-vox formant/vocoder engine pushed alien.
; Same base (whiten carrier, impose vocal envelope, sidechain vocoder with
; vowel-engine fallback) plus two deliberately wrong decisions:
;
;   ALIEN 1 -- scrambled formant geography (`scramble`): the vocal envelope is
;   read through a baked block-permutation of the folded frequency axis
;   (16-bin chunks shuffled, seed 0xAE, bass chunk anchored). Local envelope
;   shape survives but formants land at impossible places: phonemes from a
;   mouth that doesn't exist.
;
;   ALIEN 2 -- clocked envelope automaton (`stutter`): the vocal envelope only
;   updates when a geometric random clock fires (update probability falls
;   with the knob; holds reach ~1/4 s). Between firings the entire mouth is
;   LATCHED -- frozen formants over a moving carrier -- and the vowel morph
;   position quantizes to the same grid: stepped, rule-driven Autechre motion
;   instead of organic glide.
;
; The carrier (whatever synth you run through it) keeps its own pitch, phase
; and dynamics; only its spectral SHAPE is rewritten. Each hop the carrier's
; envelope is measured (constant-Q cumsum smoothing of log magnitude), removed
; (`body` = how completely), and replaced with a vocal envelope from one of
; two sources, crossfaded by `voice` x sidechain-energy:
;
;   - synthetic vowel engine: 4 formant resonances (tenor table) morphed
;     continuously u -> o -> a -> e -> i by `vowel`, with `talk` wobbling the
;     morph position through two incommensurate LFOs so it babbles on its own.
;   - sidechain vocoder: the sidechain's own measured spectral envelope is
;     imposed on the whitened carrier -- run a real voice (or drums) in and
;     the synth speaks it. Falls back to the vowel engine when the sidechain
;     goes quiet, so the effect never dies between syllables.
;
; `formant` shifts both sources' formants +/-1 octave (gender/size) -- the
; sidechain envelope is warped along the folded log axis by lerp-gather, the
; synthetic formant centers just scale. `resonance` sharpens: narrower
; gaussian bandwidths on the synth side, contrast-expanded envelope on the
; voice side. `breath` roughens the gain with per-hop symmetric noise and
; tilts in a high-frequency floor (air/whisper).
;
; Everything per-bin is built from FOLDED index/frequency tensors and mirrored
; smoothing windows, so every gain is conjugate-symmetric and the IFFT stays
; real (see dgenlisp-spectral-mask-symmetry).

(def in-l (in 1 @name left))
(def in-r (in 2 @name right))
(def sc-in (in 3 @name sidechain))

(param vowel     @min 0    @max 1 @default 0.5)  ; u -> o -> a -> e -> i
(param talk      @min 0    @max 1 @default 0.25) ; auto vowel babble depth+rate
(param voice     @min 0    @max 1 @default 1.0)  ; sidechain vocoder amount
(param formant   @min -1   @max 1 @default 0.0)  ; formant shift, +/-1 octave
(param resonance @min 0    @max 1 @default 0.6)  ; formant sharpness
(param body      @min 0    @max 1 @default 0.8)  ; carrier envelope replacement
(param breath    @min 0    @max 1 @default 0.15) ; noise roughening + HF air
(param scramble  @min 0    @max 1 @default 0.4)  ; ALIEN 1: permuted formant map
(param stutter   @min 0    @max 1 @default 0.5)  ; ALIEN 2: envelope hold clock
(param mix       @min 0    @max 1 @default 1.0)
(param output    @min 0.25 @max 2 @default 1.0)

; --- STFT analysis: 2048 frame, hop 512 (host block), sqrt-Hann * 0.7071 for
; unity COLA at 75% overlap. Carrier L/R analyzed separately (one shared gain
; keeps the stereo image); sidechain analyzed mono.
(def win (* 0.70710678 (sqrt (hann 2048))))
(def frame-l (* (reshape (buffer in-l 2048 512) @shape [2048]) win))
(def frame-r (* (reshape (buffer in-r 2048 512) @shape [2048]) win))
(def frame-sc (* (reshape (buffer sc-in 2048 512) @shape [2048]) win))
(def (re-l im-l) (fft frame-l @N 2048 @backend accelerated))
(def (re-r im-r) (fft frame-r @N 2048 @backend accelerated))
(def (re-sc im-sc) (fft frame-sc @N 2048 @backend accelerated))

(def mag-l (sqrt (+ (* re-l re-l) (* im-l im-l))))
(def mag-r (sqrt (+ (* re-r re-r) (* im-r im-r))))
(def mag (* 0.5 (+ mag-l mag-r)))
(def mag-sc (sqrt (+ (* re-sc re-sc) (* im-sc im-sc))))

(def fold-freq (tensor @shape [2048] @file "fold_freq.json"))
(def fold-idx  (tensor @shape [2048] @file "fold_index.json"))
(def fold-norm (tensor @shape [2048] @file "fold_norm.json"))
(def cq-lo     (tensor @shape [2048] @file "cq_lo.json"))
(def cq-hi     (tensor @shape [2048] @file "cq_hi.json"))
(def cq-ic     (tensor @shape [2048] @file "cq_inv_count.json"))
(def cqn-lo    (tensor @shape [2048] @file "cqn_lo.json"))
(def cqn-hi    (tensor @shape [2048] @file "cqn_hi.json"))
(def cqn-ic    (tensor @shape [2048] @file "cqn_inv_count.json"))
(def scram-idx (tensor @shape [2048] @file "perm_index.json"))

; CRITICAL for CPU (see dgenlisp-stft-cpu-temporality): hop-hold every
; param-derived scalar BEFORE it touches tensor math, or the whole synthesis
; chain demotes to per-frame and the IFFTs run at 44.1kHz.
(def voice-h  (hop-hold voice 512))
(def res-h    (hop-hold resonance 512))
(def body-h   (hop-hold body 512))
(def breath-h (hop-hold breath 512))
(def fshift-h (hop-hold (exp (* 0.6931472 formant)) 512))
(def scram-h  (hop-hold scramble 512))
(def stut-h   (hop-hold stutter 512))

; ALIEN 2 clock: one uniform random per hop; update only when it clears the
; stutter threshold -> geometric hold lengths (mean 1/(1-0.94*stutter) hops).
; CPU-CRITICAL: `follow` feeds latch conds, and latch only inherits hop rate
; from an explicitly hop-tagged cond node (a hop-hold output) -- derived math
; like a bare gte carries no tag, which demotes both latches and everything
; downstream of them (vowel chain, 2048-wide envelope latch, wet IFFTs) to
; per-frame. So the gte itself must sit INSIDE a hop-hold.
(def hop-rand (hop-hold (* 0.5 (+ 1 (noise))) 512))
(def follow (hop-hold (gte hop-rand (* 0.94 stut-h)) 512))

; --- Vowel morph position. talk wobbles it with two incommensurate sines
; (rate rises with talk), clamped to the table ends.
(def talk-rate (+ 0.4 (* 2.4 talk)))
(def wob (+ (* 0.9 (sin (* twopi (phasor talk-rate))))
            (* 0.6 (sin (* twopi (phasor (* talk-rate 1.83)))))))
(def p-live (hop-hold (min 4 (max 0 (+ (* vowel 4) (* talk 1.4 wob)))) 512))
(def p (latch p-live follow))

; Triangle weights select/blend adjacent vowels: u o a e i at p = 0 1 2 3 4.
(def w0 (relu (- 1 (abs (- p 0)))))
(def w1 (relu (- 1 (abs (- p 1)))))
(def w2 (relu (- 1 (abs (- p 2)))))
(def w3 (relu (- 1 (abs (- p 3)))))
(def w4 (relu (- 1 (abs (- p 4)))))

; Tenor formant table (Hz / linear amp / bandwidth Hz), F1..F4.
(def f1 (* fshift-h (+ (* w0 350) (* w1 400) (* w2 650) (* w3 400) (* w4 290))))
(def f2 (* fshift-h (+ (* w0 600) (* w1 800) (* w2 1080) (* w3 1700) (* w4 1870))))
(def f3 (* fshift-h (+ (* w0 2700) (* w1 2600) (* w2 2650) (* w3 2600) (* w4 2800))))
(def f4 (* fshift-h (+ (* w0 2900) (* w1 2800) (* w2 2900) (* w3 3200) (* w4 3250))))
(def a2 (+ (* w0 0.100) (* w1 0.316) (* w2 0.501) (* w3 0.200) (* w4 0.178)))
(def a3 (+ (* w0 0.141) (* w1 0.251) (* w2 0.447) (* w3 0.251) (* w4 0.126)))
(def a4 (+ (* w0 0.200) (* w1 0.251) (* w2 0.398) (* w3 0.200) (* w4 0.100)))
; Gaussian sigma from resonator bandwidth: sharp (0.42*BW ~ matched -3dB
; width) at resonance=1, soft blur at 0. Scales with the formant shift so
; perceived Q is constant.
(def bwk (* fshift-h (+ 0.42 (* 1.3 (- 1 res-h)))))
(def s1 (* bwk (+ (* w0 40) (* w1 40) (* w2 80) (* w3 70) (* w4 40))))
(def s2 (* bwk (+ (* w0 60) (* w1 80) (* w2 90) (* w3 80) (* w4 90))))
(def s3 (* bwk (+ (* w0 100) (* w1 100) (* w2 120) (* w3 100) (* w4 100))))
(def s4 (* bwk (+ (* w0 120) (* w1 120) (* w2 130) (* w3 120) (* w4 120))))

; --- Synthetic vowel envelope: 4 gaussians + floor + breath HF tilt.
(def d1 (* (- fold-freq f1) (/ 1 s1)))
(def d2 (* (- fold-freq f2) (/ 1 s2)))
(def d3 (* (- fold-freq f3) (/ 1 s3)))
(def d4 (* (- fold-freq f4) (/ 1 s4)))
(def fn2 (* fold-norm fold-norm))
(def env-syn (+ 0.004
                (* breath-h 0.12 fn2 fn2)
                (exp (* -0.5 d1 d1))
                (* a2 (exp (* -0.5 d2 d2)))
                (* a3 (exp (* -0.5 d3 d3)))
                (* a4 (exp (* -0.5 d4 d4)))))

; --- Carrier spectral envelope: constant-Q box smoothing via cumsum + baked
; mirrored windows (the cumsum-soothe trick) -- but averaged in POWER, not log.
; Log-domain averaging of a harmonic source is dominated by the deep valleys
; BETWEEN harmonics, which flattens all formant contrast out of the envelope
; (that bug made the vocoder sound like plain amp gating). Power averaging is
; peak-weighted, which is what an envelope is. Wide windows here so whitening
; removes timbre, not individual harmonics. smean separates the carrier's
; LEVEL from its SHAPE so whitening doesn't compress dynamics on the
; synthetic path.
(def pow-c (* mag mag))
(def prefix (cumsum pow-c))
(def smooth (* 0.5 (log (+ (* (+ (- (gather prefix cq-hi) (gather prefix cq-lo)) (gather pow-c cq-lo)) cq-ic) 0.000000001))))
(def smean (mean smooth))

; --- Sidechain (modulator) envelope: power-domain again but through the
; NARROW window set (+/- max(80 Hz, 0.13f): formant resolution; reusing the
; wide whitening windows blurs F1/F2 into one lump). Then formant-shift by
; lerp-gather along the folded axis (gather truncates -- manual lerp), then
; resonance contrast-expansion around its own mean.
(def pow-sc (* mag-sc mag-sc))
(def prefix-sc (cumsum pow-sc))
(def smooth-sc (* 0.5 (log (+ (* (+ (- (gather prefix-sc cqn-hi) (gather prefix-sc cqn-lo)) (gather pow-sc cqn-lo)) cqn-ic) 0.000000001))))
(def warp-idx (min (* fold-idx (/ 1 fshift-h)) 2046))
(def wlo (floor warp-idx))
(def wfr (- warp-idx wlo))
(def sc-warp (+ (* (gather smooth-sc wlo) (- 1 wfr))
                (* (gather smooth-sc (+ wlo 1)) wfr)))
(def scmean (mean smooth-sc))
(def sc-env (+ sc-warp (* res-h 1.6 (- sc-warp scmean))))

; Sidechain presence gate: vocoder only takes over while the sidechain
; actually carries energy, so pauses fall back to the vowel engine instead of
; silence. (mean mag-sc) ~ broadband level; the 12 gain makes ~ -30 dBFS
; material fully open it.
(def gate (min 1 (* (mean mag-sc) 40)))
(def v (* voice-h gate))

; --- Unified gain, log domain.
;   synthetic (v=0): gain = body*(log(env-syn) - (smooth - smean)) -- shape
;     replaced, carrier level/dynamics kept, +0.8 makeup for the energy the
;     formant filter removes.
;   vocoder (v=1): gain = body*(sc-env - smooth - LOGK) -- carrier fully
;     whitened, output follows the VOICE's envelope and dynamics (that's the
;     vocoder behavior); LOGK calibrates "normal voice level -> unity".
; breath adds symmetric per-hop noise roughening. Cap at gain <= ~20 so
; whitening can never blow up on near-silent carrier bins.
(def nfold (gather (noise @size 2048 @hop 512) fold-idx))
(def env-log (+ (* (- 1 v) (+ (log env-syn) 3.5)) (* v (- sc-env 5.8))))
; ALIEN 1: crossfade toward the permuted-axis read of the same envelope.
(def env-scram (+ (* (- 1 scram-h) env-log) (* scram-h (gather env-log scram-idx))))
; ALIEN 2: hold the whole mouth between clock firings.
(def env-held (latch env-scram follow))
(def gain-log (+ (* body-h (- env-held smooth))
                 (* body-h (- 1 v) smean)
                 (* breath-h 1.2 nfold)))
(def gain (exp (min gain-log 4.5)))

(def wet-re-l (* re-l gain))
(def wet-im-l (* im-l gain))
(def wet-re-r (* re-r gain))
(def wet-im-r (* im-r gain))

; Mix against the STFT bypass (not raw input) so dry/wet stay window-aligned;
; tanh caps the whitening worst case.
(def bypass-l (overlap-add (* (ifft re-l im-l @N 2048 @backend accelerated) win) 512))
(def bypass-r (overlap-add (* (ifft re-r im-r @N 2048 @backend accelerated) win) 512))
(def wet-l (tanh (overlap-add (* (ifft wet-re-l wet-im-l @N 2048 @backend accelerated) win) 512)))
(def wet-r (tanh (overlap-add (* (ifft wet-re-r wet-im-r @N 2048 @backend accelerated) win) 512)))

(out (* output (mix bypass-l wet-l mix)) 1 @name left)
(out (* output (mix bypass-r wet-r mix)) 2 @name right)
