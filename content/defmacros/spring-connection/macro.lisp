; Modalys 'spring / 'adhere connection between two access points, solved
; inside the sample. gap = yA - yB from modal-point, C = CA + CB. Returns F:
; push B by +F and A by -F (B is pulled toward A). stiffness is relative to
; the two points' own give: 0 is no connection, 1 meets them halfway, large
; values approach 'adhere (rigidly glued):
;   F = gap s / ((1 + s) C)
; Implicit, so any stiffness is stable.
(defmacro spring-connection (gap C stiffness)
  (/ (* gap stiffness) (* (+ 1 stiffness) (max C 1e-12))))
