;; eseq.modal.shape-lab — the *shape-lab* buffer: design the finite-element
;; solids (m/solid) of a modal script live.
;;
;; M-x Shape Lab in a script buffer evaluates the script and opens the lab
;; beside it on the script's first solid: a layers tree (its parts, added or
;; cut, and its clamp) with the selected layer's properties, a canvas to
;; place and drag parts on (or a revolve's profile points), and a 3-D view.
;; The 3-D view follows every edit (the shape meshed but not solved, in
;; milliseconds); each finished edit is solved in the background (seconds),
;; after which it swings through the lowest modes. Update draft writes the
;; solid into the script's (m/solid 'id …) call as one undoable edit,
;; evaluates the script as C-c C-c does and recompiles its draft, showing
;; its progress (draft-status).
;;
;; The host side is src/ui/host_commands/shape_lab.rs; modal-mesh (the
;; preview) is in src/lisp_host/eseq/modal_patch.rs; the parts geometry is
;; eseq.modal.shape-parts.
;;
;; Render root: registers the *shape-lab* buffer at top level. Import it
;; only from main.lisp, never from a library module.
(module eseq.modal.shape-lab)

(import eseq.modal.mesh-view :as mv)
(import eseq.modal.materials :as mat)
(import eseq.modal.shape-parts :as sp :refer (kind-of snap))

(export shape-lab loaded solved solve-progress draft-status)

;; ── State ──

;; The script's buffer, and its folder (where a spec's relative .obj paths
;; resolve).
(defstate lab-buffer "")
(defstate lab-dir "")
;; What the script's solids were loaded as: (dict :id :spec :mesh-file
;; :freqs :mass :editable) each (shape_lab.rs `load`).
(defstate lab-solids (list))
(defstate lab-index 0)
;; The solid being edited, as the modal-solve spec m/solid builds:
;; (dict :solid shape :modes n :made-of material …).
(defstate lab-spec nil)
;; (modal-mesh lab-spec): a mesh handle with :nodes :elements :mass, or
;; (dict :error message).
(defstate lab-preview nil)
;; The solve the lab last asked for, and its result once it arrives: a modal
;; data dict with :file :freqs :mass, or (dict :error message). Nil while
;; solving.
(defstate lab-ticket 0)
(defstate lab-solved nil)
;; The running solve's (phase fraction), from the host; nil when none.
(defstate lab-solve-progress nil)
;; The selected profile point, and whether a canvas drag is under way (a
;; point's, a part's or the clamp's).
(defstate lab-point 0)
(defstate lab-dragging false)
;; The profile canvas: its framing (centre and half-extents, metres) and
;; geometry handle.
(defstate lab-view (dict :cr 0 :cz 0 :hr 0.2 :hz 0.2))
(defstate lab-canvas nil)
;; Edited since loaded or written to the script.
(defstate lab-edited false)
;; A revolve's editor: :profile (its points) or :parts (cut it, add to it).
(defstate lab-editor :profile)
;; The parts editor: the selected part, the view plane (:top :front :side),
;; the pointer tool (:select moves parts and points; :hole, :slot, :block
;; or :arch places one where you click, :point a named point, then hands
;; back to :select), where a dragged part was grabbed (its centre minus the
;; pointer) and whether it moved, and the radius of the hole last sized
;; (new holes take it).
(defstate lab-part 0)
(defstate lab-plane :top)
(defstate lab-tool :select)
(defstate lab-grab (list 0 0))
(defstate lab-moved false)
(defstate lab-hole-radius nil)
;; Whether a canvas drag is moving the clamp (the spec's :block) instead of
;; a point or part.
(defstate lab-drag-clamp false)
;; The solid's named points, (dict :name (x y z)) (m/solid's :points: what
;; the script's connections and listens name), the selected one's name
;; (while lab-part is point-layer), whether a canvas drag is moving it, and
;; the name being typed for it.
(defstate lab-points (dict))
(defstate lab-named nil)
(defstate lab-drag-point false)
(defstate lab-point-draft "")

;; The instrument view: the patch's strings, connections and listens
;; (from the host), each solid's place as edited here ((id place) pairs,
;; the script's until moved), which view the 3-D panel shows (:solid or
;; :instrument), and the instrument's mesh handle.
(defstate lab-wiring nil)
(defstate lab-places (list))
(defstate lab-look :solid)
(defstate lab-assembly nil)
(defstate lab-zoom 1.7)
;; Update draft's progress (draft-status): :idle, :compiling, :done or
;; :failed, and the error.
(defstate lab-draft :idle)
(defstate lab-draft-error nil)

(def materials
  (list mat/steel mat/brass mat/bronze mat/copper mat/aluminium mat/titanium mat/silver mat/gold
        mat/lead mat/uranium mat/glass mat/granite mat/ice mat/carbon mat/diamond mat/nylon
        mat/spruce mat/maple mat/rosewood mat/mahogany))

(def material-named (name) (or (find-by-key materials :name name) mat/steel))

;; ── The solid being edited ──

(def current-solid () (if (< lab-index (len lab-solids)) (nth lab-solids lab-index) nil))

;; True when the lab can write the solid back: a spec dict (not a spec file)
;; and a (solid 'id …) call in the script.
(def editable? ()
  (let ((s (current-solid)))
    (and (not (= s nil)) (map? lab-spec) (get s :editable))))

(def shape () (if (map? lab-spec) (get lab-spec :solid) nil))

(def part () (sp/body (shape)))

(def remove-nth (xs i) (map (lambda (j) (nth xs (if (< j i) j (+ j 1)))) (range (- (len xs) 1))))
(def insert-after (xs i x)
  (map (lambda (j) (if (<= j i) (nth xs j) (if (= j (+ i 1)) x (nth xs (- j 1))))) (range (+ (len xs) 1))))

;; ── Loading and solving ──

;; M-x entry point: (re)load the script in the current buffer (or the one the
;; lab is showing) and open the lab beside it.
(def shape-lab ()
  :doc "Design the modal solids of the script in this buffer, with a live 3-D view."
  :command "Shape Lab"
  :group "View"
  (host-command "shape-lab-load"
    (dict :buffer (if (= (current-buffer-name) "*shape-lab*") lab-buffer (current-buffer-name)))))

;; The host's answer to shape-lab-load.
(def loaded (buffer dir solids &rest wiring)
  (set! lab-buffer buffer)
  (set! lab-dir dir)
  (set! lab-solids solids)
  (set! lab-wiring (if (empty? wiring) nil (first wiring)))
  (set! lab-places (map (lambda (s) (list (get s :id) (get s :place))) solids))
  (select-solid (if (< lab-index (len solids)) lab-index 0)))

(def select-solid (i)
  (set! lab-index i)
  (let ((s (current-solid)))
    (set! lab-spec (if (= s nil) nil (get s :spec)))
    (set! lab-points (if (and (not (= s nil)) (map? (get s :points))) (get s :points) (dict)))
    (set! lab-named nil)
    (set! lab-edited (and (not (= s nil)) (= (get s :edited) true)))
    (set! lab-point 0)
    (set! lab-part 0)
    (set! lab-editor :profile)
    (set! lab-draft :idle)
    (set! lab-draft-error nil)
    (set! lab-ticket (+ lab-ticket 1))
    ;; the script's own solve, already done
    (set! lab-solved (if (and (not (= s nil)) (string? (get s :mesh-file)))
                       (dict :file (get s :mesh-file) :freqs (get s :freqs) :mass (get s :mass))
                       nil))
    (refresh-preview)
    (fit-view)
    (rebuild-canvas)
    (when (and (= lab-solved nil) (map? lab-spec)) (request-solve))))

;; Keep the solid being edited in lab-solids, as it now is: what its tab
;; shows when it is picked again and what the instrument view draws while
;; another solid is edited. Its solve goes with it only if it is current;
;; :edited marks a spec not yet written to the script.
(def keep-solid! ()
  (let ((s (current-solid)))
    (when (not (= s nil))
      (set! lab-solids
        (set-nth lab-solids lab-index
          (merge s :spec lab-spec :points lab-points :edited lab-edited
                   :mesh-file (if (solved?) (get lab-solved :file) nil)
                   :freqs (if (solved?) (get lab-solved :freqs) nil)
                   :mass (if (solved?) (get lab-solved :mass) nil)))))))

;; A solid's tab: the one left keeps its edits.
(def switch-solid (i)
  (keep-solid!)
  (select-solid i))

(def refresh-preview ()
  (set! lab-preview (if (map? lab-spec) (modal-mesh lab-spec lab-dir) nil))
  (rebuild-assembly))

;; Ask for a background solve of the spec as it is now; an earlier one's
;; result is then ignored. A shape its clamp misses is not sent: the solver
;; would refuse it.
(def request-solve ()
  (set! lab-ticket (+ lab-ticket 1))
  (if (clamp-misses?)
    (set! lab-solved (dict :error "not held: its clamp is in empty space"))
    (do (set! lab-solved nil)
        (set! lab-solve-progress (list "starting" 0))
        (host-command "shape-lab-solve" (dict :ticket lab-ticket :spec lab-spec :dir lab-dir)))))

;; Stop the running solve; the shape stays unsolved until the next edit or
;; Solve.
(def cancel-solve ()
  (host-command "shape-lab-cancel" (dict))
  (set! lab-ticket (+ lab-ticket 1))
  (set! lab-solve-progress nil)
  (set! lab-solved (dict :error "cancelled" :cancelled true)))

;; The host's progress on solve `ticket`.
(def solve-progress (ticket phase fraction)
  (when (and (= ticket lab-ticket) (= lab-solved nil))
    (set! lab-solve-progress (list phase fraction))))

;; ── The clamp ──
;; A solid's :block: the region the solver holds still (where a cymbal sits
;; on its stand). Nil for a free solid. The preview counts the mesh nodes
;; it holds (:blocked) and finds the node nearest its centre.

(def held-by () (if (map? lab-spec) (get lab-spec :block) nil))

(def clamp-misses? ()
  (and (not (= (held-by) nil)) (map? lab-preview) (= (get lab-preview :blocked) 0)))

(def set-clamp! (block) (edit-spec! :block block true))

(def free-it () (set-clamp! nil))

;; Move the clamp onto the shape: its centre to the nearest mesh node.
(def clamp-to-shape ()
  (let ((nearest (get lab-preview :block-nearest)))
    (when nearest (set-clamp! (sp/move-region (held-by) nearest)))))

;; Clamp a free solid: a small ball at the middle of its body, moved onto
;; the shape if the middle is empty (a bowl's is).
(def clamp-it ()
  (let ((bd (body-bounds)))
    (edit-spec! :block (dict :sphere (dict :center (map (lambda (i) (snap (* 0.5 (+ (nth (first bd) i) (nth (nth bd 1) i))))) (range 3))
                                           :radius (snap (max 0.002 (* 0.04 (body-extent))))))
                false)
    (if (clamp-misses?) (clamp-to-shape) (request-solve))))

;; The host's answer to shape-lab-solve.
(def solved (ticket result)
  (when (= ticket lab-ticket)
    (set! lab-solve-progress nil)
    (set! lab-solved result)
    (rebuild-assembly)))

;; The spec with `s` as its shape: previewed now, solved when `solve`.
(def edit-shape! (s solve) (edit-spec! :solid s solve))

;; The spec with `key` set: previewed and redrawn now, solved when `solve`.
;; An unsolved edit (a drag) makes the last solve stale: the 3-D view shows
;; the preview until the release solves again.
(def edit-spec! (key value solve)
  (set! lab-spec (merge lab-spec key value))
  (set! lab-edited true)
  (when (not solve)
    (set! lab-ticket (+ lab-ticket 1))
    (set! lab-solve-progress nil)
    (set! lab-solved nil))
  (refresh-preview)
  (rebuild-canvas)
  (when solve (request-solve)))

;; Set `key` of the shape's own dict (its :revolve, :box, …).
(def set-part! (key value solve)
  (let ((k (kind-of (shape))))
    (edit-shape! (dict k (merge (part) key value)) solve)))

(def set-material! (name) (edit-spec! :made-of (material-named name) true))

(def set-modes! (n) (edit-spec! :modes (round n) true))

;; Write the solid into the script and evaluate it (its draft follows).
(def update-draft ()
  (when (and (editable?) (solved?))
    (set! lab-edited false)
    (set! lab-draft-error nil)
    (host-command "shape-lab-apply"
      (dict :buffer lab-buffer :id (get (current-solid) :id) :shape (shape)
            :made-of (get (get lab-spec :made-of) :name) :modes (get lab-spec :modes)
            :resolution (get lab-spec :resolution) :block (or (held-by) :free) :points lab-points
            :place (or (place-of (get (current-solid) :id)) (dict))))
    (keep-solid!)))

;; ── Templates ──

(def template-bell ()
  (dict :revolve
        (dict :profile (list (list 0 0.25 0.012) (list 0.04 0.25 0.012) (list 0.07 0.245 0.01)
                             (list 0.085 0.235 0.009) (list 0.09 0.21 0.008) (list 0.092 0.17 0.007)
                             (list 0.097 0.12 0.008) (list 0.108 0.07 0.01) (list 0.125 0.035 0.016)
                             (list 0.14 0.012 0.02) (list 0.15 0 0.016))
              :segments (dict :along 28 :around 52 :through 1))))

(def template-bowl ()
  (dict :revolve
        (dict :profile (list (list 0 0 0.005) (list 0.05 0 0.005) (list 0.08 0.006 0.0045)
                             (list 0.097 0.022 0.004) (list 0.104 0.045 0.0035) (list 0.105 0.07 0.003))
              :segments (dict :along 24 :around 52 :through 1))))

(def template-gong ()
  (dict :revolve
        (dict :profile (list (list 0 0.004 0.003) (list 0.06 0.004 0.0025) (list 0.18 0 0.002)
                             (list 0.2 -0.012 0.002) (list 0.205 -0.03 0.0025))
              :segments (dict :along 24 :around 56 :through 1))))

(def template-chime () (dict :tube (dict :outer-radius 0.016 :inner-radius 0.0135 :height 0.5)))
(def template-bar () (dict :box (dict :size (list 0.35 0.04 0.012))))
(def template-sphere () (dict :shell (dict :outer-radius 0.1 :inner-radius 0.094)))

;; A bar drilled at its fundamental's nodes (0.224 of its length from each
;; end), where a cord holds it without damping it.
(def template-drilled ()
  (dict :difference
        (list (dict :box (dict :size (list 0.3 0.04 0.012)))
              (dict :cylinder (dict :radius 0.004 :height 0.02 :center (list -0.0828 0 0)))
              (dict :cylinder (dict :radius 0.004 :height 0.02 :center (list 0.0828 0 0))))))

;; A marimba bar: the arch cut under it lowers its fundamental to tune the
;; overtones (4:1 and 10:1 on a real one).
(def template-marimba ()
  (dict :difference
        (list (dict :box (dict :size (list 0.38 0.05 0.02)))
              (dict :cylinder (dict :radius 0.6548 :height 0.08 :axis :y :center (list 0 0 -0.6548))))))

;; A tuning fork: two tines on a bridge, on a stem.
(def template-fork ()
  (dict :union
        (list (dict :box (dict :size (list 0.006 0.006 0.1) :center (list -0.009 0 0.058)))
              (dict :box (dict :size (list 0.006 0.006 0.1) :center (list 0.009 0 0.058)))
              (dict :box (dict :size (list 0.024 0.006 0.008) :center (list 0 0 0.004)))
              (dict :box (dict :size (list 0.006 0.006 0.05) :center (list 0 0 -0.025))))))

;; A plate with a round hole off centre.
(def template-plate ()
  (dict :difference
        (list (dict :box (dict :size (list 0.3 0.2 0.004)))
              (dict :cylinder (dict :radius 0.04 :height 0.01 :center (list 0.06 0.03 0))))))

;; (name make cells): cells, when set, is the mesh detail it needs.
(def templates
  (list (list "Bell" template-bell nil) (list "Bowl" template-bowl nil) (list "Gong" template-gong nil)
        (list "Chime" template-chime nil) (list "Bar" template-bar nil) (list "Shell" template-sphere nil)
        (list "Drilled" template-drilled 96) (list "Marimba" template-marimba 80)
        (list "Fork" template-fork 64) (list "Plate" template-plate 64)))

(def use-template (template)
  (set! lab-point 0)
  (set! lab-part 0)
  (set! lab-editor :profile)
  ;; a clamp placed for the old shape means nothing on the new one
  (set! lab-spec (merge lab-spec :block nil))
  (when (nth template 2) (set! lab-spec (merge lab-spec :resolution (dict :cells (nth template 2)))))
  (edit-shape! ((nth template 1)) true)
  (fit-view)
  (rebuild-canvas))

;; ── The profile canvas ──
;; A revolve's cross-section (its profile points with their walls, mirrored
;; about the axis) drawn as a flat mesh: x is r, y is z, z is the drawing
;; depth. lab-view maps it to the widget, and inverts the pointer's -1..1
;; back to (r, z), so a dragged point stays under the pointer.

