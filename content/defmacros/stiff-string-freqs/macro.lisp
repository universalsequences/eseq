; Mode frequencies of a stiff string tuned to f0 (Modalys bi-string with
; set-pitch). n is the 1-based mode-number tensor, e.g. (+ (iota 80) 1).
; Solving tension for the pitch cancels it from the stiffness:
;   B = (pi^2 / 16) (E / rho) r^2 / (L^4 f0^2),  f_n = n f0 sqrt(1 + B n^2)
; divided by sqrt(1 + B) so mode 1 stays on f0 and only the uppers stretch.
(defmacro stiff-string-freqs (n f0 young density radius length)
  (def B (/ (* 0.61685 young radius radius)
            (* density length length length length f0 f0)))
  (* n f0 (/ (sqrt (+ 1 (* B n n))) (sqrt (+ 1 B)))))
