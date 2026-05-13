; DGenLisp "Jet" Stereo Flanger
; Aggressive, wide flanging with high feedback and deep modulation

(def sig (in 1 @name signal))

(def rate (param rate @min 0.01 @max 10 @default 0.15))
(def depth (param depth @min 0 @max 1 @default 0.8))
(def manual (param manual @min 1 @max 800 @default 250))
(def fbk (param feedback @min -0.98 @max 0.98 @default 0.85))
(def mix-amt (param mix @min 0 @max 1 @default 0.5))
(def width (param width @min 0 @max 1 @default 0.5))

; Dual LFOs: 0 and 180 degrees (phase inversion for stereo)
(def ph1 (phasor rate))
(def ph2 (% (+ ph1 (* width 0.5)) 1.0)) 

(def lfo1 (sin (* twopi ph1)))
(def lfo2 (sin (* twopi ph2)))

; Delay modulation: Base delay is 'manual' samples. 
; Depth modulates from almost 0 up to 2 * manual.
; Classic flanging works best when delay approaches zero.
(def dt1 (+ 1 (* manual (+ 1 (* lfo1 depth)))))
(def dt2 (+ 1 (* manual (+ 1 (* lfo2 depth)))))

(make-history h1)
(make-history h2)

; Read previous output for feedback
(def fbk-in1 (read-history h1))
(def fbk-in2 (read-history h2))

; Delay lines with feedback
; High feedback creates the resonant "jet" whistle.
(def d1 (delay (+ sig (* fbk fbk-in1)) dt1))
(def d2 (delay (+ sig (* fbk fbk-in2)) dt2))

; Update history
(write-history h1 d1)
(write-history h2 d2)

; Final stereo output: Mix dry and wet
(out (mix sig d1 mix-amt) 1 @name Left)
(out (mix sig d2 mix-amt) 2 @name Right)