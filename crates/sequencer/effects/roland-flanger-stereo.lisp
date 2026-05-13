; DGenLisp Stereo Rich Flanger (Roland-style)
; Processes mono input into stereo output with cross-phase LFOs and feedback filtering

(def sig (in 1 @name signal))

(def rate (param rate @min 0.01 @max 5 @default 0.2))
(def depth (param depth @min 0 @max 1 @default 0.5))
(def manual (param manual @min 1 @max 500 @default 120))
(def fbk (param feedback @min -0.99 @max 0.99 @default 0.75))
(def color (param color @min 1000 @max 16000 @default 5000))
(def mix-amt (param mix @min 0 @max 1 @default 0.5))

; Dual phase-offset LFOs for stereo width
(def ph1 (phasor rate))
(def ph2 (% (+ ph1 0.25) 1.0)) 

(def lfo1 (sin (* twopi ph1)))
(def lfo2 (sin (* twopi ph2)))

; Delay modulation: Base delay is 'manual' samples, modulated by 'depth'
(def dt1 (+ 1 (* manual (+ 1 (* lfo1 depth)))))
(def dt2 (+ 1 (* manual (+ 1 (* lfo2 depth)))))

(make-history h1)
(make-history h2)

; Read previous output for feedback
(def fbk-in1 (read-history h1))
(def fbk-in2 (read-history h2))

; Apply LPF to feedback for "analog" warmth (Mode 0: LPF)
(def fbk-filt1 (biquad fbk-in1 color 0.5 0 0))
(def fbk-filt2 (biquad fbk-in2 color 0.5 0 0))

; Delay lines with feedback
(def d1 (delay (+ sig (* fbk fbk-filt1)) dt1))
(def d2 (delay (+ sig (* fbk fbk-filt2)) dt2))

; Update history
(write-history h1 d1)
(write-history h2 d2)

; Final stereo output
(out (mix sig d1 mix-amt) 1 @name Left)
(out (mix sig d2 mix-amt) 2 @name Right)