; Phase-vocoder spectral pitch shifter.

(def in-l (in 1 @name signal-l))
(def in-r (in 2 @name signal-r))
(def input (* 0.5 (+ in-l in-r)))

(param ratio @min 0.5 @max 2.0 @default 1.25)
(param color @min 1000 @max 16000 @default 11000)
(param mix @min 0 @max 1 @default 0.55)
(param drive @min 0.25 @max 2.5 @default 1.0)

(def win (hann 1024))
(def frame (* (reshape (buffer input 1024 256) @shape [1024]) win))
(def (re im) (fft frame @N 1024 @backend accelerated))
(def (shift-re shift-im) (phase-vocoder re im ratio @N 1024 @hop 256))
(def shifted (overlap-add (* (ifft shift-re shift-im @N 1024 @backend accelerated) win) 256))
(def wet (tanh (* drive (biquad shifted color 0.707 1 0))))

(out (mix in-l wet mix) 1 @name left)
(out (mix in-r wet mix) 2 @name right)
