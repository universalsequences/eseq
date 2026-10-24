; Modalys 'felt / 'strike connection: a one-sided spring that only pushes
; while A is pressed into B (gap = yA - yB < 0, e.g. a mallet above a head).
; Same sign convention and relative stiffness as spring-connection: push B by
; +F and A by -F. Implicit, so a hard felt stays stable.
(defmacro felt-contact (gap C stiffness)
  (/ (* (min gap 0) stiffness) (* (+ 1 stiffness) (max C 1e-12))))
