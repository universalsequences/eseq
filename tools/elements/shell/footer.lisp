
; ---- outputs: the module's L (aux) and R (main) jacks, then `level_db` on
; top of a makeup gain that brings the resonator models (+12 dB) to about the
; Ominous voice's level ----
(def makeup (gswitch (gt model 2.5) 1 3.98107171))
(def out_level (* makeup (exp (* 0.1151292546 (clip (mod level_db) -60 48)))))
(def voice_left (* out_aux out_level))
(def voice_right (* out_main out_level))
(out voice_left 1 @name left)
(out voice_right 2 @name right)
(use-defmacro voice-amp)
(use-defmacro release-window)
(out (voice-amp (release-window gate 30) voice_left voice_right) 3 @name amp @amp true)
