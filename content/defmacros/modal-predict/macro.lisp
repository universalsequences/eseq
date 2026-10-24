; Step 1 of the modal protocol: a modal object's free motion this sample,
; before any connection pushes on it. s1 / s2 are the object's state one and
; two samples ago (read from the patch's own histories: library macros take
; values, not history names, so the patch editor can represent them), a / b
; from modal-coefs. Works on tensor (modes) or scalar (lumped) objects.
(defmacro modal-predict (s1 s2 a b)
  (+ (* a s1) (* b s2)))
