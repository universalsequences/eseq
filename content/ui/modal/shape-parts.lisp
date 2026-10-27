;; eseq.modal.shape-parts — a finite-element solid's shape as parts: the
;; pure geometry behind the shape lab's parts editor
;; (eseq.modal.shape-lab).
;;
;; A shape (what m/solid's :shape takes) reads as a list of parts, each
;; (dict :shape primitive :cut bool): a :union's items are added parts, a
;; :difference's first item is the body (itself a union or one part) and the
;; rest are cut out of it. shape-of writes parts back as that same nesting,
;; so a lone part stays a plain primitive and a script's shape keeps the
;; form it was written in.
;;
;; Each part is drawn in a view plane (axes a, b, looking along the third)
;; as its footprint: convex polygons of (u v) points. Patterns (a hole, a
;; slot, a block, an arch, a row) are plain parts, so the script spells out
;; what they made.
(module eseq.modal.shape-parts)

(export kinds kind-of body snap parts-of shape-of reorder center-of with-center axis-of axis-names
        bounds-of union-bounds footprint hit? hole slot block arch row min-feature
        region-center region-shape move-region wall-of-profile axes-by-extent)

(def kinds (list :revolve :box :cylinder :tube :sphere :shell :union :difference :obj))

;; The shape's kind: the first of `kinds` it has a key for.
(def kind-of (s)
  (if (map? s)
    (let ((found (filter (lambda (k) (not (= (get s k) nil))) kinds)))
      (if (empty? found) nil (first found)))
    nil))

;; The shape's own dict (its :box, :cylinder, … value).
(def body (s) (get s (kind-of s)))

;; Metres to a tenth of a millimetre, so what the lab solves is exactly what
;; the script will read back (the same solve cache entry).
(def snap (v) (/ (round (* v 10000)) 10000))

(def snap-all (xs) (map snap xs))

;; ── Parts ──

(def added (s) (dict :shape s :cut false))

(def parts-of (s)
  (let ((adds (lambda (b) (if (= (kind-of b) :union) (map added (get b :union)) (list (added b))))))
    (if (= (kind-of s) :difference)
      (let ((items (get s :difference)))
        (append (adds (first items)) (map (lambda (c) (dict :shape c :cut true)) (rest items))))
      (adds s))))

(def shape-of (parts)
  (let ((adds (map (lambda (p) (get p :shape)) (filter (lambda (p) (not (get p :cut))) parts)))
        (cuts (map (lambda (p) (get p :shape)) (filter (lambda (p) (get p :cut)) parts)))
        (base (if (= (len adds) 1) (first adds) (dict :union adds))))
    (if (empty? cuts) base (dict :difference (cons base cuts)))))

;; The order shape-of keeps (added parts, then cuts, each in turn): part i's
;; place in it.
(def reorder (parts i)
  (let ((cut? (lambda (j) (get (nth parts j) :cut)))
        (order (append (filter (lambda (j) (not (cut? j))) (range (len parts)))
                       (filter cut? (range (len parts))))))
    (first (filter (lambda (k) (= (nth order k) i)) (range (len order))))))

;; ── Placement ──

(def axis-names (list :x :y :z))

;; A cylinder, tube or revolve's axis as 0, 1 or 2 (z when unset).
(def axis-of (b)
  (let ((a (get b :axis)))
    (if (or (= a :x) (= a "x")) 0 (if (or (= a :y) (= a "y")) 1 2))))

(def center-of (s) (or (get (body s) :center) (list 0 0 0)))

(def with-center (s c)
  (dict (kind-of s) (merge (body s) :center (snap-all c))))

;; A revolve profile's extent: (rmax zmin zmax), walls included.
(def profile-extent (b)
  (let ((half (lambda (p) (* 0.5 (or (if (> (len p) 2) (nth p 2) nil) (get b :thickness) 0))))
        (pts (get b :profile)))
    (list (reduce (lambda (m p) (max m (+ (first p) (half p)))) 0 pts)
          (reduce (lambda (m p) (min m (- (nth p 1) (half p)))) 1000 pts)
          (reduce (lambda (m p) (max m (+ (nth p 1) (half p)))) -1000 pts))))

;; Half-extents of something round about `axis`: r across it, h along it.
(def axial-half (axis r h) (map (lambda (i) (if (= i axis) h r)) (range 3)))

(def around (c half)
  (list (map (lambda (i) (- (nth c i) (nth half i))) (range 3))
        (map (lambda (i) (+ (nth c i) (nth half i))) (range 3))))

;; (lo hi) corners of a part's bounding box, or nil for one the editor
;; cannot place (an .obj, or a nested union or difference).
(def bounds-of (s)
  (let ((b (body s))
        (c (center-of s)))
    (match (kind-of s)
      :box (around c (map (lambda (x) (* 0.5 x)) (get b :size)))
      :cylinder (around c (axial-half (axis-of b) (get b :radius) (* 0.5 (get b :height))))
      :tube (around c (axial-half (axis-of b) (get b :outer-radius) (* 0.5 (get b :height))))
      :sphere (around c (list (get b :radius) (get b :radius) (get b :radius)))
      :shell (around c (list (get b :outer-radius) (get b :outer-radius) (get b :outer-radius)))
      :revolve (let ((e (profile-extent b))
                     (axis (axis-of b))
                     (box (around c (axial-half axis (first e) 0))))
                 (list (map (lambda (i) (if (= i axis) (+ (nth c i) (nth e 1)) (nth (first box) i))) (range 3))
                       (map (lambda (i) (if (= i axis) (+ (nth c i) (nth e 2)) (nth (nth box 1) i))) (range 3))))
      _ nil)))

;; The bounds of every placeable part in `parts`, or nil.
(def union-bounds (parts)
  (reduce (lambda (acc p)
            (let ((b (bounds-of (get p :shape))))
              (if (= b nil) acc
                (if (= acc nil) b
                  (list (map (lambda (i) (min (nth (first acc) i) (nth (first b) i))) (range 3))
                        (map (lambda (i) (max (nth (nth acc 1) i) (nth (nth b 1) i))) (range 3)))))))
          nil parts))

;; ── Footprints in a view plane (axes a, b) ──

;; Sides of a circle of radius r: 32 for a hole, more for a big one (an
;; arch's circle is metres across), so its edge stays round where it
;; crosses the body.
(def circle-steps (r) (clamp (round (* 32 (/ r 0.02))) 32 720))

(def ring-point (cu cv r k steps)
  (let ((angle (/ (* 6.283185307179586 k) steps)))
    (list (+ cu (* r (cos angle))) (+ cv (* r (sin angle))))))

(def disc (cu cv r)
  (let ((steps (circle-steps r)))
    (list (map (lambda (k) (ring-point cu cv r k steps)) (range steps)))))

(def annulus (cu cv outer inner)
  (let ((steps (circle-steps outer)))
    (map (lambda (k) (list (ring-point cu cv inner k steps) (ring-point cu cv outer k steps)
                           (ring-point cu cv outer (+ k 1) steps) (ring-point cu cv inner (+ k 1) steps)))
         (range steps))))

(def rect-of (lo hi a b)
  (list (list (list (nth lo a) (nth lo b)) (list (nth hi a) (nth lo b))
              (list (nth hi a) (nth hi b)) (list (nth lo a) (nth hi b)))))

;; A revolve seen side on (its axis in the view): its cross-section on
;; both sides of the axis, one convex quad per profile segment: the wall
;; strip (walls) or the trapezoid down to the axis (a filled profile). u is
;; the view's across axis.
(def revolve-section (bd c u axis)
  (let ((pts (get bd :profile))
        (wall (lambda (p) (or (if (> (len p) 2) (nth p 2) nil) (get bd :thickness))))
        (walled (not (= (wall (first pts)) nil)))
        (at (lambda (side r z) (list (+ (nth c u) (* side r)) (+ (nth c axis) z)))))
    (apply append
      (map (lambda (i)
             (let ((p (nth pts i))
                   (q (nth pts (+ i 1)))
                   (dr (- (first q) (first p)))
                   (dz (- (nth q 1) (nth p 1)))
                   (l (max 0.000001 (sqrt (+ (* dr dr) (* dz dz)))))
                   (nr (/ (* -1 dz) l))
                   (nz (/ dr l))
                   (hp (* 0.5 (or (wall p) 0)))
                   (hq (* 0.5 (or (wall q) 0))))
               (map (lambda (side)
                      (if walled
                        (list (at side (+ (first p) (* hp nr)) (+ (nth p 1) (* hp nz)))
                              (at side (+ (first q) (* hq nr)) (+ (nth q 1) (* hq nz)))
                              (at side (- (first q) (* hq nr)) (- (nth q 1) (* hq nz)))
                              (at side (- (first p) (* hp nr)) (- (nth p 1) (* hp nz))))
                        (list (at side 0 (nth p 1)) (at side (first p) (nth p 1))
                              (at side (first q) (nth q 1)) (at side 0 (nth q 1)))))
                    (list 1 -1))))
           (range (- (len pts) 1))))))

;; A part's footprint as convex polygons; nil when it has none.
(def footprint (s a b)
  (let ((bounds (bounds-of s)))
    (if (= bounds nil) nil
      (let ((bd (body s))
            (c (center-of s))
            (cu (nth c a))
            (cv (nth c b))
            (kind (kind-of s))
            ;; round parts seen down their axis are discs (or rings)
            (end-on (or (= kind :sphere) (= kind :shell)
                        (and (not (= kind :box)) (not (= (axis-of bd) a)) (not (= (axis-of bd) b))))))
        (if (not end-on)
          (if (= kind :revolve)
            (let ((axis (axis-of bd)))
              (if (= axis b) (revolve-section bd c a axis)
                (map (lambda (poly) (map (lambda (pt) (list (nth pt 1) (first pt))) poly)) (revolve-section bd c b axis))))
            (rect-of (first bounds) (nth bounds 1) a b))
          (match kind
            :tube (annulus cu cv (get bd :outer-radius) (get bd :inner-radius))
            :shell (annulus cu cv (get bd :outer-radius) (get bd :inner-radius))
            :cylinder (disc cu cv (get bd :radius))
            :sphere (disc cu cv (get bd :radius))
            _ (disc cu cv (first (profile-extent bd)))))))))

(def cross-z (p q r)
  (- (* (- (first q) (first p)) (- (nth r 1) (nth p 1)))
     (* (- (nth q 1) (nth p 1)) (- (first r) (first p)))))

(def inside-convex? (poly u v)
  (let ((n (len poly))
        (signs (map (lambda (i) (cross-z (nth poly i) (nth poly (mod (+ i 1) n)) (list u v))) (range n))))
    (or (empty? (filter (lambda (s) (< s 0)) signs))
        (empty? (filter (lambda (s) (> s 0)) signs)))))

(def hit? (polys u v)
  (and (not (= polys nil)) (not (empty? (filter (lambda (p) (inside-convex? p u v)) polys)))))

;; ── Patterns ──
;; Each takes the body's bounds (lo hi) and returns one part.

(def extent (bounds i) (- (nth (nth bounds 1) i) (nth (first bounds) i)))
(def mid (bounds i) (* 0.5 (+ (nth (nth bounds 1) i) (nth (first bounds) i))))

;; The axes in order of the body's extent: (longest middle shortest).
(def axes-by-extent (bounds)
  (let ((e (map (lambda (i) (extent bounds i)) (range 3)))
        (longest (if (and (>= (first e) (nth e 1)) (>= (first e) (nth e 2))) 0 (if (>= (nth e 1) (nth e 2)) 1 2)))
        (others (filter (lambda (i) (not (= i longest))) (range 3)))
        (middle (if (>= (nth e (first others)) (nth e (nth others 1))) (first others) (nth others 1)))
        (shortest (if (= middle (first others)) (nth others 1) (first others))))
    (list longest middle shortest)))

;; A point at (u v) in plane (a b), midway through the body along `n`.
(def point-at (bounds a b n u v)
  (map (lambda (i) (if (= i a) u (if (= i b) v (mid bounds n)))) (range 3)))

;; A round hole through the body along n (the view direction), at (u v):
;; a fifth of the body's thinnest side across, at most 4% of its longest.
(def hole (bounds a b n u v)
  (let ((r (min (* 0.2 (min (min (extent bounds a) (extent bounds b)) (extent bounds n)))
                (* 0.04 (max (max (extent bounds a) (extent bounds b)) (extent bounds n))))))
    (dict :shape (dict :cylinder (dict :radius (snap (max 0.001 r)) :height (snap (* 1.6 (extent bounds n)))
                                       :axis (nth axis-names n) :center (snap-all (point-at bounds a b n u v))))
          :cut true)))

;; A slot through the body along n, long along a.
(def slot (bounds a b n u v)
  (dict :shape (dict :box (dict :size (snap-all (map (lambda (i) (if (= i a) (* 0.3 (extent bounds a))
                                                                   (if (= i b) (* 0.15 (extent bounds b))
                                                                     (* 1.6 (extent bounds n)))))
                                                     (range 3)))
                                :center (snap-all (point-at bounds a b n u v))))
        :cut true))

;; A block a quarter of the body across, as deep as the body along n. (A
;; block that does not touch the body is dropped by the mesher: only the
;; largest connected piece is solved.)
(def block (bounds a b n u v)
  (dict :shape (dict :box (dict :size (snap-all (map (lambda (i) (if (= i n) (extent bounds n) (* 0.25 (extent bounds i))))
                                                     (range 3)))
                                :center (snap-all (point-at bounds a b n u v))))
        :cut false))

;; A marimba bar's undercut: a shallow arch out of the body's underside (its
;; thinnest axis, low side), across its width, spanning 60% of its length,
;; half its thickness deep: a cylinder of radius (c²/4 + d²) / 2d.
(def arch (bounds)
  (let ((axes (axes-by-extent bounds))
        (l (first axes))
        (w (nth axes 1))
        (th (nth axes 2))
        (c (* 0.6 (extent bounds l)))
        (d (* 0.5 (extent bounds th)))
        (r (/ (+ (* 0.25 c c) (* d d)) (* 2 d))))
    (dict :shape (dict :cylinder (dict :radius (snap r) :height (snap (* 1.6 (extent bounds w))) :axis (nth axis-names w)
                                       :center (snap-all (map (lambda (i) (if (= i th) (+ (nth (first bounds) th) d (* -1 r)) (mid bounds i)))
                                                              (range 3)))))
          :cut true)))

;; n copies of part p spread evenly along axis a over the middle 80% of
;; the body.
(def row (bounds p a n)
  (map (lambda (k)
         (let ((c (center-of (get p :shape)))
               (u (+ (nth (first bounds) a) (* (extent bounds a) (+ 0.1 (* 0.8 (/ (+ k 0.5) n)))))))
           (merge p :shape (with-center (get p :shape) (set-nth c a u)))))
       (range n)))


;; The smallest size of any cut (a hole's diameter, a slot's narrowest
;; side), or nil without cuts: features under about two mesh cells blur.
(def min-feature (parts)
  (reduce (lambda (m p)
            (let ((s (get p :shape))
                  (b (body s))
                  (size (match (kind-of s)
                          :cylinder (* 2 (get b :radius))
                          :sphere (* 2 (get b :radius))
                          :tube (- (get b :outer-radius) (get b :inner-radius))
                          :shell (- (get b :outer-radius) (get b :inner-radius))
                          :box (min (min (nth (get b :size) 0) (nth (get b :size) 1)) (nth (get b :size) 2))
                          _ nil)))
              (if (or (not (get p :cut)) (= size nil)) m (if (= m nil) size (min m size)))))
          nil parts))

;; ── Clamps ──
;; A solid's :block (the region the solver holds still) is a
;; (dict :sphere (dict :center :radius)) or (dict :box (dict :min :max)).

(def region-center (r)
  (if (= (kind-of r) :sphere)
    (or (get (body r) :center) (list 0 0 0))
    (map (lambda (i) (* 0.5 (+ (nth (get (body r) :min) i) (nth (get (body r) :max) i)))) (range 3))))

;; The region as a shape, for its footprint.
(def region-shape (r)
  (if (= (kind-of r) :sphere)
    r
    (dict :box (dict :size (map (lambda (i) (- (nth (get (body r) :max) i) (nth (get (body r) :min) i))) (range 3))
                     :center (region-center r)))))

;; The region moved so its centre is c.
(def move-region (r c)
  (if (= (kind-of r) :sphere)
    (dict :sphere (merge (body r) :center (snap-all c)))
    (let ((d (map (lambda (i) (- (nth c i) (nth (region-center r) i))) (range 3))))
      (dict :box (dict :min (snap-all (map (lambda (i) (+ (nth (get (body r) :min) i) (nth d i))) (range 3)))
                       :max (snap-all (map (lambda (i) (+ (nth (get (body r) :max) i) (nth d i))) (range 3))))))))

;; A revolve's thinnest wall (its points' own, else its :thickness), or nil
;; for a filled profile.
(def wall-of-profile (b)
  (reduce (lambda (m p)
            (let ((w (or (if (> (len p) 2) (nth p 2) nil) (get b :thickness))))
              (if (= w nil) m (if (= m nil) w (min m w)))))
          nil (get b :profile)))
