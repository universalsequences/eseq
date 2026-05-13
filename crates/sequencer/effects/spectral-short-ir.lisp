; Inline IR partitioned convolution for metallic early reflections.

(def in-l (in 1 @name signal-l))
(def in-r (in 2 @name signal-r))
(def input (* 0.5 (+ in-l in-r)))

(param wet @min 0 @max 1 @default 0.42)
(param gain @min 0 @max 2 @default 1.0)
(param tone @min 600 @max 14000 @default 6500)

(def early-ir (tensor @shape [16] @data [1.0 0.0 -0.35 0.0 0.22 0.0 -0.15 0.0 0.1 0.0 -0.07 0.0 0.04 0.0 -0.02 0.0]))
(def convolved (partitioned-convolve input early-ir @N 1024 @hop 256 @gain gain))
(def bright (biquad convolved tone 0.707 1 0))

(out (mix in-l bright wet) 1 @name left)
(out (mix in-r bright wet) 2 @name right)
