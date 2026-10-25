;; eseq.modal.mesh-view — a vibrating object drawn in 3-D.
;;
;; mode-mesh-view draws a mesh whose vertices move by its modes: vertex p
;; moves along its normal by  scale * sum_k amps[k] * shape[p][k]  (shape is
;; the mesh's per-vertex table), measured so that one amplitude of 1 moves
;; the furthest point `scale` mesh radii. Colour follows the displacement
;; (blue troughs, warm crests); drag on it to orbit.
;;
;;   (import eseq.modal.mesh-view :as mv)
;;   (mv/mode-mesh-view :src "bell.modes.json" :amps (list 0 1 0.3))
;;   (mv/mode-mesh-view :geometry (mv/plate-geometry) :amps (list 0 0 1))
;;
;; The geometry is an "eseq-modes" file (what the finite-element solver
;; writes; a relative :src is beside the file whose view draws it), a handle
;; from (mesh-load path) / (mesh-geometry dict), or a dict built here for the
;; analytic objects: a string, a plate, a membrane. It is uploaded once per
;; distinct mesh; :amps is per-frame data (up to 256 modes).
;;
;; mode-mesh-face is the widget itself, a mesh defwidget: see
;; docs/defwidget-vertex-stage.md for :vertex, (out …), (dot amps (table-row
;; vertex-id)) and the other mesh forms.

(module eseq.modal.mesh-view)

(import eseq.view-kit :refer (without))

(export mode-mesh-view mode-mesh-face mode-mesh-cycle mode-mesh-cycle-face
        string-geometry plate-geometry membrane-geometry)

(def pi-num 3.14159265358979)

;; ── the widget ──────────────────────────────────────────────────────────────
;; State: scale (displacement of one unit amplitude, in mesh radii), the
;; camera (yaw, pitch in radians; zoom, 1 fits the mesh) and amps, the mode
;; amplitudes. orbit-yaw / orbit-pitch are the built-in drag orbit.
(defwidget mode-mesh-face
  :width 24 :height 14
  :state (scale yaw pitch zoom (amps :floats 256))
  :vertex
  (let ((reach (* scale mesh-radius))
        (u (/ (* reach (dot amps (table-row vertex-id))) (max table-peak 0.000001)))
        (p (+ position (* normal u)))
        (turn (+ yaw orbit-yaw))
        (tilt (+ pitch orbit-pitch)))
    (out :position (orbit-project p turn tilt zoom)
         :varyings ((disp (/ u (max reach 0.000001)))
                    (view (orbit-rotate p turn tilt)))))
  :shader
  ;; The face normal from the displaced surface itself, lit from the
  ;; upper left (both sides alike), over a diverging ramp.
  (let ((n (normalize (cross (dfdx view) (dfdy view))))
        (diffuse (abs (dot n (normalize (vec3 -0.4 0.6 0.7)))))
        (spec (pow (abs (dot n (normalize (vec3 -0.2 0.3 1.0)))) 24.0))
        (t (clamp disp -1.0 1.0))
        (rest (rgba 0.74 0.76 0.80 1.0))
        (base (if (< t 0.0)
                  (mix rest (rgba 0.18 0.42 0.95 1.0) (- t))
                  (mix rest (rgba 0.98 0.52 0.16 1.0) t)))
        (shade (+ 0.32 (* 0.68 diffuse))))
    (+ (* base (rgba shade shade shade 1.0)) (rgba (* 0.18 spec) (* 0.18 spec) (* 0.18 spec) 0.0))))

;; A mode-mesh-face with defaults: :amps (list 1) (the first mode), :scale
;; 0.12, :yaw 0.6, :pitch 0.6, :zoom 1. Takes :src or :geometry, and any
;; other widget props (:width, :height, :background, :key).
(def mode-mesh-view (&rest props &key (amps (list 1)) (scale 0.12) (yaw 0.6) (pitch 0.6) (zoom 1)
                     &allow-other-keys)
  (apply mode-mesh-face :amps amps :scale scale :yaw yaw :pitch pitch :zoom zoom
         (without props (list :amps :scale :yaw :pitch :zoom))))

;; A mesh that moves by itself: its six lowest modes superposed, each swinging
;; at its own slow, unrelated rate (weights falling with mode number), so the
;; view shows how the object can move while no voice is feeding it amplitudes.
;; Shading as mode-mesh-face.
(defwidget mode-mesh-cycle-face
  :width 24 :height 14
  :animates true
  :state (scale yaw pitch zoom)
  :vertex
  (let ((reach (* scale mesh-radius))
;; modes past the mesh's own (table-cols) weigh 0 (table clamps
        ;; to its last column)
        (w (+ (* (table vertex-id 0) (sin (* itime 1.9)))
              (* (if (< 1.0 table-cols) 0.8 0.0) (table vertex-id 1) (sin (* itime 2.7)))
              (* (if (< 2.0 table-cols) 0.65 0.0) (table vertex-id 2) (sin (* itime 3.4)))
              (* (if (< 3.0 table-cols) 0.5 0.0) (table vertex-id 3) (sin (* itime 4.3)))
              (* (if (< 4.0 table-cols) 0.4 0.0) (table vertex-id 4) (sin (* itime 5.1)))
              (* (if (< 5.0 table-cols) 0.3 0.0) (table vertex-id 5) (sin (* itime 6.2)))))
        (u (/ (* reach w) (max table-peak 0.000001)))
        (p (+ position (* normal u)))
        (turn (+ yaw orbit-yaw (* itime 0.15)))
        (tilt (+ pitch orbit-pitch)))
    (out :position (orbit-project p turn tilt zoom)
         :varyings ((disp (/ u (max reach 0.000001)))
                    (view (orbit-rotate p turn tilt)))))
  :shader
  (let ((n (normalize (cross (dfdx view) (dfdy view))))
        (diffuse (abs (dot n (normalize (vec3 -0.4 0.6 0.7)))))
        (spec (pow (abs (dot n (normalize (vec3 -0.2 0.3 1.0)))) 24.0))
        (t (clamp disp -1.0 1.0))
        (rest (rgba 0.74 0.76 0.80 1.0))
        (base (if (< t 0.0)
                  (mix rest (rgba 0.18 0.42 0.95 1.0) (- t))
                  (mix rest (rgba 0.98 0.52 0.16 1.0) t)))
        (shade (+ 0.32 (* 0.68 diffuse))))
    (+ (* base (rgba shade shade shade 1.0)) (rgba (* 0.18 spec) (* 0.18 spec) (* 0.18 spec) 0.0))))

