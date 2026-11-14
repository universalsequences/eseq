; Phase-vocoder spectral freeze.
;
; Measures each bin's TRUE frequency from its phase change between hops, then
; keeps advancing the held bins at that frequency, so a frozen 440 Hz note
; stays at 440 Hz with a steady level (spectral-bin-freeze re-plays one held
; frame per hop and buzzes on the hop-rate grid instead).
;
; Because the frozen sound is stored as magnitude + true frequency per bin, it
; can be played with after the fact:
;   pitch  transposes the frozen sound (bins are re-read at k/ratio and their
;          frequencies scaled), +/-12 semitones
;   feed   lets new input louder than the held bins join the freeze, so you
;          can stack notes into a chord or drone
;   fade   lets the frozen bins die away (0 = hold forever)
;   haze   random-walks the phases: tonal -> airy
;   tilt   darkens or brightens the frozen spectrum, +/-3 dB per octave
; All five only act while frozen; unfrozen the wet path is the input.
;
; Only bins 0..1024 are synthesised (doubled, upper half zeroed) and the ifft's
; real part is used, so nothing here has to be mirrored about Nyquist.
;
; Stereo is analysed per channel. The dry signal runs through the same STFT so
; dry and frozen stay time-aligned. mix only matters once frozen: 1 is the
; frozen sound alone, lower values let the live input play over it.

; Both the dry and wet paths come out exactly 2047 samples late (one 2048
; window minus one sample, measured against noise at 128- and 512-frame
; blocks). The window is a fixed sample count, so this holds at any rate.
(effect-latency 2047)

(def in-l (in 1 @name left))
(def in-r (in 2 @name right))
(def mod1 (in 3 @name mod1 @modulator 1))
(def mod2 (in 4 @name mod2 @modulator 2))
(def mod3 (in 5 @name mod3 @modulator 3))
(def mod4 (in 6 @name mod4 @modulator 4))

(param freeze @min 0   @max 1  @default 0.0 @mod true @mod-mode additive) ; above 0.5 = frozen
(param pitch  @min -12 @max 12 @default 0.0 @mod true @mod-mode additive) ; semitones, frozen sound only
(param feed   @min 0   @max 1  @default 0.0 @mod true @mod-mode additive) ; new input joins the freeze
(param fade   @min 0   @max 1  @default 0.0 @mod true @mod-mode additive) ; 0 = hold forever .. 1 = ~0.2 s
(param haze   @min 0   @max 1  @default 0.0 @mod true @mod-mode additive) ; phase random-walk
(param tilt   @min -1  @max 1  @default 0.0 @mod true @mod-mode additive) ; dark .. bright
(param mix    @min 0   @max 1  @default 1.0)   ; frozen vs live once frozen
(param output @min 0.25 @max 2 @default 1.0)

; 2048 frame, hop 512, sqrt-Hann * 0.7071 for unity gain at 75% overlap.
(def win (* 0.70710678 (sqrt (hann 2048))))
; Unwrapped phase a bin-centre sinusoid advances per hop: k * pi/2.
(def expected (tensor @shape [2048] @file "expected.json"))
(def bin-idx  (tensor @shape [2048] @file "bin_index.json"))
; 1 at DC and Nyquist, 2 for bins 1..1023, 0 above: positive frequencies only.
(def half-w   (tensor @shape [2048] @file "half_weight.json"))
; Octaves from 1 kHz per bin, for tilt.
(def bin-oct  (tensor @shape [2048] @file "bin_oct.json"))

; Every scalar is hop-held before it touches tensor math (a frame-rate scalar
; demotes the whole tensor chain to per-frame execution).
; Freeze is a switch: knob plus modulation, sampled once per hop, frozen while
; the sum is above 0.5. An LFO or envelope crossing 0.5 grabs a new freeze.
(def f (hop-hold (gt (hop-hold (mod freeze) 512) 0.5) 512))
(def ratio-h (hop-hold (pow 2 (/ (mod pitch) 12)) 512))
(def r (hop-hold (+ 1 (* f (- ratio-h 1))) 512))
(def feed-h (hop-hold (mod feed) 512))
; Per-hop gain for T60 = 1 / (5 * fade^3) seconds; exactly 1 at fade = 0.
(def fade-m (mod fade))
(def g-h (hop-hold (exp (* -0.401 fade-m fade-m fade-m)) 512))
(def haze-m (mod haze))
(def jit-h (hop-hold (* f 3.0 haze-m haze-m) 512))
(def tilt-h (hop-hold (* f 0.3466 (mod tilt)) 512))

