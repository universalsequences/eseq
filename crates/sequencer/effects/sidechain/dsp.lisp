; DGenLisp effect — processes audio from the track's sampler
; Input on channel 1, output on channel 1

(def input (in 1 @name signal))
(def input2 (in 2 @name signal))
(def side (in 3 @name signal @modulator 1))
(param threshold @min -80 @max -2 @default -20)
(param ratio min 1 @max 20 @default 10)
(def o (compressor input ratio threshold 6 .01 .01 1 side))
(def o2 (compressor input2 ratio threshold 6 .01 .01 1 side))
(out o 1 @name audio)
(out o2 2 @name audio)