;; A mode-mesh-cycle-face with defaults (:scale 0.1, :yaw 0.6, :pitch 0.5,
;; :zoom 1); takes :src or :geometry and other widget props.
(def mode-mesh-cycle (&rest props &key (scale 0.1) (yaw 0.6) (pitch 0.5) (zoom 1) &allow-other-keys)
  (apply mode-mesh-cycle-face :scale scale :yaw yaw :pitch pitch :zoom zoom
         (without props (list :scale :yaw :pitch :zoom))))

;; ── generated geometry ──────────────────────────────────────────────────────
;; Each returns a geometry dict with the eseq-modes keys (:points :normals
;; :triangles :shapes :freqs) for :geometry. Shapes are point-major: one
;; row of mode values per point. :freqs are relative (the first mode is 1).

;; Two triangles per cell of an (nx+1) x (ny+1) row-major grid of points.
(def grid-triangles (nx ny)
  (apply append
    (map (lambda (c)
           (let ((a (+ (* (floor (/ c nx)) (+ nx 1)) (mod c nx))))
             (list a (+ a 1) (+ a nx 2) a (+ a nx 2) (+ a nx 1))))
         (range (* nx ny)))))

;; The first `count` (m n) pairs of a rectangle lx x ly, by frequency
;; (m/lx)^2 + (n/ly)^2, as (list m n f).
(def rect-modes (lx ly count)
  (let ((all (apply append
               (map (lambda (m) (map (lambda (n)
                                       (list m n (+ (/ (* m m) (* lx lx)) (/ (* n n) (* ly ly)))))
                                     (range 1 7)))
                    (range 1 7)))))
    (take-sorted all count)))

;; The `count` items of `items` with the smallest third element, smallest first.
(def take-sorted (items count)
  (if (or (= count 0) (empty? items)) (list)
    (let ((best (reduce (lambda (b x) (if (< (nth x 2) (nth b 2)) x b)) (first items) items)))
      (cons best (take-sorted (filter (lambda (x) (not (= x best))) items) (- count 1))))))

