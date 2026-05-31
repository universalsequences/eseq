; UZU-style spectral notch phaser.
; Frequency-domain phaser: STFT -> swept magnitude notch comb -> ISTFT.
; Magnitude-only (phase is preserved) so the motion stays smooth and "fluttery"
; the way the reference does -- it never touches phase, it just animates the
; magnitudes rapidly. No randomness: the character is a clean deterministic comb.
;
; Six controls mirror the plugin -- width, offset, depth, speed, mix, blur --
; plus the HZ<->OCT weighting (curve), the 1x/5x multiplier (fast), automatic
; low-end protection (lowkeep), and an output trim.

(def in-l (in 1 @name left))
(def in-r (in 2 @name right))

(param width   @min 2    @max 160 @default 28)   ; #1 distance between notches
(param offset  @min 0    @max 1   @default 0.0)  ; #2 main offset of filter shape
(param depth   @min 0    @max 1   @default 0.88) ; #3 sharpness + depth of notches
(param speed   @min 0    @max 8   @default 0.18) ; #4 automatic offset speed
(param mix     @min 0    @max 1   @default 0.70) ; #5 wet/dry
(param blur    @min 0    @max 1   @default 0.0)  ; #6 round/merge spectral bins
(param curve   @min 0    @max 1   @default 0.35) ; HZ <-> OCT weighting
(param fast    @min 0    @max 1   @default 0.0)  ; 1x / 5x speed multiplier
(param lowkeep @min 0    @max 1   @default 1.0)  ; preserve lows under ~250 Hz
(param output  @min 0.25 @max 2   @default 1.0)

; --- STFT analysis. 2048 frame, 512 hop = 75% overlap, sqrt-Hann. The bigger
; frame is what lets the comb hold hundreds of DEEP notches (1024 tops out near
; -9 dB past ~90 notches). 512 hop is required: the live sequencer runs custom
; effects in 512-frame blocks. Hann at hop=N/4 sums to 2.0, so the 0.7071 (=1/√2)
; window scale on both analysis and synthesis restores unity-gain overlap-add.
(def win (* 0.70710678 (sqrt (hann 2048))))
(def frame-l (* (reshape (buffer in-l 2048 512) @shape [2048]) win))
(def frame-r (* (reshape (buffer in-r 2048 512) @shape [2048]) win))
(def (re-l im-l) (fft frame-l @N 2048 @backend accelerated))
(def (re-r im-r) (fft frame-r @N 2048 @backend accelerated))
(def (mag-l phase-l) (polar-fft re-l im-l))
(def (mag-r phase-r) (polar-fft re-r im-r))

; --- Notch grid position across the spectrum, bent between equal-Hz spacing and
; octave spacing by `curve` (the HZ<->OCT slider). Hz spacing gives the glassy
; full-spectrum comb; the octave side feels more musical in the lows/mids.
(def bin-hz   (tensor @shape [2048] @file "bin_hz_norm.json"))
(def bin-log  (tensor @shape [2048] @file "bin_log_norm.json"))
(def low-mask (tensor @shape [2048] @file "low_protect_mask.json"))
(def bin-pos  (+ (* bin-hz (- 1 curve)) (* bin-log curve)))

; --- The sweep IS the phaser motion: an LFO on the offset slides the whole comb
; up the spectrum. `fast` is the reference's 5x-ish multiplier.
(def lfo-speed (* speed (+ 1 (* 4 fast))))
(def sweep (hop-hold (phasor lfo-speed) 512))

; `width` sets how many notch cycles span the spectrum (the comb spacing).
; A tiny fixed L/R offset keeps the stereo image alive without a stereo control.
(def comb-l (+ (* bin-pos width) offset sweep))
(def comb-r (+ comb-l 0.03))

; --- Notch shape. peak = 1 at each notch center, 0 in the passband. Raising it
; to a steep power gives a narrow null with an open passband, and letting `depth`
; reach 1 drives the center to a TRUE zero (full magnitude kill), which is the
; dramatic spectral sound -- the old version only reached ~-17 dB.
(def sharpness (+ 2 (* 6 depth)))
(def peak-l (* 0.5 (+ 1 (cos (* twopi comb-l)))))
(def peak-r (* 0.5 (+ 1 (cos (* twopi comb-r)))))
(def null-l (pow peak-l sharpness))
(def null-r (pow peak-r sharpness))

; Low-end protection: notches fade out below ~250 Hz unless lowkeep is lowered.
(def active (+ (- 1 lowkeep) (* lowkeep low-mask)))
(def notch-l (* depth active null-l))
(def notch-r (* depth active null-r))

; --- Blur smears the magnitude spectrum across bins (a real spectral blur),
; crossfaded in by `blur`. On the sharp nulls above this rounds bin energy into
; its neighbours -- the notches fill and smear, softening the glassy comb into a
; washier, more diffuse texture. The 17-tap triangular kernel sums to 1, so
; blur=0 is exact bypass and the blur never changes overall level.
(def blur-k (tensor @shape [17] @data
  [0.012346 0.024691 0.037037 0.049383 0.061728 0.074074 0.086420 0.098765
   0.111111
   0.098765 0.086420 0.074074 0.061728 0.049383 0.037037 0.024691 0.012346]))
(def mag-l-blur (conv1d mag-l blur-k))
(def mag-r-blur (conv1d mag-r blur-k))
(def mag-l-soft (+ (* (- 1 blur) mag-l) (* blur mag-l-blur)))
(def mag-r-soft (+ (* (- 1 blur) mag-r) (* blur mag-r-blur)))

; --- Apply the comb as a magnitude-only filter; phase is carried through.
(def gain-l (max (- 1 notch-l) 0.0))
(def gain-r (max (- 1 notch-r) 0.0))
(def (wet-re-l wet-im-l) (rect-fft (* mag-l-soft gain-l) phase-l))
(def (wet-re-r wet-im-r) (rect-fft (* mag-r-soft gain-r) phase-r))

; --- Mix against the STFT bypass path so dry/wet stay latency- and
; window-aligned (sqrt-Hann on analysis + synthesis = Hann, COLA at 50%).
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
