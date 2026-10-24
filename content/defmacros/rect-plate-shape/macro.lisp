; Mode-shape vector of a simply supported rectangular plate at (x, y), both
; 0..1 across the plate, scaled to unit modal mass (mean square 1/2, like a
; string's sin(n pi x)). pm / pn as in rect-plate.
(defmacro rect-plate-shape (pm pn x y)
  (* 1.4142136 (sin (* pm 3.14159265 x)) (sin (* pn 3.14159265 y))))
