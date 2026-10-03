; voice-amp — the `@amp` flag for an instrument voice: 1 while the voice can
; still be heard, 0 once it is silent, so the host stops running a released
; voice as soon as it is done instead of holding a fixed release tail:
;   (out (voice-amp env_amp left right) 3 @name amp @amp true)
; A voice counts as audible while its amp envelope is above -80 dB, or while
; either output's peak (falling with a 50 ms time constant, so zero crossings
; and short gaps never read as silence) is above -100 dB. The output term
; keeps post-envelope tails alive: in-voice delays, reverbs, ringing filters.
; Pass 0 for env when the voice has no amp envelope (physical models).
(defmacro voice-amp (env left right)
  (make-history voice_amp_peak)
  (def voice_amp_level
    (max (max (abs left) (abs right))
         (* (read-history voice_amp_peak) (exp (/ -20 samplerate)))))
  (write-history voice_amp_peak voice_amp_level)
  (max (gt env 0.0001) (gt voice_amp_level 0.00001)))