;; Width / height of one cell in pixels, for an undistorted canvas.
(def cell-aspect 0.46)
(def canvas-cols 30)
(def canvas-rows 15)
(def canvas-aspect (* cell-aspect (/ canvas-cols canvas-rows)))

(defwidget profile-face
  :width 30 :height 15
  :state (cr cz hr hz)
  :vertex
  (out :position (vec3 (/ (- (dot position (vec3 1 0 0)) cr) hr)
                       (/ (- (dot position (vec3 0 1 0)) cz) hz)
                       (dot position (vec3 0 0 1)))
       :varyings ((kind (table vertex-id 0))))
  :shader
  (if (< kind 0.5) (rgba 0.84 0.62 0.34 1.0)
    (if (< kind 1.5) (rgba 0.93 0.94 0.96 1.0)
      (if (< kind 2.5) (rgba 0.28 0.62 1.0 1.0)
        (if (< kind 3.5) (rgba 0.46 0.48 0.53 1.0)
          (if (< kind 4.5) (rgba 0.42 0.33 0.22 1.0)
            (rgba 0.36 0.85 0.52 0.9)))))))

(def profile () (get (get (shape) :revolve) :profile))

;; The clamp's centre on the profile canvas: (r z), r its distance from the
;; axis (a revolve about z through the origin).
(def clamp-rz ()
  (let ((c (sp/region-center (held-by))))
    (list (sqrt (+ (* (first c) (first c)) (* (nth c 1) (nth c 1)))) (nth c 2))))

;; The clamp on the profile canvas, and its mirror image across the axis.
(def profile-clamp-polygons ()
  (if (= (held-by) nil) (list)
    (let ((rz (clamp-rz))
          (at (lambda (r) (sp/move-region (held-by) (list r 0 (nth rz 1)))))
          (sides (if (> (first rz) 0.0001) (list (first rz) (* -1 (first rz))) (list 0))))
      (apply append
        (map (lambda (r) (map (lambda (poly) (list poly 5 0.05)) (or (sp/footprint (sp/region-shape (at r)) 0 2) (list))))
             sides)))))

;; Point i's wall: its own third value, else the revolve's :thickness (nil
;; for a filled profile).
(def wall-of (p)
  (if (> (len p) 2) (nth p 2) (get (get (shape) :revolve) :thickness)))

;; Frame the shape: the parts' body, or a profile and its mirror image.
(def fit-view ()
  (if (parts-mode?) (fit-parts-view) (fit-profile-view)))

(def fit-profile-view ()
  (when (= (kind-of (shape)) :revolve)
    (let ((pts (profile))
          (pad (lambda (p) (let ((w (wall-of p))) (if (= w nil) 0 (* 0.5 w)))))
          (rmax (reduce (lambda (m p) (max m (+ (first p) (pad p)))) 0.01 pts))
          (zmin (reduce (lambda (m p) (min m (- (nth p 1) (pad p)))) 1000 pts))
          (zmax (reduce (lambda (m p) (max m (+ (nth p 1) (pad p)))) -1000 pts))
          (hz (* 0.58 (max (- zmax zmin) (/ (* 2 rmax) canvas-aspect)))))
      (set! lab-view (dict :cr 0 :cz (* 0.5 (+ zmin zmax)) :hr (* hz canvas-aspect) :hz hz)))))

;; A polygon to draw: its convex (x y) corners, colour kind and depth.
(def quad (a b c d kind depth) (list (list a b c d) kind depth))

;; The wall strip from point p to point q (r mirrored by `side`).
(def wall-quad (p q side kind)
  (let ((dr (- (first q) (first p)))
        (dz (- (nth q 1) (nth p 1)))
        (l (max 0.000001 (sqrt (+ (* dr dr) (* dz dz)))))
        (nr (/ (* -1 dz) l))
        (nz (/ dr l))
        (hp (* 0.5 (or (wall-of p) (* 0.004 (get lab-view :hz)))))
        (hq (* 0.5 (or (wall-of q) (* 0.004 (get lab-view :hz)))))
        (at (lambda (pt h s) (list (* side (+ (first pt) (* s h nr))) (+ (nth pt 1) (* s h nz))))))
    (quad (at p hp 1) (at q hq 1) (at q hq -1) (at p hp -1) kind (if (= side 1) 0.5 0.6))))

(def square (r z half kind)
  (quad (list (- r half) (- z half)) (list (+ r half) (- z half))
        (list (+ r half) (+ z half)) (list (- r half) (+ z half)) kind 0.1))

(def canvas-quads ()
  (let ((pts (profile))
        (n (len pts))
        (v lab-view)
        (handle (* 0.035 (get v :hz)))
        (axis (* 0.008 (get v :hz)))
        (segs (range (- n 1))))
    (append
      (map (lambda (i) (square (first (nth pts i)) (nth (nth pts i) 1) handle (if (= i lab-point) 2 1)))
           (range n))
      (map (lambda (i) (wall-quad (nth pts i) (nth pts (+ i 1)) 1 0)) segs)
      (map (lambda (i) (wall-quad (nth pts i) (nth pts (+ i 1)) -1 4)) segs)
      (profile-clamp-polygons)
      (list (quad (list (* -1 axis) (- (get v :cz) (get v :hz))) (list axis (- (get v :cz) (get v :hz)))
                  (list axis (+ (get v :cz) (get v :hz))) (list (* -1 axis) (+ (get v :cz) (get v :hz))) 3 0.7)))))

(def rebuild-canvas ()
  (if (parts-mode?)
    (set! lab-canvas (polygons-geometry (parts-polygons) lab-canvas))
    (when (= (kind-of (shape)) :revolve)
      (set! lab-canvas (polygons-geometry (canvas-quads) lab-canvas)))))