; Read positions for pitch: output bin k takes its magnitude and phase from
; held bin k / ratio. Bins that would read above Nyquist are silenced.
(def src (/ bin-idx r))
(def valid (lt src 1024.5))
(def src-c (min (max src 0) 1024))
(def src-lo (- src-c (% src-c 1)))
(def src-fr (- src-c src-lo))
(def r-1 (hop-hold (- r 1) 512))
(def shape-gain (* half-w valid (exp (* tilt-h bin-oct))))

; ---- Left channel
(def frame-l (* (reshape (buffer in-l 2048 512) @shape [2048]) win))
(def (re-l im-l) (fft frame-l @N 2048 @backend accelerated))
(def (mag-l phase-l) (polar-fft re-l im-l))

; True per-bin phase advance per hop = bin-centre advance + wrapped deviation.
(make-history prev-phase-l @shape [2048] @hop 512)
(def dphi-l (- phase-l (read-history prev-phase-l)))
(write-history prev-phase-l phase-l)
(def adv-l (+ expected (wrap (- dphi-l expected) -3.14159265 3.14159265)))

; Frozen state: held bins fade by g; a live bin louder than what is held
; (scaled by feed) replaces it, bringing its own frequency with it.
(make-history held-mag-l @shape [2048] @hop 512)
(make-history held-adv-l @shape [2048] @hop 512)
(def kept-l (* g-h (read-history held-mag-l)))
(def cand-l (* feed-h mag-l))
(def take-l (gt cand-l kept-l))
(def fmag-l (+ (* take-l cand-l) (* (- 1 take-l) kept-l)))
(def fadv-l (+ (* take-l adv-l) (* (- 1 take-l) (read-history held-adv-l))))
(def mag-n-l (+ (* f fmag-l) (* (- 1 f) mag-l)))
(def adv-n-l (+ (* f fadv-l) (* (- 1 f) adv-l)))
(write-history held-mag-l mag-n-l)
(write-history held-adv-l adv-n-l)

; Held-bin phase: follow the input live, accumulate the held advance frozen.
(make-history synth-phase-l @shape [2048] @hop 512)
; Per-bin random walk for haze. The built-in noise op costs ~80% of a core
; here (it is not hop-gated), so each bin runs its own cheap chaotic
; generator instead: x <- frac(x * 1103.515 + seed), seeded per bin from a
; baked table (a different table per channel keeps L and R decorrelated).
(def seed-l (tensor @shape [2048] @file "rand_seed_l.json"))
(make-history rand-l @shape [2048] @hop 512)
(def rnd-l (% (+ (* 1103.515 (read-history rand-l)) seed-l) 1))
(write-history rand-l rnd-l)
(def jit-l (* jit-h (- (* 2 rnd-l) 1)))
(def acc-l (wrap (+ (read-history synth-phase-l) adv-n-l jit-l) -3.14159265 3.14159265))
(def ph-n-l (+ (* f acc-l) (* (- 1 f) phase-l)))
(write-history synth-phase-l ph-n-l)

; Pitch. A second accumulator carries the extra rotation (ratio - 1) * advance
; per held bin, so held + extra turns at ratio * the true frequency. Bin
; phases are moved as "phase at the frame centre" (bin phase + c*k, c =
; pi*2047/2048): neighbouring bins of one partial agree on that value, so the
; partial stays coherent when it is re-read at other bins. Magnitude is lerped
; between the two source bins; phase comes from the nearest (gather
; truncates, so +0.5 rounds).
(make-history shift-phase-l @shape [2048] @hop 512)
(def psi-l (* f (wrap (+ (read-history shift-phase-l) (* r-1 adv-n-l)) -3.14159265 3.14159265)))
(write-history shift-phase-l psi-l)
(def centre-l (+ ph-n-l psi-l (* 3.14005876 bin-idx)))
(def m-out-l (+ (* (- 1 src-fr) (gather mag-n-l src-lo))
                  (* src-fr (gather mag-n-l (+ src-lo 1)))))
(def moved-l (- (gather centre-l (+ src-c 0.5)) (* 3.14005876 bin-idx)))
(def p-out-l (+ (* f moved-l) (* (- 1 f) phase-l)))

