;; ui/cells.lisp — shader building blocks for cell-shaped widgets.
;;
;; Macros for a defwidget's :shader. A widget's coordinates span -1..1 on its
;; shorter side and further on its longer one, so anything measured from y or
;; x alone changes with the box's shape; these measure in `unit-x` / `unit-y`
;; (0..1 across the box whatever its shape) and size shapes from `width` /
;; `height`, so a widget looks the same at any size. No colors or styling of
;; their own: a view composes them into its look.

(module eseq.cells)

(export unit-x unit-y fill-rect inset shade span-x span-y bands neon)

;; 0 at the box's left edge, 1 at its right.
(defmacro unit-x () `(* 0.5 (+ (/ x width) 1)))

;; 0 at the box's top edge, 1 at its bottom.
(defmacro unit-y () `(* 0.5 (+ (/ y height) 1)))

;; The whole box.
(defmacro fill-rect () `(sdf/rounded-rect width height 0))

;; The box inset by d on every side.
(defmacro inset (d) `(sdf/rounded-rect (- width ,d) (- height ,d) 0))

;; Color c scaled from `top` at the box's top to `bottom` at its bottom.
(defmacro shade (c top bottom)
  `(* ,c (let ((k (mix ,top ,bottom (eseq.cells/unit-y)))) (rgba k k k 1))))

;; `shape` cut to the stretch between positions a and b (0..1) across the box,
;; left to right: a horizontal bar from a to b.
(defmacro span-x (shape a b)
  `(max ,shape
     (max (- x (+ (- width) (* 2 width (max ,a ,b))))
          (- (+ (- width) (* 2 width (min ,a ,b))) x))))

;; `shape` cut to the stretch between positions a and b (0..1) up the box,
;; bottom to top: a vertical bar from a to b.
(defmacro span-y (shape a b)
  `(max ,shape
     (max (- y (- height (* 2 height (min ,a ,b))))
          (- (- height (* 2 height (max ,a ,b))) y))))

;; Diagonal bands, bottom-left to top-right: 0..1, about n across the box.
(defmacro bands (n soft)
  `(smoothstep (- 1 ,soft) 1 (sin (* 3.14159 ,n (+ (eseq.cells/unit-x) (- 1 (eseq.cells/unit-y)))))))

;; Color (r g b) at full strength (its strongest channel at 1), lifted toward
;; white by `lift`: a dark color glows as vividly as a bright one.
(defmacro neon (r g b lift)
  `(let ((peak (max 0.0001 (max ,r (max ,g ,b)))))
     (rgba (mix (/ ,r peak) 1 ,lift) (mix (/ ,g peak) 1 ,lift) (mix (/ ,b peak) 1 ,lift) 1)))
