; eseq-yxau: shared scalar feedback downstream of stereo overlap-add.
; This is an identity transform followed by a single audio-rate gain smoother.
; It must not change behavior when the host block contains multiple FFT hops.
; On macOS DGenLisp v0.1.8, blocks 256/512 corrupt the left channel, whereas
; blocks 64/128 do not. Fixed in v0.1.9: scalar regions preserve cadence and
; independent static setup no longer splits the shared hop island lifetime.

(def left (in 1 @name Left))
(def right (in 2 @name Right))
(param gain @default 1 @min 0 @max 2)

(defmacro smooth (signal hz)
  (make-history previous)
  (def coefficient (min 1 (/ (* twopi hz) samplerate)))
  (write-history previous (+ (read-history previous) (* coefficient (- signal (read-history previous))))))

(def win (hann 512))
(def frame-l (* (reshape (buffer left 512 128) @shape [512]) win))
(def frame-r (* (reshape (buffer right 512 128) @shape [512]) win))
(def (re-l im-l) (fft frame-l @N 512 @backend accelerated))
(def (re-r im-r) (fft frame-r @N 512 @backend accelerated))
(def time-l (ifft re-l im-l @N 512 @backend accelerated))
(def time-r (ifft re-r im-r @N 512 @backend accelerated))
(def output-gain (smooth gain 100))
(out (* output-gain (/ (overlap-add (* time-l win) 128) 1.5)) 1 @name Left)
(out (* output-gain (/ (overlap-add (* time-r win) 128) 1.5)) 2 @name Right)
