; UZU-style spectral phaser with a continuous phaser <-> spectral morph.
;
; Signal path: STFT -> swept notch comb (a clean zero-phase filter) -> optional
; spectral ROTATION (a circular bin-roll of the complex spectrum: real cross-bin
; energy movement, the thing a magnitude filter physically cannot do) -> ISTFT.
;
; `blur` is the morph: 0 = pure comb phaser (analog-ish, source intact), 1 = the
; fully rotating "alien" spectral sound. Everything between blends smoothly, so
; one knob walks you from one world to the other. `speed` sets the motion rate
; for both the comb sweep and the rotation.

(def in-l (in 1 @name left))
(def in-r (in 2 @name right))

(param width   @min 2    @max 160 @default 28)   ; notch spacing (comb density)
(param offset  @min 0    @max 1   @default 0.0)  ; manual comb position
(param depth   @min 0    @max 1   @default 0.88) ; notch depth + sharpness
(param speed   @min 0    @max 8   @default 0.18) ; motion rate (sweep + rotation)
(param mix     @min 0    @max 1   @default 0.70) ; wet/dry
(param blur    @min 0    @max 1   @default 0.0)  ; PHASER (0) <-> SPECTRAL (1) morph
(param curve   @min 0    @max 1   @default 0.35) ; HZ <-> OCT weighting
(param fast    @min 0    @max 1   @default 0.0)  ; 1x / 5x speed multiplier
(param lowkeep @min 0    @max 1   @default 1.0)  ; preserve lows under ~250 Hz
(param output  @min 0.25 @max 2   @default 1.0)

; --- STFT analysis. 2048 frame, 512 hop = 75% overlap, sqrt-Hann. 512 hop is
; required: the live sequencer runs custom effects in 512-frame blocks. Hann at
; hop=N/4 sums to 2.0, so the 0.7071 (=1/√2) window scale on both analysis and
; synthesis restores unity-gain overlap-add.
(def win (* 0.70710678 (sqrt (hann 2048))))
(def frame-l (* (reshape (buffer in-l 2048 512) @shape [2048]) win))
(def frame-r (* (reshape (buffer in-r 2048 512) @shape [2048]) win))
(def (re-l im-l) (fft frame-l @N 2048 @backend accelerated))
(def (re-r im-r) (fft frame-r @N 2048 @backend accelerated))

; --- Notch grid position, bent between equal-Hz and octave spacing by `curve`.
(def bin-hz   (tensor @shape [2048] @file "bin_hz_norm.json"))
(def bin-log  (tensor @shape [2048] @file "bin_log_norm.json"))
(def low-mask (tensor @shape [2048] @file "low_protect_mask.json"))
(def bin-pos  (+ (* bin-hz (- 1 curve)) (* bin-log curve)))

; --- Motion: one LFO drives both the comb sweep and the spectral rotation.
(def lfo-speed (* speed (+ 1 (* 4 fast))))
(def sweep (hop-hold (phasor lfo-speed) 512))
(def comb-l (+ (* bin-pos width) offset sweep))
(def comb-r (+ comb-l 0.03))

; --- Notch shape. peak = 1 at each notch center, 0 in the passband; a steep
; power makes a narrow null, and depth->1 drives the center to a true zero.
(def sharpness (+ 2 (* 6 depth)))
(def peak-l (* 0.5 (+ 1 (cos (* twopi comb-l)))))
(def peak-r (* 0.5 (+ 1 (cos (* twopi comb-r)))))
(def null-l (pow peak-l sharpness))
(def null-r (pow peak-r sharpness))
(def active (+ (- 1 lowkeep) (* lowkeep low-mask)))
(def notch-l (* depth active null-l))
(def notch-r (* depth active null-r))

; --- Apply the comb as a zero-phase filter directly on the complex spectrum.
(def gain-l (max (- 1 notch-l) 0.0))
(def gain-r (max (- 1 notch-r) 0.0))
(def comb-re-l (* re-l gain-l))
(def comb-im-l (* im-l gain-l))
(def comb-re-r (* re-r gain-r))
(def comb-im-r (* im-r gain-r))

; --- BLUR = DEPTH of the spectral warp (NOT a wet/dry crossfade). A smooth
; bipolar roll displaces the comb-filtered spectrum by up to +/- blur*1024 bins;
; `blur` sets how FAR it warps, `speed` sets the rate (shared with the comb
; sweep). blur=0 -> no warp (pure comb phaser); higher blur warps the spectrum
; further each cycle -> progressively deeper spectral mangling. The roll is a sin
; so it's smooth across the sweep wrap, and the gather is circular. Uses the
; dynamic per-frame gather.
(def bin-index (tensor @shape [2048] @file "bin_index.json"))
(def warp (* (* blur 1024) (sin (* twopi sweep))))
(def roll-idx (wrap (- bin-index warp) 0 2048))
(def wet-re-l (gather comb-re-l roll-idx))
(def wet-im-l (gather comb-im-l roll-idx))
(def wet-re-r (gather comb-re-r roll-idx))
(def wet-im-r (gather comb-im-r roll-idx))

; --- ISTFT, mixed against the STFT bypass so dry/wet stay window-aligned.
(def bypass-frame-l (ifft re-l im-l @N 2048 @backend accelerated))
(def bypass-frame-r (ifft re-r im-r @N 2048 @backend accelerated))
(def wet-frame-l (ifft wet-re-l wet-im-l @N 2048 @backend accelerated))
(def wet-frame-r (ifft wet-re-r wet-im-r @N 2048 @backend accelerated))
(def bypass-l (overlap-add (* bypass-frame-l win) 512))
(def bypass-r (overlap-add (* bypass-frame-r win) 512))
(def wet-l (overlap-add (* wet-frame-l win) 512))
(def wet-r (overlap-add (* wet-frame-r win) 512))

(out (* output (mix bypass-l wet-l mix)) 1 @name left)
(out (* output (mix bypass-r wet-r mix)) 2 @name right)