; The +/-2 clip is only a safety net: feed at 1 on dense input plus a bright
; tilt can stack well past full scale. Normal levels never reach it.
(def (wre-l wim-l) (rect-fft (hop-hold (* m-out-l shape-gain) 512) (hop-hold p-out-l 512)))
(def wet-l (clip (overlap-add (* (ifft wre-l wim-l @N 2048 @backend accelerated) win) 512) -2 2))
(def dry-l (overlap-add (* (ifft re-l im-l @N 2048 @backend accelerated) win) 512))

; ---- Right channel
(def frame-r (* (reshape (buffer in-r 2048 512) @shape [2048]) win))
(def (re-r im-r) (fft frame-r @N 2048 @backend accelerated))
(def (mag-r phase-r) (polar-fft re-r im-r))

; True per-bin phase advance per hop = bin-centre advance + wrapped deviation.
(make-history prev-phase-r @shape [2048] @hop 512)
(def dphi-r (- phase-r (read-history prev-phase-r)))
(write-history prev-phase-r phase-r)
(def adv-r (+ expected (wrap (- dphi-r expected) -3.14159265 3.14159265)))

; Frozen state: held bins fade by g; a live bin louder than what is held
; (scaled by feed) replaces it, bringing its own frequency with it.
(make-history held-mag-r @shape [2048] @hop 512)
(make-history held-adv-r @shape [2048] @hop 512)
(def kept-r (* g-h (read-history held-mag-r)))
(def cand-r (* feed-h mag-r))
(def take-r (gt cand-r kept-r))
(def fmag-r (+ (* take-r cand-r) (* (- 1 take-r) kept-r)))
(def fadv-r (+ (* take-r adv-r) (* (- 1 take-r) (read-history held-adv-r))))
(def mag-n-r (+ (* f fmag-r) (* (- 1 f) mag-r)))
(def adv-n-r (+ (* f fadv-r) (* (- 1 f) adv-r)))
(write-history held-mag-r mag-n-r)
(write-history held-adv-r adv-n-r)

; Held-bin phase: follow the input live, accumulate the held advance frozen.
(make-history synth-phase-r @shape [2048] @hop 512)
; Per-bin random walk for haze. The built-in noise op costs ~80% of a core
; here (it is not hop-gated), so each bin runs its own cheap chaotic
; generator instead: x <- frac(x * 1103.515 + seed), seeded per bin from a
; baked table (a different table per channel keeps L and R decorrelated).
(def seed-r (tensor @shape [2048] @file "rand_seed_r.json"))
(make-history rand-r @shape [2048] @hop 512)
(def rnd-r (% (+ (* 1103.515 (read-history rand-r)) seed-r) 1))
(write-history rand-r rnd-r)
(def jit-r (* jit-h (- (* 2 rnd-r) 1)))
(def acc-r (wrap (+ (read-history synth-phase-r) adv-n-r jit-r) -3.14159265 3.14159265))
(def ph-n-r (+ (* f acc-r) (* (- 1 f) phase-r)))
(write-history synth-phase-r ph-n-r)

; Pitch. A second accumulator carries the extra rotation (ratio - 1) * advance
; per held bin, so held + extra turns at ratio * the true frequency. Bin
; phases are moved as "phase at the frame centre" (bin phase + c*k, c =
; pi*2047/2048): neighbouring bins of one partial agree on that value, so the
; partial stays coherent when it is re-read at other bins. Magnitude is lerped
; between the two source bins; phase comes from the nearest (gather
; truncates, so +0.5 rounds).
(make-history shift-phase-r @shape [2048] @hop 512)
(def psi-r (* f (wrap (+ (read-history shift-phase-r) (* r-1 adv-n-r)) -3.14159265 3.14159265)))
(write-history shift-phase-r psi-r)
(def centre-r (+ ph-n-r psi-r (* 3.14005876 bin-idx)))
(def m-out-r (+ (* (- 1 src-fr) (gather mag-n-r src-lo))
                  (* src-fr (gather mag-n-r (+ src-lo 1)))))
(def moved-r (- (gather centre-r (+ src-c 0.5)) (* 3.14005876 bin-idx)))
(def p-out-r (+ (* f moved-r) (* (- 1 f) phase-r)))

(def (wre-r wim-r) (rect-fft (hop-hold (* m-out-r shape-gain) 512) (hop-hold p-out-r 512)))
(def wet-r (clip (overlap-add (* (ifft wre-r wim-r @N 2048 @backend accelerated) win) 512) -2 2))
(def dry-r (overlap-add (* (ifft re-r im-r @N 2048 @backend accelerated) win) 512))

(out (* output (mix dry-l wet-l mix)) 1 @name left)
(out (* output (mix dry-r wet-r mix)) 2 @name right)