;; A simply supported plate lx x ly metres on an nx x ny grid in the xy
;; plane (normal +z), with its first `modes` modes sin(m pi u) sin(n pi v).
(def plate-geometry (&key (nx 28) (ny 20) (lx 0.6) (ly 0.4) (modes 8))
  (let ((ms (rect-modes lx ly modes))
        (f0 (nth (first ms) 2))
        (pts (range (* (+ nx 1) (+ ny 1)))))
    (dict :points (apply append
                    (map (lambda (p) (list (* lx (- (/ (mod p (+ nx 1)) nx) 0.5))
                                           (* ly (- (/ (floor (/ p (+ nx 1))) ny) 0.5)) 0))
                         pts))
          :normals (apply append (map (lambda (p) (list 0 0 1)) pts))
          :triangles (grid-triangles nx ny)
          :shapes (apply append
                    (map (lambda (p)
                           (let ((u (/ (mod p (+ nx 1)) nx)) (v (/ (floor (/ p (+ nx 1))) ny)))
                             (map (lambda (m) (* (sin (* (first m) pi-num u)) (sin (* (nth m 1) pi-num v))))
                                  ms)))
                         pts))
          :freqs (map (lambda (m) (sqrt (/ (nth m 2) f0))) ms))))

;; A string `length` metres along x as a thin ribbon (normal +z), with its
;; first `modes` harmonics sin(k pi u).
(def string-geometry (&key (length 0.65) (segments 96) (modes 8))
  (let ((w (* length 0.012))
        (pts (range (* 2 (+ segments 1)))))
    (dict :points (apply append
                    (map (lambda (p) (list (* length (- (/ (mod p (+ segments 1)) segments) 0.5))
                                           (* w (- (floor (/ p (+ segments 1))) 0.5)) 0))
                         pts))
          :normals (apply append (map (lambda (p) (list 0 0 1)) pts))
          :triangles (grid-triangles segments 1)
          :shapes (apply append
                    (map (lambda (p)
                           (let ((u (/ (mod p (+ segments 1)) segments)))
                             (map (lambda (k) (sin (* k pi-num u))) (range 1 (+ modes 1)))))
                         pts))
          :freqs (range 1 (+ modes 1)))))

;; Bessel J_m(x) by its power series (enough terms for x below about 15).
(def bessel-sum (h2 m k term acc)
  (if (> k 40) acc
    (bessel-sum h2 m (+ k 1) (/ (* term -1 h2) (* (+ k 1) (+ k 1 m))) (+ acc term))))
(def factorial (n) (if (< n 2) 1 (* n (factorial (- n 1)))))
(def bessel-j (m x)
  (let ((h (/ x 2)))
    (bessel-sum (* h h) m 0 (/ (pow h m) (factorial m)) 0)))

;; Clamped circular membrane modes (m n) by frequency, with the Bessel zero
;; j_mn: shape J_m(j_mn r / R) cos(m theta).
(def membrane-modes
  (list (list 0 2.40483) (list 1 3.83171) (list 2 5.13562) (list 0 5.52008)
        (list 3 6.38016) (list 1 7.01559) (list 4 7.58834) (list 2 8.41724)
        (list 0 8.65373) (list 5 8.77148) (list 3 9.76102) (list 1 10.17347)))

;; A membrane of `radius` metres in the xy plane (normal +z): a centre point
;; and `rings` rings of `segments` points, with its first `modes` modes.
(def membrane-geometry (&key (radius 0.2) (rings 16) (segments 48) (modes 8))
  (let ((ms (if (< modes (len membrane-modes))
                (map (lambda (i) (nth membrane-modes i)) (range modes))
                membrane-modes))
        (polar (cons (list 0 0)
                     (apply append
                       (map (lambda (r) (map (lambda (s) (list (/ (+ r 1) rings) (/ (* 2 pi-num s) segments)))
                                             (range segments)))
                            (range rings)))))
        (ring-start (lambda (r) (+ 1 (* r segments)))))
    (dict :points (apply append
                    (map (lambda (q) (list (* radius (first q) (cos (nth q 1)))
                                           (* radius (first q) (sin (nth q 1))) 0))
                         polar))
          :normals (apply append (map (lambda (q) (list 0 0 1)) polar))
          :triangles (append
                       (apply append (map (lambda (s) (list 0 (+ 1 s) (+ 1 (mod (+ s 1) segments))))
                                          (range segments)))
                       (apply append
                         (map (lambda (c)
                                (let ((r (floor (/ c segments))) (s (mod c segments))
                                      (s2 (mod (+ (mod c segments) 1) segments)))
                                  (let ((a (+ (ring-start r) s)) (b (+ (ring-start r) s2))
                                        (c2 (+ (ring-start (+ r 1)) s)) (d (+ (ring-start (+ r 1)) s2)))
                                    (list a b d a d c2))))
                              (range (* (- rings 1) segments)))))
          :shapes (apply append
                    (map (lambda (q)
                           (map (lambda (m) (* (bessel-j (first m) (* (nth m 1) (first q)))
                                               (cos (* (first m) (nth q 1)))))
                                ms))
                         polar))
          :freqs (map (lambda (m) (/ (nth m 1) 2.40483)) ms))))
