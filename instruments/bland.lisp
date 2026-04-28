; DGenLisp instrument — generates audio from gate, pitch, velocity, trigger, and shared mod buses
; Inputs: gate (ch 1), pitch_hz (ch 2), velocity (ch 3), trigger (ch 4)
; Mod inputs: mod1..mod6 (ch 5..10)
; Output: audio (ch 1)
; Helpers injected at compile time: adsr/modulation macros

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))
(def mod5 (in 9 @name mod5 @modulator 5))
(def mod6 (in 10 @name mod6 @modulator 6))
(def osc (sin (* (phasor pitch) twopi)))
(out (* osc gate velocity) 1 @name audio)
