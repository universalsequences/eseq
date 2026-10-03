; release-window — 1 while the gate is held and for release_ms after it
; falls, else 0. A stand-in envelope for voice-amp when the real amp envelope
; lives inside a sub-voice (block-gate, unison copies) or does not exist
; (physical models): the voice is kept for at least its nominal release, and
; voice-amp's output check covers anything still ringing after it.
;   (out (voice-amp (release-window gate amp_release_ms) left right) 3 @amp true)
(defmacro release-window (gate release_ms)
  (make-history release_window_left)
  (def release_window_held (gt gate 0.5))
  (def release_window_value
    (gswitch release_window_held (* (max 0 release_ms) 0.001 samplerate)
      (max 0 (- (read-history release_window_left) 1))))
  (write-history release_window_left release_window_value)
  (max release_window_held (gt release_window_value 0)))
