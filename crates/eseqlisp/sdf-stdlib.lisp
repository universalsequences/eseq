;; sdf-stdlib.lisp — SDF primitives and combinators
;; Loaded at editor startup. All shapes operate in normalized coords
;; with free variables x, y bound by the caller.
;;
;; This file is the module-system pilot (docs/module-system-spec.md §9):
;; the (module sdf) header qualifies each defmacro as sdf/<name>, so the
;; ~34 consumer files calling (sdf/circle …) etc. keep working verbatim —
;; the names they always used are now real qualified references instead
;; of lucky flat strings. %-prefixed macros are internal (spec §2).

(module sdf)

;; ── Primitive Shapes ──────────────────────────────────────────────────

;; Circle centered at origin with radius r.
;; Returns negative inside, zero on boundary, positive outside.
(defmacro circle (r)
  `(- (length (vec2 x y)) ,r))

;; Axis-aligned box centered at origin, half-extents w and h.
(defmacro rect (w h)
  `(let ((dx (- (abs x) ,w))
         (dy (- (abs y) ,h)))
     (+ (length (vec2 (max dx 0) (max dy 0)))
        (min (max dx dy) 0))))

;; Rounded rectangle: box with corner radius r.
(defmacro rounded-rect (w h r)
  `(let ((dx (- (abs x) (- ,w ,r)))
         (dy (- (abs y) (- ,h ,r))))
     (- (+ (length (vec2 (max dx 0) (max dy 0)))
           (min (max dx dy) 0))
        ,r)))

;; Aspect-aware rounded rect that fills the box minus an inset.
;; Uses the implicit `aspect` variable so the shape always covers
;; the full widget area regardless of rendered dimensions.
(defmacro fill-rounded-rect (inset r)
  `(sdf/rounded-rect (- (max aspect 1.0) ,inset)
                     (- (max (/ 1.0 aspect) 1.0) ,inset)
                     ,r))

;; Line segment from (ax,ay) to (bx,by). Returns distance from (x,y).
(defmacro line (ax ay bx by)
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
(defmacro translate (tx ty body)
  `(let ((x (- x ,tx))
         (y (- y ,ty)))
     ,body))

;; Scale: uniform scale by factor s.
(defmacro scale (s body)
  `(* (let ((x (/ x ,s))
            (y (/ y ,s)))
        ,body)
      ,s))

;; Rotate by angle (radians) counter-clockwise.
;; Saves old x/y before rebinding since let is sequential.
(defmacro rotate (angle body)
  `(let ((__rot_cos (cos ,angle))
         (__rot_sin (sin ,angle))
         (__rot_x x)
         (__rot_y y))
     (let ((x (+ (* __rot_cos __rot_x) (* __rot_sin __rot_y)))
           (y (- (* __rot_cos __rot_y) (* __rot_sin __rot_x))))
       ,body)))

;; ── Boolean Combinators ───────────────────────────────────────────────

;; Union: combine two shapes (closest surface).
(defmacro union (a b)
  `(min ,a ,b))

;; Subtract shape b from shape a.
(defmacro subtract (a b)
  `(max ,a (- 0 ,b)))

;; Intersect: overlap of two shapes.
(defmacro intersect (a b)
  `(max ,a ,b))

;; Smooth union with blending radius k.
(defmacro smooth-union (k d1 d2)
  `(let ((h (clamp (+ 0.5 (/ (* 0.5 (- ,d2 ,d1)) ,k)) 0 1)))
     (- (mix ,d2 ,d1 h) (* ,k h (- 1 h)))))

;; ── Lighting ────────────────────────────────────────────────────────────

;; Estimate a 3D surface normal from a 2D SDF expression.
;; Wraps the SDF in smoothstep(edge-min, edge-max, sdf) to create a
;; height field, then samples at 4 offsets via central differences.
;; The edge-min/edge-max parameters control the curvature profile —
;; tighter range = sharper bevel, wider = softer dome.
(defmacro normal (sdf-expr eps edge-min edge-max)
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
(defmacro diffuse (normal light-dir)
  `(max 0.0 (dot ,normal ,light-dir)))

;; Blinn-Phong specular highlight.
;; view-dir and light-dir should be normalized vec3s.
(defmacro specular (normal light-dir view-dir shininess)
  `(let ((__half (normalize (+ ,light-dir ,view-dir))))
     (pow (max 0.0 (dot ,normal __half)) ,shininess)))

;; ── Built-in Widget Fill Shapes ──────────────────────────────────────
;; Used internally by the :material feature on built-in slider widgets.
;; These produce the SDF for the value/fill portion of each slider type.

;; Horizontal slider fill bar: rounded rect from left edge to value_t.
;; In SDF coords the fill center-x = aspect*(value_t - 1), halfW = aspect*value_t.
;; Vertical inset 0.64 (= (0.5 - 0.18) * 2 from UV padding), corner radius 0.24.
(defmacro %hslider-fill ()
  `(sdf/translate (* aspect (- value_t 1.0)) 0.0
     (sdf/rounded-rect (* aspect value_t) 0.64 0.24)))

;; Vertical slider fill bar with material and bipolar (origin_t) support.
;; Compute the fill shape in UV-like local coordinates so `d` matches the
;; hardcoded slider geometry more closely. This keeps material edge logic
;; from collapsing into an outline on tall, narrow sliders.
(defmacro %vslider-fill-with-material (mat)
  `(let ((__fill_lo (min value_t origin_t))
         (__fill_hi (max value_t origin_t))
         (__fill_span (- __fill_hi __fill_lo))
         (__center_y (* 0.5 (- 1.0 (+ __fill_lo __fill_hi))))
         (__half_h (* 0.5 __fill_span))
         (__half_w (* 0.32 aspect))
         (__radius (min (* 0.12 aspect) __half_h)))
     (sdf/fill
       (let ((x (* 0.5 aspect x))
             (y (* 0.5 aspect y)))
         (sdf/translate 0.0 __center_y
           (sdf/rounded-rect __half_w
                             __half_h
                             __radius)))
       ,mat)))

;; All-in-one lit material color with curvature control.
;; edge-min/edge-max control the smoothstep on the SDF before normal estimation:
;;   tight range (e.g. -0.15, 0.02) = sharp bevel/rim
;;   wide range  (e.g. -0.7,  0.05) = soft dome
;; Returns an rgba suitable for use in (material :color ...).
(defmacro lit (base-color sdf-expr edge-min edge-max)
  `(let ((__lit_n    (sdf/normal ,sdf-expr 0.01 ,edge-min ,edge-max))
         (__lit_l    (normalize (vec3 -0.9 -0.9 1.3)))
         (__lit_v    (vec3 0.0 0.0 1.0))
         (__lit_diff (sdf/diffuse __lit_n __lit_l))
         (__lit_spec (sdf/specular __lit_n __lit_l __lit_v 48.0))
         (__lit_brightness (+ 0.6 (* 0.4 __lit_diff))))
     (+ (* ,base-color (rgba __lit_brightness __lit_brightness __lit_brightness 1.0))
        (rgba (* 0.5 __lit_spec) (* 0.5 __lit_spec) (* 0.5 __lit_spec) 0.0))))
