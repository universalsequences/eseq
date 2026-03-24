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

;; Aspect-aware rounded rect that fills the box minus an inset.
;; Uses the implicit `aspect` variable so the shape always covers
;; the full widget area regardless of rendered dimensions.
(defmacro sdf/fill-rounded-rect (inset r)
  `(sdf/rounded-rect (- (max aspect 1.0) ,inset)
                     (- (max (/ 1.0 aspect) 1.0) ,inset)
                     ,r))

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

;; ── Lighting ────────────────────────────────────────────────────────────

;; Estimate a 3D surface normal from a 2D SDF expression.
;; Wraps the SDF in smoothstep(edge-min, edge-max, sdf) to create a
;; height field, then samples at 4 offsets via central differences.
;; The edge-min/edge-max parameters control the curvature profile —
;; tighter range = sharper bevel, wider = softer dome.
(defmacro sdf/normal (sdf-expr eps edge-min edge-max)
  `(let ((__nr (smoothstep ,edge-min ,edge-max
                (let ((x (+ x ,eps))) ,sdf-expr)))
         (__nl (smoothstep ,edge-min ,edge-max
                (let ((x (- x ,eps))) ,sdf-expr)))
         (__nu (smoothstep ,edge-min ,edge-max
                (let ((y (+ y ,eps))) ,sdf-expr)))
         (__nd (smoothstep ,edge-min ,edge-max
                (let ((y (- y ,eps))) ,sdf-expr))))
     (normalize (vec3 (/ (- __nr __nl) (* 2.0 ,eps))
                       (/ (- __nu __nd) (* 2.0 ,eps))
                       1.0))))

;; Lambertian diffuse: max(0, dot(normal, light_dir)).
;; light-dir should be a normalized vec3.
(defmacro sdf/diffuse (normal light-dir)
  `(max 0.0 (dot ,normal ,light-dir)))

;; Blinn-Phong specular highlight.
;; view-dir and light-dir should be normalized vec3s.
(defmacro sdf/specular (normal light-dir view-dir shininess)
  `(let ((__half (normalize (+ ,light-dir ,view-dir))))
     (pow (max 0.0 (dot ,normal __half)) ,shininess)))

;; All-in-one lit material color with curvature control.
;; edge-min/edge-max control the smoothstep on the SDF before normal estimation:
;;   tight range (e.g. -0.15, 0.02) = sharp bevel/rim
;;   wide range  (e.g. -0.7,  0.05) = soft dome
;; Returns an rgba suitable for use in (material :color ...).
(defmacro sdf/lit (base-color sdf-expr edge-min edge-max)
  `(let ((__lit_n    (sdf/normal ,sdf-expr 0.01 ,edge-min ,edge-max))
         (__lit_l    (normalize (vec3 -0.9 -0.9 1.3)))
         (__lit_v    (vec3 0.0 0.0 1.0))
         (__lit_diff (sdf/diffuse __lit_n __lit_l))
         (__lit_spec (sdf/specular __lit_n __lit_l __lit_v 48.0))
         (__lit_brightness (+ 0.6 (* 0.4 __lit_diff))))
     (+ (* ,base-color (rgba __lit_brightness __lit_brightness __lit_brightness 1.0))
        (rgba (* 0.5 __lit_spec) (* 0.5 __lit_spec) (* 0.5 __lit_spec) 0.0))))
