; Soothe-style spectral resonator tamer.
; Demonstrates cumsum spectral smoothing plus hop-gated tensor history inside a
; DGenLisp STFT path.

(def in-l (in 1 @name left))
(def in-r (in 2 @name right))

(param amount @min 0 @max 100 @default 4.0)
(param threshold @min 0 @max 3 @default 0.35)
(param gate @min -2 @max 2 @default -1.0)
(param low @min 0 @max 1 @default 0.0)
(param high @min 0 @max 1 @default 1.0)
(param attack @min 0 @max 0.9 @default 0.15)
(param release @min 0.6 @max 0.995 @default 0.92)
(param freeze @min 0 @max 1 @default 0.0)
(param hold @min 0 @max 1 @default 0.0)
(param alien @min 0 @max 4 @default 0.0)
(param delta @min 0 @max 1 @default 0.0)
(param output @min 0.25 @max 2 @default 1.0)
(param mix @min 0 @max 1 @default 0.45)

; Use a 512-sample hop because the live sequencer processes custom effects in
; 512-frame blocks. A smaller 256 hop needs multiple STFT advances per host
; block, which currently breaks stereo state in the live DGenLisp wrapper.
(def win (sqrt (hann 1024)))
(def frame-l (* (reshape (buffer in-l 1024 512) @shape [1024]) win))
(def frame-r (* (reshape (buffer in-r 1024 512) @shape [1024]) win))
(def (re-l im-l) (fft frame-l @N 1024 @backend accelerated))
(def (re-r im-r) (fft frame-r @N 1024 @backend accelerated))

(def mag-l (sqrt (+ (* re-l re-l) (* im-l im-l))))
(def mag-r (sqrt (+ (* re-r re-r) (* im-r im-r))))
(def mag (* 0.5 (+ mag-l mag-r)))

; Work in log magnitude so the detector follows perceived spectral shape instead
; of letting a few loud bins dominate the reference.
(def log-mag (log (+ mag 0.000001)))

; Constant-Q-ish box smoothing via cumsum + gather. The index tensors encode
; per-bin lo/hi/count windows for a 1/3-octave reference. This is the key step
; that makes the detector frequency-relative instead of broad fixed-width gain
; reduction.
(def cq-lo (tensor @shape [1024] @file "cq_lo.json"))
(def cq-hi (tensor @shape [1024] @file "cq_hi.json"))
(def cq-inv-count (tensor @shape [1024] @file "cq_inv_count.json"))
(def bin-norm (tensor @shape [1024] @file "cq_bin_norm.json"))
(def prefix (cumsum log-mag))
(def smooth (* (+ (- (gather prefix cq-hi) (gather prefix cq-lo)) (gather log-mag cq-lo)) cq-inv-count))

; Only act on bins that rise above the local log envelope by `threshold`, then
; optionally gate/filter the detector curve. `bin-pos` promotes the static bin
; tensor into the hop-rate tensor domain so low/high can be live parameters.
(def contrast (- log-mag smooth))
(def bin-pos (+ bin-norm (* low 0)))
(def band-mask (* (gte bin-pos low) (lte bin-pos high)))
(def gate-mask (gt contrast gate))
(def excess (* band-mask gate-mask (relu (- contrast threshold))))
(def target-gain (exp (* -1 amount excess)))
(def target-reduction (- 1 target-gain))

; Hop-gated per-bin one-pole follower. Storing reduction instead of gain gives
; a natural zero-initialized state: startup is full-bandwidth, then resonant bins
; acquire attenuation as the cumsum-smoothed target rises. `hop-hold` makes the
; hop-rate tensor safe for the per-frame IFFT consumer.
(make-history soothe-reduction @shape [1024] @hop 512)
(def previous-reduction (read-history soothe-reduction))
; For reduction, rising means "attack" and falling means "release". The max of
; the fast and slow one-poles chooses the faster rising value when target is
; above the previous state, and the slower falling value when target is below.
(def attack-reduction (+ (* previous-reduction attack) (* target-reduction (- 1 attack))))
(def release-reduction (+ (* previous-reduction release) (* target-reduction (- 1 release))))
(def next-reduction (max attack-reduction release-reduction))

; Freeze as sample-and-hold: while freeze is off, the latch follows the live
; hop-held reduction curve. When freeze turns on, the latch stops updating and
; the captured filter curve is applied until freeze is released.
(def live-reduction (hop-hold (write-history soothe-reduction next-reduction) 512))
(def freeze-hop (hop-hold freeze 512))
(def freeze-active (hop-hold (gt freeze-hop 0.5) 512))
(def freeze-follow (hop-hold (lte freeze-hop 0.5) 512))
(def latched-reduction (latch live-reduction freeze-follow))
(def selected-reduction (+ (* live-reduction (- 1 freeze-active)) (* latched-reduction freeze-active)))
(def applied-reduction (hop-hold selected-reduction 512))
(def applied-gain (max (- 1 applied-reduction) 0))

; `hold` is the intentionally non-transparent freeze: it freezes the actual
; complex spectrum, not just the detector curve. That produces the obvious
; drone/robot/spectral-smear behavior that filter-curve freeze will not.
(def hold-hop (hop-hold hold 512))
(def hold-active (hop-hold (gt hold-hop 0.5) 512))
(def hold-follow (hop-hold (lte hold-hop 0.5) 512))
(def held-re-l (latch re-l hold-follow))
(def held-im-l (latch im-l hold-follow))
(def held-re-r (latch re-r hold-follow))
(def held-im-r (latch im-r hold-follow))
(def source-re-l (+ (* re-l (- 1 hold-active)) (* held-re-l hold-active)))
(def source-im-l (+ (* im-l (- 1 hold-active)) (* held-im-l hold-active)))
(def source-re-r (+ (* re-r (- 1 hold-active)) (* held-re-r hold-active)))
(def source-im-r (+ (* im-r (- 1 hold-active)) (* held-im-r hold-active)))

(def gre-l (* source-re-l applied-gain))
(def gim-l (* source-im-l applied-gain))
(def gre-r (* source-re-r applied-gain))
(def gim-r (* source-im-r applied-gain))

; sqrt-Hann on analysis and synthesis gives Hann-squared-equivalent energy, and
; Hann is COLA-normalized at 50% overlap.
(def bypass-frame-l (ifft re-l im-l @N 1024 @backend accelerated))
(def bypass-frame-r (ifft re-r im-r @N 1024 @backend accelerated))
(def wet-frame-l (ifft gre-l gim-l @N 1024 @backend accelerated))
(def wet-frame-r (ifft gre-r gim-r @N 1024 @backend accelerated))
(def bypass-l (overlap-add (* bypass-frame-l win) 512))
(def bypass-r (overlap-add (* bypass-frame-r win) 512))
(def wet-l (overlap-add (* wet-frame-l win) 512))
(def wet-r (overlap-add (* wet-frame-r win) 512))
(def normal-l (* output (mix bypass-l wet-l mix)))
(def normal-r (* output (mix bypass-r wet-r mix)))
(def delta-l (* output (- bypass-l wet-l)))
(def delta-r (* output (- bypass-r wet-r)))
(def styled-l (+ normal-l (* alien delta-l)))
(def styled-r (+ normal-r (* alien delta-r)))

(out (mix styled-l delta-l delta) 1 @name left)
(out (mix styled-r delta-r delta) 2 @name right)
