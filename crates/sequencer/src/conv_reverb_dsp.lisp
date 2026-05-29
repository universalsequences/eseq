; Convolution Reverb — true stereo (independent L/R IRs)
; N=1024, hop=512, K=128 partitions (~1.5s IR @ 44.1k).
;
; irL_re/irL_im/irR_re/irR_im are named mutable tensors. The Rust host fills
; them at runtime with partition-ir-style FFT'd impulse-response data via the
; GE_WRITE_NODE_STATE bulk-write command (see crates/sequencer/src/conv_reverb.rs).
; This is a builtin effect whose DSP body is dgenlisp; do not edit casually —
; the tensor names/shapes and hop/N are contracts with the Rust IR pipeline.
;
; Keep the live path in the same block format as `partition-ir`: each hop-sized
; time block is zero-padded to N before the FFT. FFTing an overlapping N-sample
; window makes this an STFT-style convolver instead of partitioned OLA; using a
; rectangular overlapping N-window exposes circular wrap/overlap artifacts, and
; adding Hann windows only masks them while adding block-rate coloration.

(def inL (in 1 @name left))
(def inR (in 2 @name right))

(def wet-amt (param mix @min 0 @max 1 @default 0.35))
(def out-gain (param gain @min 0 @max 8 @default 1.0))

; ---- Left ----
(def irL-re (wavetable-param @shape [128 1024] @name irL_re))
(def irL-im (wavetable-param @shape [128 1024] @name irL_im))
(def blockL (reshape (buffer inL 512 512) @shape [512]))
(def fftBlockL (pad blockL @padding [0:512]))
(def (xLre xLim) (fft fftBlockL @N 1024 @backend accelerated))
(def (yLre yLim) (partitioned-spectral-mac xLre xLim irL-re irL-im @N 1024))
(def tdL (ifft yLre yLim @N 1024))
(def wetL (overlap-add (* tdL out-gain) 512))

; ---- Right ----
(def irR-re (wavetable-param @shape [128 1024] @name irR_re))
(def irR-im (wavetable-param @shape [128 1024] @name irR_im))
(def blockR (reshape (buffer inR 512 512) @shape [512]))
(def fftBlockR (pad blockR @padding [0:512]))
(def (xRre xRim) (fft fftBlockR @N 1024 @backend accelerated))
(def (yRre yRim) (partitioned-spectral-mac xRre xRim irR-re irR-im @N 1024))
(def tdR (ifft yRre yRim @N 1024))
(def wetR (overlap-add (* tdR out-gain) 512))

(out (mix inL wetL wet-amt) 1 @name left)
(out (mix inR wetR wet-amt) 2 @name right)