;; Polygons (corners kind depth) as a flat mesh: x y and depth, each fanned
;; into triangles, its kind the vertices' one table column.
(def polygons-geometry (polys previous)
  (let ((sizes (map (lambda (q) (len (first q))) polys))
        (starts (reduce (lambda (acc n) (append acc (list (+ (nth acc (- (len acc) 1)) n)))) (list 0) sizes)))
    (mesh-geometry
      (dict :points (apply append (map (lambda (q) (apply append (map (lambda (c) (list (first c) (nth c 1) (nth q 2)))
                                                                     (first q))))
                                       polys))
            :normals (apply append (map (lambda (i) (list 0 0 1)) (range (nth starts (len polys)))))
            :triangles (apply append (map (lambda (k) (let ((o (nth starts k)))
                                                        (apply append (map (lambda (j) (list o (+ o j) (+ o j 1)))
                                                                           (range 1 (- (nth sizes k) 1))))))
                                          (range (len polys))))
            :shapes (apply append (map (lambda (q) (map (lambda (c) (nth q 1)) (first q))) polys))
            :modes 1)
      previous)))

;; The pointer's -1..1 (y down) as (r z).
(def canvas-at (sx sy)
  (let ((v lab-view))
    (list (+ (get v :cr) (* sx (get v :hr))) (- (get v :cz) (* sy (get v :hz))))))

(def nearest-point (at)
  (let ((pts (profile)))
    (reduce (lambda (best i)
              (let ((p (nth pts i))
                    (d (+ (pow (- (abs (first at)) (first p)) 2) (pow (- (nth at 1) (nth p 1)) 2))))
                (if (< d (nth best 1)) (list i d) best)))
            (list -1 1000000) (range (len pts)))))

(def canvas-down (sx sy region)
  (let ((at (canvas-at sx sy))
        (hit (nearest-point at))
        (reach (* 0.08 (get lab-view :hz))))
    (set! lab-drag-clamp false)
    (if (< (nth hit 1) (* reach reach))
      (do (set! lab-point (first hit))
          (set! lab-dragging true)
          (rebuild-canvas))
      (if (and (not (= (held-by) nil)) (sp/hit? (map first (profile-clamp-polygons)) (first at) (nth at 1)))
        (let ((rz (clamp-rz))) (grab-clamp! (list (abs (first at)) (nth at 1)) (first rz) (nth rz 1)))
        (set! lab-dragging false)))))

(def canvas-drag (sx sy region)
  (when lab-dragging
    (let ((at (canvas-at sx sy)))
      (if lab-drag-clamp
        (drag-clamp! at (list (max 0 (+ (abs (first at)) (first lab-grab))) 0 (+ (nth at 1) (nth lab-grab 1))))
        (let ((p (nth (profile) lab-point))
              (moved (append (list (snap (max 0 (abs (first at)))) (snap (nth at 1)))
                             (if (> (len p) 2) (list (nth p 2)) (list)))))
          (set-part! :profile (set-nth (profile) lab-point moved) false))))))

(def canvas-up (sx sy region)
  (when lab-dragging
    (set! lab-dragging false)
    (request-solve)))

;; Set field k (0 r, 1 z, 2 wall) of the selected point.
;; Only a wall gives a 2-element point a third value: one on a filled
;; profile would make the whole profile walled.
(def set-point-field! (k value)
  (let ((p (nth (profile) lab-point))
        (full (if (or (> (len p) 2) (not (= k 2))) p (list (first p) (nth p 1) (or (wall-of p) 0.005)))))
    (set-part! :profile (set-nth (profile) lab-point (set-nth full k (snap value))) true)))

;; A new point halfway to the next one (or past the last), selected.
(def add-point ()
  (let ((pts (profile))
        (p (nth pts lab-point))
        (q (if (< (+ lab-point 1) (len pts)) (nth pts (+ lab-point 1))
             (list (+ (first p) (* 0.1 (get lab-view :hz))) (nth p 1))))
        (mid (lambda (k) (snap (* 0.5 (+ (nth p k) (nth q k))))))
        (point (append (list (mid 0) (mid 1)) (if (> (len p) 2) (list (nth p 2)) (list)))))
    (set-part! :profile (insert-after pts lab-point point) true)
    (set! lab-point (+ lab-point 1))
    (rebuild-canvas)))

(def remove-point ()
  (when (> (len (profile)) 2)
    (set-part! :profile (remove-nth (profile) lab-point) true)
    (set! lab-point (max 0 (- lab-point 1)))
    (rebuild-canvas)))

;; Every wall times k.
(def scale-walls (k)
  (set-part! :profile
    (map (lambda (p) (if (> (len p) 2) (list (first p) (nth p 1) (snap (max 0.0002 (* k (nth p 2))))) p))
         (profile))
    true))

;; Every point (and wall) times k: the same shape, bigger or smaller.
(def scale-shape (k)
  (set-part! :profile (map (lambda (p) (map (lambda (x) (snap (* k x))) p)) (profile)) true)
  (fit-view)
  (rebuild-canvas))

