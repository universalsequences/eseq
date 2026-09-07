(use-defmacro heat-formant-bank)
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def source (* gate (polyblep_saw (phasor pitch) pitch)))
(param steep @default 0 @min 0 @max 1)
(out (* 0.01 (heat-formant-bank source 271.47756 2302.53549 3026.47963
  1.12697649 -0.11269431 0.08943757 steep)) 1)
