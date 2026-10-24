; Per-mode decay rate (1/s): constant + frequency-proportional loss, plus an
; always-on f^2 air loss (~32/s at 18 kHz, ~0.1/s at 1 kHz) so zero loss
; knobs cannot leave ultrasonic modes ringing forever.
(defmacro modal-loss (freqs const_loss freq_loss)
  (+ const_loss (* freq_loss freqs 0.01) (* freqs freqs 1e-7)))
