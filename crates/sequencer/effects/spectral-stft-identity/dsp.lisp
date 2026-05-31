; Diagnostic: stereo STFT identity path with no spectral processing.
; If this collapses one side in the live app, the issue is in live effect
; routing/process sizing rather than the cumsum soothe detector.

(def in-l (in 1 @name signal-l))
(def in-r (in 2 @name signal-r))

(param mix @min 0 @max 1 @default 1.0)
(param output @min 0.25 @max 2 @default 1.0)

(def win (sqrt (hann 1024)))

(def frame-l (* (reshape (buffer in-l 1024 512) @shape [1024]) win))
(def (re-l im-l) (fft frame-l @N 1024 @backend accelerated))
(def wet-frame-l (ifft re-l im-l @N 1024 @backend accelerated))
(def wet-l (* output (overlap-add (* wet-frame-l win) 512)))

(def frame-r (* (reshape (buffer in-r 1024 512) @shape [1024]) win))
(def (re-r im-r) (fft frame-r @N 1024 @backend accelerated))
(def wet-frame-r (ifft re-r im-r @N 1024 @backend accelerated))
(def wet-r (* output (overlap-add (* wet-frame-r win) 512)))

(out (mix in-l wet-l mix) 1 @name left)
(out (mix in-r wet-r mix) 2 @name right)
