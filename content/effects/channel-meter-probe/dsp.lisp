; Diagnostic: no DSP, just expose the two input channels directly.
; Use this to confirm a track/effect slot is receiving stereo before testing STFT.

(def in-l (in 1 @name signal-l))
(def in-r (in 2 @name signal-r))

(param swap @min 0 @max 1 @default 0)

(out (mix in-l in-r swap) 1 @name left)
(out (mix in-r in-l swap) 2 @name right)
