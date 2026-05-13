; LexiLush - A lush Lexicon-style modulated reverb
; Uses a series of diffusers followed by a modulated figure-eight feedback tank

(def in-l (in 1 @name signal-l))
; If channel 2 is empty (mono), use channel 1
(def in-r (in 2 @name signal-r))
(def in-stereo (+ in-l in-r))

; Parameters
(param pre_dly @min 0 @max 5000 @default 240)
(param size @min 0.5 @max 2.0 @default 1.0)
(param decay @min 0.1 @max 0.98 @default 0.8)
(param damping @min 500 @max 18000 @default 4500)
(param mod_freq @min 0.1 @max 5 @default 0.8)
(param mod_amt @min 0 @max 100 @default 40)
(param diffusion @min 0.1 @max 0.9 @default 0.7)
(param mix @min 0 @max 1 @default 0.35)

; Correct Schroeder Allpass Macro
(defmacro ap (sig g d_samples)
  (make-history h)
  (def node (+ sig (* (read-history h) g)))
  (def delayed (delay node d_samples))
  (write-history h delayed)
  (- delayed (* g node)))

; Stereo Pre-delay
(def pd (delay in-stereo pre_dly))

; Initial Diffusion Stage (Primes for density)
(def d1 (ap pd diffusion (* size 143)))
(def d2 (ap d1 diffusion (* size 281)))
(def d3 (ap d2 diffusion (* size 491)))
(def d4 (ap d3 diffusion (* size 733)))

; Modulated Feedback Tank (Figure-8)
(make-history h-loop-l)
(make-history h-loop-r)

; LFO for "Spin" modulation
(def lfo (sin (* twopi (phasor mod_freq))))
(def lfo-mod (* lfo mod_amt))

; Left Tank Path
(def tank-l-in (+ d4 (* (read-history h-loop-r) decay)))
(def tank-l-ap1 (ap tank-l-in 0.6 (* size 1123)))
; Modulation is inside the loop
(def tank-l-mod (delay tank-l-ap1 (+ (* size 1927) lfo-mod)))
; Biquad gain must be 1 (using 0 might have caused silence)
(def tank-l-damp (biquad tank-l-mod damping 0.5 1 0)) 
(write-history h-loop-l tank-l-damp)

; Right Tank Path
(def tank-r-in (+ d4 (* (read-history h-loop-l) decay)))
(def tank-r-ap1 (ap tank-r-in 0.6 (* size 1373)))
(def tank-r-mod (delay tank-r-ap1 (+ (* size 2341) (- lfo-mod))))
(def tank-r-damp (biquad tank-r-mod damping 0.5 1 0)) 
(write-history h-loop-r tank-r-damp)

; Final Output Mix
(def wet-l tank-l-damp)
(def wet-r tank-r-damp)

(out (+ (* in-l (- 1 mix)) (* wet-l mix)) 1 @name out-l)
(out (+ (* in-r (- 1 mix)) (* wet-r mix)) 2 @name out-r)
