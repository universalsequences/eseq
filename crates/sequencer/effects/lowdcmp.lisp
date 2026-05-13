; DGenLisp effect — processes audio from the track's sampler
; Input on channel 1, output on channel 1

(def input1 (in 1 @name signal))
(param threshold @min -40 @max 0 @default -10)
(param gain @min 0 @max 10 @default 1)
(param ratio @min 1 @max 32 @default 10)
(param knee @min 1 @max 10 @default 5)
(out (compressor 
    (* gain input1) 
    ratio 
    threshold 
    knee 
    0.1 
    0.1) 
  1 @name audio)
(def input2 (in 2 @name signal))
(out (compressor (* gain input2) ratio threshold knee 0.1 0.1) 2 @name audio)
