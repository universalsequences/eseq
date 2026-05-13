; Lightweight spectral bin hold/smear.
; Uses one FFT/IFFT path. Freeze crossfades toward hop-held bins; smear blurs
; magnitude only, so it stays audible without the tensor-history cost.

(def in-l (in 1 @name signal-l))
(def in-r (in 2 @name signal-r))
(def input (* 0.5 (+ in-l in-r)))

(param freeze @min 0 @max 1 @default 0.0)
(param smear @min 0 @max 1 @default 0.0)
(param tone @min 800 @max 18000 @default 14000)
(param mix @min 0 @max 1 @default 0.8)
(param width @min 0 @max 1 @default 0.75)

(def win (hann 1024))
(def frame (* (reshape (buffer input 1024 256) @shape [1024]) win))
(def (re im) (fft frame @N 1024 @backend accelerated))
(def (mag phase) (polar-fft re im))

(def smear-k (tensor @shape [9] @data [0.02 0.04 0.08 0.14 0.44 0.14 0.08 0.04 0.02]))
(def smeared-mag (+ (* (- 1 smear) mag) (* smear (conv1d mag smear-k))))
(def (sre sim) (rect-fft smeared-mag phase))

(def hold-re (hop-hold sre 8192))
(def hold-im (hop-hold sim 8192))
(def frozen-re (+ (* (- 1 freeze) sre) (* freeze hold-re)))
(def frozen-im (+ (* (- 1 freeze) sim) (* freeze hold-im)))

(def td (ifft frozen-re frozen-im @N 1024 @backend accelerated))
(def wet (biquad (overlap-add (* td win) 256) tone 0.6 1 0))
(def side (delay wet 37))

(out (mix in-l (mix wet side (* 0.5 (- 1 width))) mix) 1 @name left)
(out (mix in-r (mix side wet (* 0.5 (- 1 width))) mix) 2 @name right)
