; Step 2 of the modal protocol: read an access point of a modal object.
; phi is the point's mode-shape vector, G the force -> mode gain. Returns
;   y = sum(free phi)          where the point would be with no force
;   C = G sum(phi^2)           how far it moves per unit force this sample
; Connections solve their force from both ends' y and C (implicitly, so any
; stiffness stays stable), then modal-advance applies F phi to each end.
(defmacro modal-point (free phi G)
  (tuple (sum (* free phi)) (* G (sum (* phi phi)))))
