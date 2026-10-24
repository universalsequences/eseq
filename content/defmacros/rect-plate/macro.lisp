; Simply supported rectangular plate (Modalys rect-plate), Kirchhoff bending:
;   f_mn = (pi / 2) h sqrt(E / (12 rho (1 - nu^2))) (m^2 / lx^2 + n^2 / ly^2)
; with nu = 0.3. pm / pn are 1-based mode-index tensors of equal shape (e.g.
; an 8 x 8 grid flattened). thick, lx, ly in metres. Returns freqs (Hz).
; Shapes are sin(m pi x) sin(n pi y) at a point (x, y) in 0..1: see
; rect-plate-shape. Unit modal mass needs the factor 2 (mean square of a
; product of two sines is 1/4), applied there.
(defmacro rect-plate (pm pn young density thick lx ly)
  (def c (* 1.5707963 thick (sqrt (/ young (* 10.92 density)))))
  (* c (+ (/ (* pm pm) (* lx lx)) (/ (* pn pn) (* ly ly)))))
