; Hop-rate spectral cloud gate.
; Density opens sparse spectral islands, smear blurs bin clusters, motion pulses the
; mask, and stereo widens one shared resynthesis path without using spectral delay.

(def in-l (in 1 @name signal-l))
(def in-r (in 2 @name signal-r))
(def input (* 0.5 (+ in-l in-r)))

(param density @min 0.03 @max 1 @default 0.32)
(param smear @min 0 @max 1 @default 0.35)
(param motion @min 0.02 @max 12 @default 1.6)
(param phase_amt @min 0 @max 1 @default 0.45)
(param width @min 0 @max 1 @default 0.55)
(param mix @min 0 @max 1 @default 0.65)

(def win (hann 1024))
(def frame (* (reshape (buffer input 1024 256) @shape [1024]) win))
(def (re im) (fft frame @N 1024 @backend accelerated))
(def (mag phase) (polar-fft re im))

(def blur-k (tensor @shape [9] @data [0.02 0.04 0.08 0.14 0.44 0.14 0.08 0.04 0.02]))
(def blurred-mag (conv1d mag blur-k))
(def base-mag (+ (* (- 1 smear) mag) (* smear blurred-mag)))

(def threshold (- 1 density))
(def pulse (+ 0.45 (* 0.55 (sin (* twopi (hop-hold (phasor motion) 256))))))
(def floor (* 0.08 density))

(def raw (abs (noise @size 1024 @hop 256)))
(def island (tanh (* 10 (relu (+ raw (* -1 threshold))))))
(def mask (+ floor (* pulse island)))

(def rand-phase (* (noise @size 1024 @hop 256) twopi))
(def cloud-mag (* base-mag mask))
(def cloud-phase (+ (* (- 1 phase_amt) phase) (* phase_amt rand-phase)))

(def (cre cim) (rect-fft cloud-mag cloud-phase))
(def wet (overlap-add (* (ifft cre cim @N 1024 @backend accelerated) win) 256))
(def side (delay wet 29))

(out (mix in-l (mix wet side (* 0.5 width)) mix) 1 @name left)
(out (mix in-r (mix wet side (- 1 (* 0.5 width))) mix) 2 @name right)
