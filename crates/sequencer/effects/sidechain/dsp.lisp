; DGenLisp effect - stereo sidechain compressor.

(def input-l (in 1 @name left))
(def input-r (in 2 @name right))
(def mod1 (in 3 @name mod1 @modulator 1))
(def mod2 (in 4 @name mod2 @modulator 2))
(def mod3 (in 5 @name mod3 @modulator 3))
(def mod4 (in 6 @name mod4 @modulator 4))
(def sidechain (in 7 @name sidechain))

(param threshold @min -80 @max -2 @default -20 @mod true @mod-mode additive)
(param ratio @min 1 @max 20 @default 10 @mod true @mod-mode additive)

(def out-l (compressor input-l (mod ratio) (mod threshold) 6 .01 .01 1 sidechain))
(def out-r (compressor input-r (mod ratio) (mod threshold) 6 .01 .01 1 sidechain))

(out out-l 1 @name left)
(out out-r 2 @name right)
