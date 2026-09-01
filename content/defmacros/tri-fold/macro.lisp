; tri-fold — branchless triangle wavefolder, the core Wavetable synth's
; "fold" stage applied AFTER a table read. Drives the input by 1 + 6*drive
; and passes it through a periodic triangle transfer function built from
; wrap + abs:  out = 1 - |wrap(in*g + 1, 0, 4) - 2|.  Signal beyond +-1
; reflects back instead of clipping (West-coast wavefold), turning metallic
; as drive rises. drive 0 is transparent for inputs within +-1: gain 1 and
; the triangle's linear middle segment passes the signal untouched.
; Typical use:  (tri-fold (sample table phase wave) fold)
(defmacro tri-fold (input drive)
  (def g (+ 1 (* 6 (clip drive 0 1))))
  (def out (- 1 (abs (- (wrap (+ (* input g) 1) 0 4) 2))))
  out)