;; ── Named points ──
;; Spots on the solid with names, written to its :points: a script names
;; them where a mallet strikes, a string's bridge sits or a mic listens
;; ((m/listen 'bell :lip)). The Point tool places one on the surface under
;; the click, seen from the view's side; Move drags it across the view and
;; drops it back onto the surface.

(def point-layer -2)

(def point-selected? () (and (= lab-part point-layer) (not (= lab-named nil)) (get lab-points lab-named)))

(def set-points! (pts)
  (set! lab-points pts)
  (set! lab-edited true)
  (rebuild-canvas)
  (rebuild-assembly))

(def set-point! (name xyz) (set-points! (merge lab-points name xyz)))

(def without-point (name)
  (apply dict (apply append (map (lambda (k) (list k (get lab-points k)))
                                 (filter (lambda (k) (not (= k name))) (keys lab-points))))))

(def select-point! (name)
  (set! lab-part point-layer)
  (set! lab-named name)
  (set! lab-point-draft (bare-name name))
  (rebuild-canvas))

(def delete-point ()
  (when (point-selected?)
    (set-points! (without-point lab-named))
    (set! lab-named nil)
    (set! lab-part 0)
    (rebuild-canvas)))

;; :tip -> "tip"
(def bare-name (k) (let ((s (source k))) (substring s 1 (len s))))

;; The keyword a typed name stands for, or nil when it is not one.
(def name-keyword (text)
  (let ((k (if (= text "") nil (read-string (str ":" text)))))
    (if (and (not (= k nil)) (= (source k) (str ":" text))) k nil)))

;; :p1, :p2, … the first not taken.
(def fresh-point-name ()
  (let ((taken (map source (keys lab-points)))
        (free (filter (lambda (n) (empty? (filter (lambda (t) (= t (str ":p" n))) taken))) (range 1 (+ 2 (len taken))))))
    (read-string (str ":p" (first free)))))

;; Rename the selected point to what was typed (a free, valid name).
(def rename-point ()
  (let ((k (name-keyword lab-point-draft)))
    (when (and (point-selected?) (not (= k nil)) (not (= k lab-named)) (= (get lab-points k) nil))
      (let ((xyz (get lab-points lab-named)))
        (set-points! (merge (without-point lab-named) k xyz))
        (set! lab-named k)))))

;; The surface point under (u v) in the view plane, seen from the view's
;; side (modal-mesh-pick on the preview mesh); without a mesh, the point on
;; the body's bounds toward the viewer.
(def surface-at (u v)
  (let ((axes (plane-axes))
        (id (if (map? lab-preview) (get lab-preview :mesh-id) nil))
        (picked (if (= id nil) nil (modal-mesh-pick id (first axes) (nth axes 1) (nth axes 2) u v))))
    (if (list? picked) (map snap picked)
      (set-nth (set-nth (set-nth (list 0 0 0) (first axes) (snap u)) (nth axes 1) (snap v))
               (nth axes 2) (nth (nth (body-bounds) 1) (nth axes 2))))))

(def place-point! (u v)
  (let ((name (fresh-point-name)))
    (set-point! name (surface-at u v))
    (select-point! name)))

;; Drop the selected point back onto the surface under it.
(def snap-point ()
  (when (point-selected?)
    (let ((axes (plane-axes)) (xyz (get lab-points lab-named)))
      (set-point! lab-named (surface-at (nth xyz (first axes)) (nth xyz (nth axes 1)))))))

;; A marker's half-size on the canvas (u and v), square on screen.
(def marker-half ()
  (let ((v lab-view) (hu (* 0.028 (get v :hr))))
    (list hu (* hu (/ (get v :hz) (get v :hr)) canvas-aspect))))

;; The named points as markers (kind 6, the selected one 7).
(def point-polygons (a b)
  (let ((h (marker-half)))
    (map (lambda (k)
           (let ((xyz (get lab-points k)) (u (nth xyz a)) (v (nth xyz b)))
             (list (list (list (- u (first h)) (- v (nth h 1))) (list (+ u (first h)) (- v (nth h 1)))
                         (list (+ u (first h)) (+ v (nth h 1))) (list (- u (first h)) (+ v (nth h 1))))
                   (if (and (= lab-part point-layer) (= k lab-named)) 7 6)
                   0.04)))
         (keys lab-points))))

;; The named point under (u v), or nil.
(def point-hit (u v)
  (let ((axes (plane-axes))
        (h (marker-half))
        (hits (filter (lambda (k) (let ((xyz (get lab-points k)))
                                    (and (<= (abs (- (nth xyz (first axes)) u)) (* 1.6 (first h)))
                                         (<= (abs (- (nth xyz (nth axes 1)) v)) (* 1.6 (nth h 1))))))
                      (keys lab-points))))
    (if (empty? hits) nil (first hits))))

;; ── The parts editor ──
;; A shape as parts (eseq.modal.shape-parts) drawn in one view plane: x
;; across and y up for :top, x and z for :front, y and z for :side. Added
;; parts are bronze, cuts red over them, the selected part brighter. A thin
;; body is stretched up to fill the canvas (lab-view :ex times), which the
;; pointer's inverse undoes.

(def parts-mode? ()
  (let ((k (kind-of (shape))))
    (if (= k :revolve) (= lab-editor :parts)
      (and (not (= k nil)) (not (= k :obj))))))

(def layer-parts () (sp/parts-of (shape)))

(def selected-part ()
  (let ((ps (layer-parts))) (if (and (>= lab-part 0) (< lab-part (len ps))) (nth ps lab-part) nil)))

;; (a b n): the view plane's across and up axes, and the one it looks along.
(def plane-axes () (match lab-plane :top (list 0 1 2) :front (list 0 2 1) _ (list 1 2 0)))

;; The added parts' bounds: what holes go through and patterns fit.
(def added-parts () (filter (lambda (p) (not (get p :cut))) (layer-parts)))

(def body-bounds ()
  (or (sp/union-bounds (added-parts))
      (list (list -0.05 -0.05 -0.05) (list 0.05 0.05 0.05))))

(def set-parts! (ps solve) (edit-shape! (sp/shape-of ps) solve))

(def set-selected! (p solve) (set-parts! (set-nth (layer-parts) lab-part p) solve))

;; Add part p and select it.
(def add-part! (p solve)
  (let ((ps (append (layer-parts) (list p))))
    (set! lab-part (sp/reorder ps (- (len ps) 1)))
    (set-parts! ps solve)))

(def added-count () (len (added-parts)))

;; The body's longest side, metres.
(def body-extent ()
  (let ((bd (body-bounds)))
    (reduce (lambda (m i) (max m (- (nth (nth bd 1) i) (nth (first bd) i)))) 0 (range 3))))

;; Added <-> cut; the body always keeps one added part.
(def toggle-cut ()
  (let ((p (selected-part)))
    (when (or (get p :cut) (> (added-count) 1))
      (let ((ps (set-nth (layer-parts) lab-part (merge p :cut (not (get p :cut))))))
        (set! lab-part (sp/reorder ps lab-part))
        (set-parts! ps true)))))

(def delete-part ()
  (let ((p (selected-part)))
    (when (or (get p :cut) (> (added-count) 1))
      (let ((ps (remove-nth (layer-parts) lab-part)))
        (set! lab-part (max 0 (- lab-part 1)))
        (set-parts! ps true)))))

;; A copy of the selected part beside it (a tenth of the body across).
(def copy-part ()
  (let ((p (selected-part))
        (a (first (plane-axes)))
        (c (sp/center-of (get p :shape)))
        (step (* 0.1 (- (nth (nth (body-bounds) 1) a) (nth (first (body-bounds)) a))))
        (ps (insert-after (layer-parts) lab-part (merge p :shape (sp/with-center (get p :shape) (set-nth c a (+ (nth c a) step)))))))
    (set! lab-part (+ lab-part 1))
    (set-parts! ps true)))

;; The selected part as n copies in a row across the view.
(def make-row (n)
  (let ((ps (layer-parts))
        (copies (sp/row (body-bounds) (selected-part) (first (plane-axes)) n)))
    (set-parts! (append (map (lambda (j) (nth ps j)) (range lab-part))
                        copies
                        (map (lambda (j) (nth ps j)) (range (+ lab-part 1) (len ps))))
                true)))

;; The part tool `tool` places at (u v) in the view, sized to the body; a
;; hole takes the last hole's radius.
(def placed-part (tool u v)
  (let ((axes (plane-axes))
        (made (if (= tool :arch)
                (arch-at u v)
                ((match tool :hole sp/hole :slot sp/slot _ sp/block)
                 (body-bounds) (first axes) (nth axes 1) (nth axes 2) (snap u) (snap v)))))
    (if (and (= tool :hole) lab-hole-radius)
      (merge made :shape (dict :cylinder (merge (get (get made :shape) :cylinder) :radius lab-hole-radius)))
      made)))

;; An arch under the body, centred where along it the click was (the arch
;; runs along the body's longest side, wherever the view shows that).
(def arch-at (u v)
  (let ((axes (plane-axes))
        (made (sp/arch (body-bounds)))
        (s (get made :shape))
        (c (sp/center-of s))
        (longest (first (sp/axes-by-extent (body-bounds))))
        (moved (if (= longest (first axes)) (set-nth c longest (snap u))
                 (if (= longest (nth axes 1)) (set-nth c longest (snap v)) c))))
    (merge made :shape (sp/with-center s moved))))

(def set-plane! (plane)
  (set! lab-plane plane)
  (fit-view)
  (rebuild-canvas))

(def fit-parts-view ()
  (let ((axes (plane-axes))
        (a (first axes))
        (b (nth axes 1))
        (bd (body-bounds))
        (ha (* 0.6 (max 0.001 (- (nth (nth bd 1) a) (nth (first bd) a)))))
        (hb (* 0.6 (max 0.0005 (- (nth (nth bd 1) b) (nth (first bd) b)))))
        (hr (max ha (* hb canvas-aspect)))
        (hz (/ hr canvas-aspect))
        (ex (clamp (floor (/ (* 0.5 hz) hb)) 1 8)))
    (set! lab-view (dict :cr (* 0.5 (+ (nth (first bd) a) (nth (nth bd 1) a)))
                         :cz (* 0.5 (+ (nth (first bd) b) (nth (nth bd 1) b)))
                         :hr hr :hz (/ hz ex) :ex ex))))

;; Every part's footprint, cuts over added parts, the selection over its
;; layer; then the origin's axes faintly underneath.
(def parts-polygons ()
  (let ((axes (plane-axes))
        (ps (layer-parts))
        (v lab-view)
        (tu (* 0.004 (get v :hr)))
        (tv (* 0.004 (get v :hz)))
        (left (- (get v :cr) (get v :hr)))
        (right (+ (get v :cr) (get v :hr)))
        (bottom (- (get v :cz) (get v :hz)))
        (top (+ (get v :cz) (get v :hz))))
    (append
      (apply append
        (map (lambda (i)
               (let ((p (nth ps i))
                     (selected (= i lab-part))
                     (kind (+ (if (get p :cut) 2 0) (if selected 1 0)))
                     (depth (- (if (get p :cut) (if selected 0.2 0.3) (if selected 0.5 0.6)) (* 0.0002 i))))
                 (map (lambda (poly) (list poly kind depth))
                      (or (sp/footprint (get p :shape) (first axes) (nth axes 1)) (list)))))
             (range (len ps))))
      (clamp-polygons (first axes) (nth axes 1))
      (point-polygons (first axes) (nth axes 1))
      (list (quad (list left (* -1 tv)) (list right (* -1 tv)) (list right tv) (list left tv) 4 0.9)
            (quad (list (* -1 tu) bottom) (list tu bottom) (list tu top) (list (* -1 tu) top) 4 0.9)))))

;; The clamp's footprint in plane (a b), drawn over everything (kind 5).
(def clamp-polygons (a b)
  (if (= (held-by) nil) (list)
    (map (lambda (poly) (list poly 5 0.1)) (or (sp/footprint (sp/region-shape (held-by)) a b) (list)))))

(def clamp-hit? (a b u v)
  (and (not (= (held-by) nil)) (sp/hit? (sp/footprint (sp/region-shape (held-by)) a b) u v)))

;; Start dragging the clamp from `at`, its centre seen in plane (a b).
(def grab-clamp! (at ca cb)
  (set! lab-grab (list (- ca (first at)) (- cb (nth at 1))))
  (set! lab-drag-clamp true)
  (set! lab-dragging true))

(def drag-clamp! (at center)
  (set! lab-moved true)
  (edit-spec! :block (sp/move-region (held-by) center) false))

(defwidget parts-face
  :width 30 :height 15
  :state (cr cz hr hz)
  :vertex
  (out :position (vec3 (/ (- (dot position (vec3 1 0 0)) cr) hr)
                       (/ (- (dot position (vec3 0 1 0)) cz) hz)
                       (dot position (vec3 0 0 1)))
       :varyings ((kind (table vertex-id 0))))
  :shader
  (if (< kind 0.5) (rgba 0.72 0.53 0.3 1.0)
    (if (< kind 1.5) (rgba 0.96 0.76 0.46 1.0)
      (if (< kind 2.5) (rgba 0.5 0.2 0.19 0.92)
        (if (< kind 3.5) (rgba 0.95 0.38 0.32 0.95)
          (if (< kind 4.5) (rgba 0.3 0.32 0.36 1.0)
            (if (< kind 5.5) (rgba 0.36 0.85 0.52 0.9)
              (if (< kind 6.5) (rgba 0.33 0.6 1.0 1.0) (rgba 0.8 0.9 1.0 1.0)))))))))

;; The parts topmost first: cuts, then added parts, later ones over earlier.
(def parts-hit (u v)
  (let ((ps (layer-parts))
        (axes (plane-axes))
        (cut? (lambda (i) (get (nth ps i) :cut)))
        (order (append (reverse (filter cut? (range (len ps))))
                       (reverse (filter (lambda (i) (not (cut? i))) (range (len ps))))))
        (hits (filter (lambda (i) (sp/hit? (sp/footprint (get (nth ps i) :shape) (first axes) (nth axes 1)) u v)) order)))
    (if (empty? hits) nil (first hits))))

(def grab! (at)
  (let ((axes (plane-axes))
        (c (sp/center-of (get (selected-part) :shape))))
    (set! lab-grab (list (- (nth c (first axes)) (first at)) (- (nth c (nth axes 1)) (nth at 1))))
    (set! lab-dragging true)))

(def parts-down (sx sy region)
  (let ((at (canvas-at sx sy))
        (axes (plane-axes))
        (a (first axes))
        (b (nth axes 1)))
    (set! lab-moved false)
    (set! lab-drag-clamp false)
    (set! lab-drag-point false)
    (if (= lab-tool :point)
      (do (place-point! (first at) (nth at 1))
          (set! lab-tool :select)
          (set! lab-dragging false))
    (if (not (= lab-tool :select))
      (do (add-part! (placed-part lab-tool (first at) (nth at 1)) false)
          (set! lab-tool :select)
          (set! lab-moved true)
          (grab! at))
      (if (point-hit (first at) (nth at 1))
        (let ((name (point-hit (first at) (nth at 1))) (xyz (get lab-points name)))
          (select-point! name)
          (set! lab-grab (list (- (nth xyz a) (first at)) (- (nth xyz b) (nth at 1))))
          (set! lab-drag-point true)
          (set! lab-dragging true))
      (if (clamp-hit? a b (first at) (nth at 1))
        (let ((c (sp/region-center (held-by)))) (grab-clamp! at (nth c a) (nth c b)))
        (let ((hit (parts-hit (first at) (nth at 1))))
          (if (= hit nil)
            (set! lab-dragging false)
            (do (set! lab-part hit)
                (grab! at)
                (rebuild-canvas))))))))))

(def parts-drag (sx sy region)
  (when lab-dragging
    (let ((at (canvas-at sx sy))
          (axes (plane-axes))
          (place (lambda (c) (set-nth (set-nth c (first axes) (+ (first at) (first lab-grab)))
                                          (nth axes 1) (+ (nth at 1) (nth lab-grab 1))))))
      (if lab-drag-point
        (do (set! lab-moved true)
            (set-point! lab-named (place (get lab-points lab-named))))
      (if lab-drag-clamp
        (drag-clamp! at (place (sp/region-center (held-by))))
        (let ((p (selected-part)))
          (set! lab-moved true)
          (set-selected! (merge p :shape (sp/with-center (get p :shape) (place (sp/center-of (get p :shape))))) false)))))))

(def parts-up (sx sy region)
  (when lab-dragging
    (set! lab-dragging false)
    ;; a point is not part of the solve: it lands back on the surface
    (if lab-drag-point
      (do (set! lab-drag-point false)
          (when lab-moved (snap-point)))
      (when lab-moved (request-solve)))))

;; Mesh detail. A revolve body with :segments (the bell templates) keeps
;; its body-fitted mesh under cuts, its elements set by those counts: Detail
;; scales them. Anything else is meshed by :resolution cells across its
;; longest side (voxels, or a lone revolve's fitted mesh).
(def detail-steps (list 16 24 32 48 64 80 96 128))

(def detail () (or (get (get lab-spec :resolution) :cells) 32))

;; A shape keeps a body-fitted mesh (cuts carved out of it) only when its
;; body is one walled revolve, tube, shell or cylinder; anything else (a
;; union above all) is meshed by voxel cubes, which a wall thinner than a
;; cube falls through.
(def fitted? (s)
  (let ((k (kind-of s)))
    (or (= k :tube) (= k :shell) (= k :cylinder)
        (and (= k :revolve) (not (= (sp/wall-of-profile (sp/body s)) nil))))))

(def voxel-body? ()
  (let ((adds (added-parts)))
    (or (> (len adds) 1) (not (fitted? (get (first adds) :shape))))))

;; The body's revolve, when it is meshed by its own :segments, else nil.
(def segmented-revolve ()
  (let ((s (get (first (layer-parts)) :shape)))
    (if (and (not (voxel-body?)) (= (kind-of s) :revolve) (get (sp/body s) :segments)
             (get (get (sp/body s) :segments) :around))
      s nil)))

;; The thinnest wall among the added parts (a revolve's, a tube's or a
;; shell's), or nil.
(def thinnest-wall ()
  (reduce (lambda (m p)
            (let ((s (get p :shape))
                  (b (sp/body s))
                  (w (match (kind-of s)
                       :revolve (sp/wall-of-profile b)
                       :tube (- (get b :outer-radius) (get b :inner-radius))
                       :shell (- (get b :outer-radius) (get b :inner-radius))
                       _ nil)))
              (if (= w nil) m (if (= m nil) w (min m w)))))
          nil (added-parts)))

;; About how wide one mesh element is, metres.
(def cell-size ()
  (let ((r (segmented-revolve)))
    (if r
      (let ((b (sp/body r))
            (seg (get b :segments))
            (pts (get b :profile))
            (arc (reduce (lambda (sum i) (+ sum (sqrt (+ (pow (- (first (nth pts (+ i 1))) (first (nth pts i))) 2)
                                                        (pow (- (nth (nth pts (+ i 1)) 1) (nth (nth pts i) 1)) 2)))))
                         0 (range (- (len pts) 1))))
            (rmax (reduce (lambda (m p) (max m (first p))) 0 pts)))
        (max (/ (* 6.283185307179586 rmax) (get seg :around)) (/ arc (or (get seg :along) 16))))
      (let ((sizes (get (get lab-spec :resolution) :cell-size)))
        ;; per-axis cells: the thinnest (through a top)
        (if (list? sizes) (reduce (lambda (m h) (min m h)) (first sizes) sizes) (/ (body-extent) (detail)))))))

(def set-detail! (cells) (edit-spec! :resolution (dict :cells cells) true))

;; Finer (dir > 0) or coarser.
(def step-detail (dir)
  (let ((r (segmented-revolve)))
    (if r
      (let ((k (if (> dir 0) 1.25 0.8))
            (seg (get (sp/body r) :segments))
            (scaled (merge seg :along (max 4 (round (* k (or (get seg :along) 16)))) :around (max 8 (round (* k (get seg :around)))))))
        (set-parts! (set-nth (layer-parts) 0 (merge (first (layer-parts)) :shape (dict :revolve (merge (sp/body r) :segments scaled))))
                    true))
      (let ((now (detail))
            (finer (filter (lambda (c) (> c now)) detail-steps))
            (coarser (reverse (filter (lambda (c) (< c now)) detail-steps))))
        (if (> dir 0)
          (when (not (empty? finer)) (set-detail! (first finer)))
          (when (not (empty? coarser)) (set-detail! (first coarser))))))))

;; What the mesh will lose: a thin wall under voxel cubes (a union is
;; voxelized), or a cut smaller than about two cells; "" when nothing.
(def detail-warning ()
  (let ((smallest (sp/min-feature (layer-parts)))
        (wall (thinnest-wall))
        (cell (cell-size)))
    (if (and (voxel-body?) (not (= wall nil)) (< wall (* 2 cell)))
      (fmt "Adding a part meshes the body as {:.1} mm cubes, and its {:.1} mm wall is thinner than that, so it will mostly vanish."
           (* 1000 cell) (* 1000 wall))
      (if (and (not (= smallest nil)) (< smallest (* 2 cell)))
        (fmt "A {:.1} mm cut is under two {:.1} mm mesh cells, so it may vanish: make Mesh finer." (* 1000 smallest) (* 1000 cell))
        ""))))

;; ── Controls ──

(def mm-knob (key title value lo hi on-set)
  (knob-number :key key :label title :value value :min lo :max hi :unit "mm" :value-scale 1000
               :decimals 1 :drag-cells 60 :width 7 :height 4 :on-change on-set))

;; A compact millimetre field (type, or drag: half a millimetre per row,
;; Shift ten times that), for a layer's numbers, kept within lo..hi metres.
(def mm-field (key title value lo hi on-set)
  (v-stack :width 7 :gap 0.08
    (label title :font-size 8.5 :color :dim :bg :transparent)
    (number-picker :key key :value (* 1000 value) :min (* 1000 lo) :max (* 1000 hi)
                   :drag :relative :drag-step 0.5 :step 0.1 :decimals 1 :unit "mm"
                   :noui true :font-size 10.2 :text-color :accent :edit-color :yellow :text-align :left
                   :width 7 :height 0.9 :on-change (lambda (v) (on-set (clamp (/ v 1000) lo hi))))))

(def small-button (key text on-click)
  (button text :key key :height 1.3 :font-size 9.5 :on-click (lambda (e) (on-click))))

(def narrow-button (key text on-click)
  (button text :key key :width 2.6 :height 1.3 :font-size 9.5 :on-click (lambda (e) (on-click))))

;; One of a set of choices: filled with its colour when chosen.
(def choice-button (key text chosen colour on-click)
  (if chosen
    (button text :key key :height 1.3 :font-size 9.5 :background-color colour :color :white
            :on-click (lambda (e) (on-click)))
    (button text :key key :height 1.3 :font-size 9.5 :variant :secondary :on-click (lambda (e) (on-click)))))

(def section-label (text)
  (label text :font-size 9 :color :dim :bg :transparent))

(def revolve-controls ()
  (let ((p (nth (profile) lab-point)))
    (v-stack :gap 0.4
      (h-stack :gap 0.6 :align :center
        (label (str "Point " (+ lab-point 1) " of " (len (profile))) :font-size 10 :bg :transparent :color :dim)
        (small-button "lab-add-point" "+ Point" add-point)
        (small-button "lab-remove-point" "− Point" remove-point))
      (h-stack :gap 0.6 :align :center
        (small-button "lab-thinner" "Walls −" (lambda () (scale-walls 0.9)))
        (small-button "lab-thicker" "Walls +" (lambda () (scale-walls 1.1)))
        (small-button "lab-smaller" "Size −" (lambda () (scale-shape 0.9)))
        (small-button "lab-bigger" "Size +" (lambda () (scale-shape 1.1))))
      (h-stack :gap 0.8
        (mm-knob "lab-point-r" "Radius" (first p) 0 1 (lambda (v) (set-point-field! 0 v)))
        (mm-knob "lab-point-z" "Height" (nth p 1) -1 1 (lambda (v) (set-point-field! 1 v)))
        (mm-knob "lab-point-wall" "Wall" (or (wall-of p) 0) 0.0002 0.05 (lambda (v) (set-point-field! 2 v)))))))

;; Knobs for a part's own numbers: (title key lo hi), metres.
(def size-fields (kind)
  (match kind
    :cylinder (list (list "Radius" :radius 0.0005 1) (list "Depth" :height 0.001 2))
    :tube (list (list "Outer r" :outer-radius 0.001 1) (list "Inner r" :inner-radius 0.0005 1) (list "Length" :height 0.002 3))
    :sphere (list (list "Radius" :radius 0.001 1))
    :shell (list (list "Outer r" :outer-radius 0.001 1) (list "Inner r" :inner-radius 0.0005 1))
    _ (list)))

;; Set `key` of the selected part's own dict.
(def set-part-field! (key value)
  (let ((s (get (selected-part) :shape)))
    (when (and (= key :radius) (get (selected-part) :cut) (= (kind-of s) :cylinder))
      (set! lab-hole-radius value))
    (set-selected! (merge (selected-part) :shape (dict (kind-of s) (merge (sp/body s) key value))) true)))

(def part-size-knobs (s)
  (let ((kind (kind-of s))
        (b (sp/body s)))
    (if (= kind :box)
      (let ((size (get b :size)))
        (h-stack :gap 0.6
          (mm-field "lab-box-x" "Length" (nth size 0) 0.0005 2 (lambda (v) (set-part-field! :size (set-nth size 0 (snap v)))))
          (mm-field "lab-box-y" "Width" (nth size 1) 0.0005 2 (lambda (v) (set-part-field! :size (set-nth size 1 (snap v)))))
          (mm-field "lab-box-z" "Height" (nth size 2) 0.0005 2 (lambda (v) (set-part-field! :size (set-nth size 2 (snap v)))))))
      (h-stack :gap 0.6 :align :center
        (each (size-fields kind) |f|
          (mm-field (str "lab-field-" (nth f 0)) (nth f 0) (get b (nth f 1)) (nth f 2) (nth f 3)
                   (lambda (v) (set-part-field! (nth f 1) (snap v)))))
        (if (or (= kind :cylinder) (= kind :tube))
          (v-stack :gap 0.2
            (section-label "Axis")
            (dropdown :key "lab-part-axis" :width 6 :height 1.3 :font-size 9.5
                      :value (nth (list "x" "y" "z") (sp/axis-of b))
                      :options (list "x" "y" "z")
                      :on-change |name| (set-part-field! :axis (match name "x" :x "y" :y _ :z))))
          (label "" :bg :transparent))))))

(def position-knobs (c on-set)
  (h-stack :gap 0.6
    (each (range 3) |i|
      (mm-field (str "lab-center-" i) (nth (list "X" "Y" "Z") i) (nth c i) -1 1
               (lambda (v) (on-set (set-nth c i (snap v))))))))

;; ── Layers ──
;; The parts as a tree: the body's parts, then the cuts, then the clamp.
;; Selecting a layer selects it on the canvas; its actions and numbers
;; follow under the tree. lab-part is the selected part's index, or
;; clamp-layer for the clamp.

(def clamp-layer -1)

(def clamp-selected? () (= lab-part clamp-layer))

(def kind-label (k)
  (match k :box "Box" :cylinder "Cylinder" :tube "Tube" :sphere "Sphere" :shell "Shell" :revolve "Revolve"
         :union "Union" :difference "Difference" :obj "Mesh" _ "Shape"))

;; What a part is called in the tree: a cut cylinder is a hole (an arch when
;; it is wider than the body), a cut box a slot, an added box after the
;; first a block.
(def part-noun (ps i)
  (let ((p (nth ps i))
        (s (get p :shape))
        (k (kind-of s)))
    (if (get p :cut)
      (match k
        :cylinder (if (> (get (sp/body s) :radius) (body-extent)) "Arch" "Hole")
        :box "Slot"
        _ (str "Cut " (kind-label k)))
      (if (and (= k :box) (> i 0)) "Block" (kind-label k)))))


;; "Hole 2": the noun numbered among the parts with the same noun.
(def layer-name (ps i)
  (let ((noun (part-noun ps i))
        (same (filter (lambda (j) (= (part-noun ps j) noun)) (range (len ps)))))
    (if (= (len same) 1) noun
      (str noun " " (+ 1 (len (filter (lambda (j) (< j i)) same)))))))

(def select-layer! (i)
  (set! lab-part i)
  ;; a cut is edited on the parts canvas
  (when (and (>= i 0) (get (nth (layer-parts) i) :cut)) (set! lab-editor :parts))
  (rebuild-canvas))

(def solid-swatch "#b8874c")
(def cut-swatch "#c8524a")
(def clamp-swatch "#5cd985")
(def point-swatch "#5499ff")

;; A point's place in the tree: x, y, z in mm.
(def xyz-detail (xyz) (str (mm (first xyz)) ", " (mm (nth xyz 1)) ", " (mm (nth xyz 2)) " mm"))

;; A part's size in the tree: a hole's diameter, a box's sides.
(def layer-detail (s)
  (let ((b (sp/body s)))
    (match (kind-of s)
      :cylinder (fmt "⌀ {:.1} mm" (* 2000 (get b :radius)))
      :box (str (mm (nth (get b :size) 0)) " × " (mm (nth (get b :size) 1)) " × " (mm (nth (get b :size) 2)) " mm")
      :sphere (fmt "⌀ {:.1} mm" (* 2000 (get b :radius)))
      _ nil)))

(def mm (x) (fmt "{:.0}" (* 1000 x)))

;; The tree's items: headers, then a row per part (its :path the part's
;; index) and the clamp's row.
(def layer-items ()
  (let ((ps (layer-parts))
        (row (lambda (i) (with-detail (dict :label (layer-name ps i) :path (str i)
                                            :swatch (if (get (nth ps i) :cut) cut-swatch solid-swatch))
                                      (layer-detail (get (nth ps i) :shape)))))
        (adds (filter (lambda (i) (not (get (nth ps i) :cut))) (range (len ps))))
        (cuts (filter (lambda (i) (get (nth ps i) :cut)) (range (len ps)))))
    (append (list (dict :label "Body" :kind "header"))
            (map row adds)
            (if (empty? cuts) (list) (cons (dict :label "Cuts" :kind "header") (map row cuts)))
            (if (empty? (keys lab-points)) (list)
              (cons (dict :label "Points" :kind "header")
                    (map (lambda (k) (dict :label (bare-name k) :path (str "point" (source k)) :swatch point-swatch
                                           :detail (xyz-detail (get lab-points k))))
                         (keys lab-points))))
            (list (dict :label "Held by" :kind "header")
                  (if (= (held-by) nil)
                    (dict :label "Nothing (free)" :path "clamp")
                    (with-detail (dict :label "Clamp" :path "clamp" :swatch clamp-swatch)
                                 (if (clamp-misses?) "not touching" nil)))))))

;; A tree item with secondary text, unless there is none.
(def with-detail (item detail) (if detail (merge item :detail detail) item))

(def layers ()
  (tree :key "lab-layers" :width :fill :font-size 11
        :items (layer-items)
        :selected-path (if (clamp-selected?) "clamp"
                         (if (point-selected?) (str "point" (source lab-named)) (str lab-part)))
        :selection-follows-external true
        :on-select (lambda (item)
                     (let ((path (get item :path))
                           (point (filter (lambda (k) (= (str "point" (source k)) path)) (keys lab-points)))
                           (hit (filter (lambda (i) (= (str i) path)) (range (len (layer-parts))))))
                       (if (not (empty? point)) (select-point! (first point))
                         (select-layer! (if (= path "clamp") clamp-layer (if (empty? hit) 0 (first hit)))))))))

(def repeat-counts (list 2 3 4 5 6 8))
(def repeat-options (cons "Repeat…" (map (lambda (n) (str n " in a row")) repeat-counts)))

;; The selected part's actions: Duplicate, Repeat, Delete.
(def part-actions ()
  (h-stack :gap 0.3 :align :center :width :fill
    (small-button "lab-part-copy" "Duplicate" copy-part)
    (dropdown :key "lab-part-repeat" :width 8 :height 1.3 :font-size 9.5 :value "Repeat…" :options repeat-options
              :on-change |choice| (let ((picked (filter (lambda (n) (= choice (str n " in a row"))) repeat-counts)))
                                    (when (not (empty? picked)) (make-row (first picked)))))
    (box :flex 1)
    (small-button "lab-part-delete" "Delete" delete-part)))

(def effect-options (list "Adds to the body" "Cuts into the body"))

;; Whether the part adds material to the body or cuts it away (a union or
;; a difference in the script).
(def part-effect (p)
  (v-stack :gap 0.2 :width :fill
    (section-label "EFFECT")
    (dropdown :key "lab-part-effect" :width :fill :height 1.3 :font-size 9.5
              :value (nth effect-options (if (get p :cut) 1 0)) :options effect-options
              :on-change |choice| (when (not (= (= choice (nth effect-options 1)) (get p :cut))) (toggle-cut)))))

(def clamp-properties ()
  (if (= (held-by) nil)
    (small-button "lab-clamp-add" "Add a clamp" clamp-it)
    (v-stack :gap 0.4 :width :fill
      (h-stack :gap 0.4 :align :center
        (if (clamp-misses?) (small-button "lab-clamp-snap" "Move onto shape" clamp-to-shape) (label "" :bg :transparent))
        (small-button "lab-clamp-free" "Remove" free-it))
      (if (= (kind-of (held-by)) :sphere)
        (mm-field "lab-clamp-radius" "Size" (get (sp/body (held-by)) :radius) 0.0005 0.2
                 (lambda (v) (set-clamp! (dict :sphere (merge (sp/body (held-by)) :radius (snap v))))))
        (label "" :bg :transparent))
      (section-label "POSITION")
      (position-knobs (sp/region-center (held-by)) (lambda (c) (set-clamp! (sp/move-region (held-by) c)))))))

(def part-properties ()
  (let ((p (selected-part))
        (s (get p :shape)))
    (v-stack :gap 0.5 :width :fill
      (part-actions)
      (part-effect p)
      ;; the profile editor reads the shape's own :revolve, not a body under cuts
      (if (and (= (kind-of s) :revolve) (= (kind-of (shape)) :revolve))
        (small-button "lab-edit-profile" (if (= lab-editor :profile) "Done editing profile" "Edit profile")
                      (lambda () (set-editor! (if (= lab-editor :profile) :parts :profile))))
        (label "" :height 0.1 :bg :transparent))
      (if (= (sp/bounds-of s) nil)
        (label "Edited in the script." :font-size 10 :color :dim :bg :transparent)
        (v-stack :gap 0.3
          (section-label "SIZE")
          (part-size-knobs s)
          (section-label "POSITION")
          (position-knobs (sp/center-of s)
                          (lambda (c) (set-selected! (merge (selected-part) :shape (sp/with-center s c)) true))))))))

;; A named point: its name (Enter renames it), where it is, and how a script
;; uses it.
(def point-properties ()
  (let ((xyz (get lab-points lab-named)) (name (bare-name lab-named)))
    (v-stack :gap 0.4 :width :fill
      (h-stack :gap 0.4 :align :center :width :fill
        (section-label "NAME")
        (text-input :key "lab-point-name" :width 10 :height 1.3 :font-size 10
                    :value lab-point-draft
                    :on-change (lambda (value) (set! lab-point-draft value))
                    :on-submit (lambda () (rename-point))
                    :on-cancel (lambda () (set! lab-point-draft name)))
        (box :flex 1)
        (small-button "lab-point-snap" "Snap" snap-point)
        (small-button "lab-point-delete" "Delete" delete-point))
      (if (and (not (= lab-point-draft name)) (= (name-keyword lab-point-draft) nil))
        (label "A name is letters, digits and -." :font-size 9.5 :color :yellow :bg :transparent)
        (label "" :height 0.1 :bg :transparent))
      (section-label "POSITION")
      (position-knobs xyz (lambda (c) (set-point! lab-named c)))
      (label (str "In the script: (m/listen '" (get (current-solid) :id) " :" name ")")
             :font-size 9.5 :color :dim :bg :transparent :width :fill))))

(def layers-panel ()
  (v-stack :gap 0.5 :width 25
    (box :width :fill :corner-radius 6 :padding 0.3 :background-color :buffer-bg
      (layers))
    (if (clamp-selected?) (clamp-properties)
      (if (point-selected?) (point-properties)
        (if (= (selected-part) nil) (label "" :bg :transparent) (part-properties))))))

;; ── The canvas ──

(def views (list (list :top "Top") (list :front "Front") (list :side "Side")))

;; (tool label colour): what a click on the parts canvas does, and the
;; colour its button fills with while armed (a cut's red, a solid's bronze),
;; so an armed tool is never mistaken for Move. A placing tool places one
;; part, then hands back to Move.
(def tools
  (list (list :select "Move" nil) (list :hole "Hole" cut-swatch) (list :slot "Slot" cut-swatch)
        (list :block "Block" solid-swatch) (list :arch "Arch" cut-swatch) (list :point "Point" point-swatch)))

(def canvas-toolbar ()
  (h-stack :gap 0.4 :align :center :width canvas-cols
    (if (parts-mode?)
      (dropdown :key "lab-view-plane" :width 8 :height 1.3 :font-size 9.5
                :value (nth (first (filter (lambda (v) (= (first v) lab-plane)) views)) 1)
                :options (map (lambda (v) (nth v 1)) views)
                :on-change |name| (set-plane! (first (first (filter (lambda (v) (= (nth v 1) name)) views)))))
      (label "Profile" :key "lab-view-profile" :font-size 10 :bg :transparent))
    (box :flex 1)
    (if (and (parts-mode?) (> (or (get lab-view :ex) 1) 1))
      (label (str "height ×" (get lab-view :ex)) :key "lab-stretch" :font-size 9.5 :color :dim :bg :transparent)
      (label "" :bg :transparent))))

(def tool-row ()
  (h-stack :gap 0.3 :align :center :width canvas-cols
    (each tools |t|
      (box :width 4.7
        (choice-button (str "lab-tool-" (nth t 1)) (nth t 1) (= lab-tool (first t)) (or (nth t 2) "#2f6fe0")
                       (lambda () (set! lab-tool (if (= lab-tool (first t)) :select (first t)))))))))

(def set-editor! (e)
  (set! lab-editor e)
  (fit-view)
  (rebuild-canvas))

(def canvas-panel ()
  (v-stack :gap 0.5 :width (+ canvas-cols 1)
    (canvas-toolbar)
    (box :width canvas-cols :height canvas-rows :corner-radius 6
      (if (parts-mode?)
        (parts-face :key "lab-parts" :geometry lab-canvas :width canvas-cols :height canvas-rows
                    :background view-background
                    :cr (get lab-view :cr) :cz (get lab-view :cz) :hr (get lab-view :hr) :hz (get lab-view :hz)
                    :on-mouse-down parts-down :on-drag parts-drag :on-mouse-up parts-up)
        (profile-face :key "lab-profile" :geometry lab-canvas :width canvas-cols :height canvas-rows
                      :background view-background
                      :cr (get lab-view :cr) :cz (get lab-view :cz) :hr (get lab-view :hr) :hz (get lab-view :hz)
                      :on-mouse-down canvas-down :on-drag canvas-drag :on-mouse-up canvas-up)))
    (if (parts-mode?) (tool-row) (revolve-controls))))

;; ── The instrument ──
;; Every solid where it sits, drawn together: each solid at its :place (as
;; moved here), or seated by its glue (a spring from one of its named
;; points to a placed solid's puts the two points together), else at the
;; origin. Strings run between their two attachments, lifted off the
;; surface; pickups sit under them, the pluck over them; named points and
;; mics are markers. Only the solid being edited is written back.

(def deg 0.0174532925199433)

;; An id (a symbol from the patch, a string from the host) as a string.
(def id-name (x) (if (string? x) x (source x)))

(def place-of (id)
  (let ((hit (filter (lambda (p) (= (first p) (id-name id))) lab-places)))
    (if (empty? hit) nil (nth (first hit) 1))))

(def set-place! (id place)
  (set! lab-places (map (lambda (p) (if (= (first p) (id-name id)) (list (first p) place) p)) lab-places))
  (when (= (id-name id) (get (current-solid) :id)) (set! lab-edited true))
  (rebuild-assembly))

(def solid-named (id)
  (let ((hit (filter (lambda (s) (= (get s :id) (id-name id))) lab-solids)))
    (if (empty? hit) nil (first hit))))

;; A solid's named points: the edited ones for the solid being edited.
(def points-of (id)
  (if (= (id-name id) (get (current-solid) :id)) lab-points
    (let ((pts (get (solid-named id) :points))) (if (map? pts) pts (dict)))))

;; p turned rx, ry then rz degrees about x, y and z (turn a list of three).
(def turn-point (turn p)
  (let ((ax (* deg (nth turn 0))) (ay (* deg (nth turn 1))) (az (* deg (nth turn 2)))
        (x (nth p 0)) (y (nth p 1)) (z (nth p 2))
        (y1 (- (* y (cos ax)) (* z (sin ax)))) (z1 (+ (* y (sin ax)) (* z (cos ax))))
        (x2 (+ (* x (cos ay)) (* z1 (sin ay)))) (z2 (- (* z1 (cos ay)) (* x (sin ay)))))
    (list (- (* x2 (cos az)) (* y1 (sin az))) (+ (* x2 (sin az)) (* y1 (cos az))) z2)))

(def add3 (a b) (map (lambda (i) (+ (nth a i) (nth b i))) (range 3)))
(def sub3 (a b) (map (lambda (i) (- (nth a i) (nth b i))) (range 3)))
(def scale3 (a k) (map (lambda (x) (* x k)) a))
(def lerp3 (a b t) (add3 a (scale3 (sub3 b a) t)))
(def len3 (a) (sqrt (+ (* (nth a 0) (nth a 0)) (* (nth a 1) (nth a 1)) (* (nth a 2) (nth a 2)))))
(def zero3 (list 0 0 0))

;; A connection or listen position on a solid as a local (x y z): a named
;; point, or a position; nil for an index.
(def local-at (id at)
  (if (list? at) at
    (if (= at nil) nil
      (if (number? at) nil (get (points-of id) at)))))

;; place: (dict :at :turn), both filled in.
(def full-place (p)
  (dict :at (or (if (map? p) (get p :at) nil) zero3) :turn (or (if (map? p) (get p :turn) nil) zero3)))

(def world-at (place local) (add3 (get place :at) (turn-point (get place :turn) local)))

(def wiring (key) (if (map? lab-wiring) (or (get lab-wiring key) (list)) (list)))
(def connections () (wiring :connections))
(def listens () (wiring :listens))
(def string-ids () (map id-name (wiring :strings)))

(def solid-id? (x) (not (= (solid-named x) nil)))

;; The springs between named points of two solids: (from-id from-at to-id
;; to-at), both ways round.
(def glue-pairs ()
  (apply append
    (map (lambda (c)
           (if (and (= (get c :law) :spring) (solid-id? (get c :a)) (solid-id? (get c :b)))
             (list (list (id-name (get c :a)) (get c :a-at) (id-name (get c :b)) (get c :b-at))
                   (list (id-name (get c :b)) (get c :b-at) (id-name (get c :a)) (get c :a-at)))
             (list)))
         (connections))))

;; ((id (dict :at :turn :how)) …) for every solid; :how is :placed (its
;; place has an :at), :seated (with :on, the solid it is glued to; a turn
;; it was given stays) or :origin.
(def resolve-places ()
  (let ((given (filter (lambda (s) (let ((p (place-of (get s :id)))) (and (map? p) (get p :at)))) lab-solids))
        (start (if (and (empty? given) (not (empty? lab-solids)))
                 (list (list (get (first lab-solids) :id) (merge (full-place nil) :how :origin)))
                 (map (lambda (s) (list (get s :id) (merge (full-place (place-of (get s :id))) :how :placed))) given))))
    (seat-all start (len lab-solids))))

(def placed-in (placed id)
  (let ((hit (filter (lambda (p) (= (first p) id)) placed))) (if (empty? hit) nil (nth (first hit) 1))))

(def seat-all (placed rounds)
  (let ((pairs (glue-pairs))
        (more (apply append
                (map (lambda (s)
                       (let ((id (get s :id)))
                         (if (placed-in placed id) (list)
                           (let ((glue (filter (lambda (g) (and (= (first g) id) (placed-in placed (nth g 2))
                                                                (local-at id (nth g 1)) (local-at (nth g 2) (nth g 3))))
                                               pairs)))
                             (if (empty? glue) (list)
                               (let ((g (first glue))
                                     (turn (get (full-place (place-of id)) :turn))
                                     (there (world-at (placed-in placed (nth g 2)) (local-at (nth g 2) (nth g 3)))))
                                 (list (list id (dict :at (sub3 there (turn-point turn (local-at id (nth g 1))))
                                                      :turn turn :how :seated :on (nth g 2))))))))))
                     lab-solids))))
    (if (or (empty? more) (< rounds 1))
      (append placed (map (lambda (s) (list (get s :id) (merge (full-place nil) :how :origin)))
                          (filter (lambda (s) (= (placed-in placed (get s :id)) nil)) lab-solids)))
      ;; one seat per round, so a chain seats in order
      (seat-all (append placed (list (first more))) (- rounds 1)))))

;; Tints (assembly-face's palette, r g b per tint).
(def palette
  (list 0.70 0.72 0.76   0.88 0.66 0.40   0.56 0.27 0.16   0.93 0.83 0.62   0.32 0.18 0.13
        0.74 0.76 0.80   0.33 0.60 1.00   0.62 0.92 1.00   0.88 0.88 0.84   0.13 0.13 0.14
        0.96 0.56 0.16   0.36 0.86 0.52   0.86 0.69 0.30))
(def tint-point 6)
(def tint-picked 7)
(def tint-string 8)
(def tint-pickup 9)
(def tint-pick 10)
(def tint-mic 11)
(def tint-metal 5)

(def material-tints
  (list (list "maple" 1) (list "mahogany" 2) (list "spruce" 3) (list "rosewood" 4)
        (list "brass" 12) (list "bronze" 12) (list "copper" 12) (list "gold" 12)))

(def material-tint (name)
  (let ((hit (filter (lambda (t) (= (first t) name)) material-tints)))
    (if (empty? hit) tint-metal (nth (first hit) 1))))

(def solid-material (s)
  (if (= (get s :id) (get (current-solid) :id)) (get (get lab-spec :made-of) :name)
    (get (get (get s :spec) :made-of) :name)))

;; How far strings sit off the surface they are mounted on (m).
(def string-lift 0.007)

;; Where along string sid each attachment is, (fraction world-point).
(def string-ends (sid places)
  (let ((on (lambda (id at) (let ((pl (placed-in places (id-name id))) (local (local-at id at)))
                                  (if (and pl local) (world-at pl local) nil)))))
    (filter (lambda (e) (not (= (nth e 1) nil)))
      (apply append
        (map (lambda (c)
               (let ((law (get c :law)) (a (id-name (get c :a))) (b (id-name (get c :b))))
                 (if (and (= law :spring) (= a sid) (number? (get c :a-at)) (solid-id? b))
                   (list (list (get c :a-at) (on b (get c :b-at))))
                   (if (and (= law :spring) (= b sid) (number? (get c :b-at)) (solid-id? a))
                     (list (list (get c :b-at) (on a (get c :a-at))))
                     (if (and (= law :mount) (= b sid) (solid-id? a))
                       (list (list (get c :end) (on a (get c :a-at))))
                       (list))))))
             (connections))))))

;; String sid as (from-fraction from-point to-fraction to-point), its two
;; outermost attachments, lifted; nil unless it has two.
(def string-line (sid places)
  (let ((ends (string-ends sid places)))
    (if (< (len ends) 2) nil
      (let ((lo (reduce (lambda (b e) (if (< (first e) (first b)) e b)) (first ends) ends))
            (hi (reduce (lambda (b e) (if (> (first e) (first b)) e b)) (first ends) ends))
            (up (list 0 0 string-lift)))
        (if (= (first lo) (first hi)) nil
          (list (first lo) (add3 (nth lo 1) up) (first hi) (add3 (nth hi 1) up)))))))

(def along (line s)
  (lerp3 (nth line 1) (nth line 3) (/ (- s (nth line 0)) (- (nth line 2) (nth line 0)))))

;; A unit vector across the string, level.
(def across (line)
  (let ((d (sub3 (nth line 3) (nth line 1)))
        (flat (sqrt (+ (* (nth d 0) (nth d 0)) (* (nth d 1) (nth d 1))))))
    (if (< flat 0.000001) (list 0 1 0) (list (/ (- 0 (nth d 1)) flat) (/ (nth d 0) flat) 0))))

(def rod (from to radius tint) (dict :from from :to to :radius radius :tint tint))
(def mark (at size tint) (dict :at at :size size :tint tint))

;; The parts, rods and marks of the whole instrument.
(def assembly-parts (places)
  (filter (lambda (p) (not (= p nil)))
    (map (lambda (s)
           (let ((pl (placed-in places (get s :id)))
                 (editing (= (get s :id) (get (current-solid) :id)))
                 (tint (+ (material-tint (solid-material s)) (if editing 0.5 0)))
                 (src (if editing (if (solved?) (get lab-solved :file) nil) (get s :mesh-file)))
                 (where (dict :at (get pl :at) :turn (get pl :turn) :tint tint)))
             (if (string? src) (merge where :src src)
               (if (and editing (map? lab-preview) (get lab-preview :mesh-id))
                 (merge where :mesh-id (get lab-preview :mesh-id))
                 (if (and (not editing) (map? (get s :spec)))
                   (merge where :spec (get s :spec) :dir lab-dir)
                   nil)))))
         lab-solids)))

(def assembly-rods (places)
  (apply append
    (map (lambda (sid)
           (let ((line (string-line sid places)))
             (if (= line nil) (list)
               (append
                 (list (rod (nth line 1) (nth line 3) 0.0009 tint-string)
                       ;; the saddle and nut under the lifted ends
                       (rod (sub3 (nth line 1) (list 0 0 string-lift)) (nth line 1) 0.0022 tint-metal)
                       (rod (sub3 (nth line 3) (list 0 0 string-lift)) (nth line 3) 0.0022 tint-metal))
                 ;; a pickup is a bar across the string, below it
                 (map (lambda (l)
                        (let ((at (add3 (along line (get l :at)) (list 0 0 -0.0045))) (w (scale3 (across line) 0.022)))
                          (rod (sub3 at w) (add3 at w) 0.0042 (if (= (get l :hear) :pickup) tint-pickup tint-mic))))
                      (filter (lambda (l) (and (= (id-name (get l :obj)) sid) (number? (get l :at)))) (listens)))))))
         (string-ids))))

(def assembly-marks (places)
  (append
    ;; every solid's named points
    (apply append
      (map (lambda (s)
             (let ((pl (placed-in places (get s :id))) (pts (points-of (get s :id)))
                   (editing (= (get s :id) (get (current-solid) :id))))
               (map (lambda (k) (mark (world-at pl (get pts k)) 0.0045
                                      (if (and editing (= k lab-named)) tint-picked tint-point)))
                    (keys pts))))
           lab-solids))
    ;; mics on solids
    (apply append
      (map (lambda (l) (let ((id (id-name (get l :obj))) (pl (placed-in places (id-name (get l :obj)))))
                         (if (and pl (local-at id (get l :at))) (list (mark (world-at pl (local-at id (get l :at))) 0.008 tint-mic)) (list))))
           (listens)))
    ;; plucks over strings, strikes on solids
    (apply append
      (map (lambda (c)
             (let ((b (id-name (get c :b))) (pl (placed-in places (id-name (get c :b)))))
               (if (and (= (get c :law) :pluck) (number? (get c :b-at)) (string-line b places))
                 (let ((line (string-line b places)))
                   (list (mark (add3 (along line (get c :b-at)) (list 0 0 0.006)) 0.006 tint-pick)))
                 (if (and (= (get c :law) :felt) pl (local-at b (get c :b-at)))
                   (list (mark (world-at pl (local-at b (get c :b-at))) 0.009 tint-pick))
                   (list)))))
           (connections)))))

;; Rebuild the instrument's mesh (only while it is shown).
(def rebuild-assembly ()
  (when (= lab-look :instrument)
    (let ((places (resolve-places)))
      (set! lab-assembly (modal-assembly-mesh (assembly-parts places) (assembly-rods places) (assembly-marks places))))))

(def set-look! (look)
  (set! lab-look look)
  (rebuild-assembly))

;; The solid being edited's place, filled in, as resolved.
(def current-place ()
  (placed-in (resolve-places) (get (current-solid) :id)))

;; Move the solid being edited: key :at or :turn, axis i, to v. Moving a
;; seated solid places it where it is seated, then moves it.
(def nudge-place! (key i v)
  (let ((pl (current-place)) (now (if (= pl nil) zero3 (get pl key))))
    (set-place! (get (current-solid) :id)
                (merge (dict :at (get pl :at) :turn (get pl :turn)) key (set-nth now i v)))))

;; Seat the solid being edited by its glue again: drop its :at (a turn it
;; was given stays).
(def reseat ()
  (let ((p (place-of (get (current-solid) :id))))
    (set-place! (get (current-solid) :id)
                (if (and (map? p) (get p :turn) (not (= (get p :turn) zero3))) (dict :turn (get p :turn)) nil))))

(def place-summary (pl)
  (match (get pl :how)
    :seated (str "Seated on " (get pl :on) " by its glue")
    :placed "Placed"
    _ "At the origin"))

(def degree-field (key title value on-set)
  (v-stack :width 5.6 :gap 0.08
    (label title :font-size 8.5 :color :dim :bg :transparent)
    (number-picker :key key :value value :min -180 :max 180
                   :drag :relative :drag-step 0.5 :step 0.5 :decimals 1 :unit "°"
                   :noui true :font-size 10.2 :text-color :accent :edit-color :yellow :text-align :left
                   :width 5.6 :height 0.9 :on-change (lambda (v) (on-set (clamp v -180 180))))))

;; Under the instrument view: where the solid being edited sits.
(def place-controls ()
  (let ((pl (current-place)))
    (if (= pl nil) (label "" :bg :transparent)
      (v-stack :gap 0.3 :width :fill
        (h-stack :gap 0.5 :align :center :width :fill
          (section-label (str "PLACE · " (get (current-solid) :id)))
          (label (place-summary pl) :key "lab-place-how" :font-size 9.5 :color :dim :bg :transparent)
          (box :flex 1)
          (if (map? (place-of (get (current-solid) :id)))
            (small-button "lab-reseat" (if (empty? (glue-pairs)) "Origin" "Seat by glue") reseat)
            (label "" :bg :transparent)))
        (h-stack :gap 0.5
          (each (range 3) |i|
            (mm-field (str "lab-place-at-" i) (nth (list "X" "Y" "Z") i) (nth (get pl :at) i) -2 2
                      (lambda (v) (nudge-place! :at i (snap v)))))
          (each (range 3) |i|
            (degree-field (str "lab-place-turn-" i) (nth (list "Turn X" "Turn Y" "Turn Z") i) (nth (get pl :turn) i)
                          (lambda (v) (nudge-place! :turn i v)))))))))

;; ── 3-D view ──

(def view-background "#1b1d22")

(def solved? () (and (map? lab-solved) (string? (get lab-solved :file))))

(def view-rows (+ canvas-rows 2))

;; The solved shape swings through its lowest modes; until then, the shape
;; as meshed. In the instrument look, every part where it sits.
(def view-3d ()
  (box :width :fill :height view-rows :corner-radius 6
    (if (= lab-look :instrument)
      (if (and (map? lab-assembly) (get lab-assembly :mesh-id))
        (mv/assembly-view :key "lab-assembly" :geometry lab-assembly :palette palette :zoom lab-zoom
                          :width :fill :height view-rows :yaw -0.9 :pitch 0.75 :background view-background)
        (label (str "  " (if (map? lab-assembly) (or (get lab-assembly :error) "") "")) :font-size 10 :color :dim :bg :transparent))
    (if (solved?)
      (mv/mode-mesh-cycle :key "lab-cycle" :src (get lab-solved :file) :width :fill :height view-rows
                          :background view-background)
      (if (and (map? lab-preview) (not (= (get lab-preview :mesh-id) nil)))
        (mv/mode-mesh-view :key "lab-preview" :geometry lab-preview :amps (list 0)
                           :width :fill :height view-rows :yaw 0.6 :pitch 0.55 :background view-background)
        (label "" :bg :transparent))))))

(def status-text ()
  (let ((p lab-preview))
    (if (and (map? p) (get p :error)) (str "Shape: " (get p :error))
      (if (map? p)
        (str (fmt "{:.3}" (get p :mass)) " kg · "
             (if (solved?) (str "lowest mode " (fmt "{:.1}" (first (get lab-solved :freqs))) " Hz")
               (if (= lab-solved nil) "solving…" (get lab-solved :error))))
        ""))))

(def progress-cols 14)

;; While solving: a bar, the phase and percent, and Cancel. After a cancel:
;; Solve. Otherwise the solve's summary.
(def solve-status ()
  (if (and (= lab-solved nil) lab-solve-progress)
    (let ((fraction (nth lab-solve-progress 1)))
      (h-stack :gap 0.5 :align :center
        (h-stack :key "lab-solve-bar" :width progress-cols :height 0.5 :gap 0
          (box :width (max 0.2 (* progress-cols fraction)) :height 0.5 :corner-radius 3 :background-color :accent)
          (box :width (max 0 (* progress-cols (- 1 fraction))) :height 0.5 :corner-radius 3 :background-color :buffer-bg))
        ;; a fixed width (the widest phase at 99%), so Cancel stays put
        (label (str (first lab-solve-progress) " " (floor (* 100 fraction)) "%") :key "lab-solve-phase"
               :width 12 :font-size 10 :color :dim :bg :transparent)
        (small-button "lab-solve-cancel" "Cancel" cancel-solve)))
    (if (and (map? lab-solved) (get lab-solved :cancelled))
      (h-stack :gap 0.5 :align :center
        (label "Solve cancelled." :key "lab-status" :font-size 10 :color :dim :bg :transparent)
        (small-button "lab-solve-again" "Solve" request-solve))
      (label (status-text) :key "lab-status" :font-size 10 :color :dim :bg :transparent))))

(def looks (list (list :solid "This solid") (list :instrument "Instrument")))

(def look-toggle ()
  (h-stack :gap 0.3 :align :center :width :fill
    (each looks |l|
      (choice-button (str "lab-look-" (nth l 1)) (nth l 1) (= lab-look (first l)) "#2f6fe0"
                     (lambda () (set-look! (first l)))))
    (box :flex 1)
    (if (= lab-look :instrument)
      (h-stack :gap 0.3 :align :center
        (label "Zoom" :font-size 10 :color :dim :bg :transparent)
        (narrow-button "lab-zoom-out" "−" (lambda () (set! lab-zoom (max 0.5 (/ lab-zoom 1.25)))))
        (narrow-button "lab-zoom-in" "+" (lambda () (set! lab-zoom (min 8 (* lab-zoom 1.25))))))
      (label "" :bg :transparent))))

(def view-panel ()
  (v-stack :flex 1 :gap 0.4
    (look-toggle)
    (view-3d)
    (h-stack :gap 0.5 :align :center :width :fill
      (solve-status)
      (box :flex 1)
      (label "Mesh" :font-size 10 :color :dim :bg :transparent)
      (narrow-button "lab-coarser" "−" (lambda () (step-detail -1)))
      (label (fmt "{:.1} mm" (* 1000 (cell-size))) :key "lab-detail" :font-size 10 :bg :transparent)
      (narrow-button "lab-finer" "+" (lambda () (step-detail 1))))
    (if (parts-mode?)
      (label (detail-warning) :key "lab-detail-warning" :font-size 10 :color :yellow :bg :transparent :width :fill)
      (label "" :bg :transparent))
    (if (= lab-look :instrument) (place-controls) (label "" :bg :transparent))))

;; ── Header ──

(def solid-tabs ()
  (h-stack :gap 0.4
    (each lab-solids |s i|
      (button (str (get s :id)) :key (str "lab-solid-" i) :height 1.3 :font-size 9.5 :shape :tab
              :active (= i lab-index) :on-click (lambda (e) (switch-solid i))))))

;; Replaces the shape with a template's.
(def template-menu ()
  (dropdown :key "lab-template" :width 11 :height 1.3 :font-size 9.5 :value "New shape…"
            :options (cons "New shape…" (map first templates))
            :on-change |name| (let ((picked (filter (lambda (t) (= (first t) name)) templates)))
                                (when (not (empty? picked)) (use-template (first picked))))))

(def header ()
  (h-stack :width :fill :gap 0.8 :align :center
    (label (str "SHAPE LAB · " lab-buffer) :font-size 11 :bg :transparent)
    (solid-tabs)
    (if (map? lab-spec) (template-menu) (label "" :bg :transparent))
    (box :flex 1)
    (if (map? lab-spec) (spec-controls) (label "" :bg :transparent))
    (button "Reload" :key "lab-reload" :height 1.3 :font-size 9.5 :on-click (lambda (e) (shape-lab)))
    (apply-button)))

(def spec-controls ()
  (h-stack :gap 0.8 :align :center
    (dropdown :key "lab-material" :width 11 :height 1.3 :font-size 9.5
              :value (get (get lab-spec :made-of) :name)
              :options (map (lambda (m) (get m :name)) materials)
              :on-change |name| (set-material! name))
    (label "Modes" :font-size 10 :color :dim :bg :transparent)
    (narrow-button "lab-fewer-modes" "−" (lambda () (set-modes! (max 4 (- (get lab-spec :modes) 8)))))
    (label (str (get lab-spec :modes)) :key "lab-modes" :font-size 11 :bg :transparent)
    (narrow-button "lab-more-modes" "+" (lambda () (set-modes! (min 160 (+ (get lab-spec :modes) 8)))))))

;; One click: writes the script, evaluates it and recompiles its draft; the
;; label follows (lab-draft). It waits for the solve, so the script's
;; evaluation reads it from the cache instead of solving on the UI thread.
(def apply-button ()
  (button (if (= lab-draft :compiling) "Compiling draft…"
            (if (not lab-edited) (if (= lab-draft :done) "Draft updated" "Draft is current")
              (if (= lab-solved nil) "Solving…"
                (if (solved?) "Update draft" (if (get lab-solved :cancelled) "Not solved" "Can't solve")))))
          :key "lab-apply" :height 1.3 :font-size 9.5 :variant :primary
          :disabled (not (and lab-edited (editable?) (solved?) (not (= lab-draft :compiling))))
          :on-click (lambda (e) (update-draft))))

;; The host's progress on an Update draft: :compiling, :done or :failed
;; (with the error).
(def draft-status (status error)
  (set! lab-draft status)
  (set! lab-draft-error (if (= status :failed) error nil))
  (when (= status :failed) (set! lab-edited true)))

(def root-widget ()
   (v-stack :width :fill :gap 0.6 :padding 0.8
    (if (= lab-spec nil)
      (label (if (= lab-buffer "") "M-x Shape Lab in a modal script's buffer opens its solids here."
               (str lab-buffer " has no m/solid to design."))
             :font-size 11 :color :dim :bg :transparent)
      (v-stack :width :fill :gap 0.6
        (header)
        (if lab-draft-error
          (label (str "Draft: " lab-draft-error) :key "lab-draft-error" :font-size 10 :color :red :bg :transparent :width :fill)
          (label "" :height 0.1 :bg :transparent))
        (if (not (map? lab-spec))
          (label (str "This solid's spec is the file " lab-spec "; edit it there.") :font-size 10 :color :dim :bg :transparent)
          (h-stack :width :fill :gap 1.2 :align :start
            (layers-panel)
            (canvas-panel)
            (view-panel)))))))

(effect-buffer "*shape-lab*"
  (root-widget))
