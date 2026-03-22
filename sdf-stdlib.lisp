;; sdf-stdlib.lisp — SDF primitives and combinators
;; Loaded at editor startup. All shapes operate in normalized coords
;; with free variables x, y bound by the caller.

;; ── Primitive Shapes ──────────────────────────────────────────────────

;; Circle centered at origin with radius r.
;; Returns negative inside, zero on boundary, positive outside.
(defmacro sdf/circle (r)
  `(- (length (vec2 x y)) ,r))

;; Axis-aligned box centered at origin, half-extents w and h.
(defmacro sdf/rect (w h)
  `(let ((dx (- (abs x) ,w))
         (dy (- (abs y) ,h)))
     (+ (length (vec2 (max dx 0) (max dy 0)))
        (min (max dx dy) 0))))

;; Rounded rectangle: box with corner radius r.
(defmacro sdf/rounded-rect (w h r)
  `(let ((dx (- (abs x) (- ,w ,r)))
         (dy (- (abs y) (- ,h ,r))))
     (- (+ (length (vec2 (max dx 0) (max dy 0)))
           (min (max dx dy) 0))
        ,r)))

;; Line segment from (ax,ay) to (bx,by). Returns distance from (x,y).
(defmacro sdf/line (ax ay bx by)
  `(let ((pax (- x ,ax))
         (pay (- y ,ay))
         (bax (- ,bx ,ax))
         (bay (- ,by ,ay)))
     (let ((h (clamp (/ (+ (* pax bax) (* pay bay))
                        (+ (* bax bax) (* bay bay)))
                     0 1)))
       (length (vec2 (- pax (* h bax))
                     (- pay (* h bay)))))))

;; ── Transform Combinators ─────────────────────────────────────────────

;; Translate: shift the coordinate origin by (tx, ty).
(defmacro sdf/translate (tx ty body)
  `(let ((x (- x ,tx))
         (y (- y ,ty)))
     ,body))

;; Scale: uniform scale by factor s.
(defmacro sdf/scale (s body)
  `(* (let ((x (/ x ,s))
            (y (/ y ,s)))
        ,body)
      ,s))

;; Rotate by angle (radians) counter-clockwise.
;; Saves old x/y before rebinding since let is sequential.
(defmacro sdf/rotate (angle body)
  `(let ((__rot_cos (cos ,angle))
         (__rot_sin (sin ,angle))
         (__rot_x x)
         (__rot_y y))
     (let ((x (+ (* __rot_cos __rot_x) (* __rot_sin __rot_y)))
           (y (- (* __rot_cos __rot_y) (* __rot_sin __rot_x))))
       ,body)))

;; ── Boolean Combinators ───────────────────────────────────────────────

;; Union: combine two shapes (closest surface).
(defmacro sdf/union (a b)
  `(min ,a ,b))

;; Subtract shape b from shape a.
(defmacro sdf/subtract (a b)
  `(max ,a (- 0 ,b)))

;; Intersect: overlap of two shapes.
(defmacro sdf/intersect (a b)
  `(max ,a ,b))

;; Smooth union with blending radius k.
(defmacro sdf/smooth-union (k d1 d2)
  `(let ((h (clamp (+ 0.5 (/ (* 0.5 (- ,d2 ,d1)) ,k)) 0 1)))
     (- (mix ,d2 ,d1 h) (* ,k h (- 1 h)))))
