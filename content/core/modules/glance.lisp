;; eseq.glance — each sequencer kind's small live view (docs/rig-spec.md §7
;; "Glances").
;;
;; A kind registers its glance once, by kind id:
;;
;;   (import eseq.glance)
;;   (eseq.glance/def-glance "alez/neural:neural" (lambda (self hint) (my-glance self hint)))
;;
;; and any view shows any instance's glance without knowing its kind:
;;
;;   (eseq.glance/glance-for inst (dict :height 4 :color g.color))
;;
;; A renderer is `(fn self hint) -> widget`: `self` the live instance (the
;; value a kind's :view receives: it.id, it.kind, it.label), `hint` a
;; dict a view fills with what it knows, every key optional:
;;   :height  rows the glance may use (default 4)
;;   :color   an :rgb to tint it with (a kit's colour), or nil for the theme
;;   :key     a prefix for its widget keys, so two views of one instance
;;            keep apart (default "glance")
;; Keep a renderer cheap: it runs whenever what it reads moves, so wrap the
;; live reads in their own `subtree`s.
;;
;; An instance whose kind registered none gets `fallback-glance` (its kind's
;; icon and its name), so callers never special-case a missing glance.
;;
;; A kind id is "package:kind" ("alez/neural:neural"); a bare kind name
;; ("neural") also matches any package's kind of that name, the exact id
;; winning.
;;
;; Register a trampoline, `(lambda (self hint) (my-glance self hint))`, not
;; the function itself: re-evaluating the kind's file then restyles every
;; glance at once (a stored function value keeps drawing the old body).

(module eseq.glance)

(export def-glance glance-renderer has-glance? glance-for glance-card
        fallback-glance glance-hint)

;; ((kind renderer) …). A `defstate`, not a `def`: re-evaluating this file
;; (C-c C-c, hot reload) keeps the registrations the packages made, and a
;; registration re-renders whatever asked for a glance before it existed.
(defstate glances (list))

;; Register `renderer` as kind `kind`'s glance (replacing an earlier one);
;; a nil `renderer` removes it, so the kind falls back.
(def def-glance (kind renderer)
  (do
    (set! glances
      (append (filter (lambda (e) (not (= (nth e 0) kind))) glances)
              (if renderer (list (list kind renderer)) (list))))
    kind))

;; True when entry name `name` names kind id `kind`: the id itself, or its
;; bare kind name (what follows the package's ":").
(def names-kind? (name kind)
  (if (= name kind)
    true
    (let ((n (len kind)) (m (+ (len name) 1)))
      (if (> n m)
        (= (substring kind (- n m) n) (str ":" name))
        false))))

;; The renderer registered for kind id `kind`, or nil. An exact id beats a
;; bare name.
(def glance-renderer (kind)
  (if (string? kind)
    (let ((exact (filter (lambda (e) (= (nth e 0) kind)) glances)))
      (if (empty? exact)
        (let ((bare (filter (lambda (e) (names-kind? (nth e 0) kind)) glances)))
          (if (empty? bare) nil (nth (first bare) 1)))
        (nth (first exact) 1)))
    nil))

;; `x` as a live instance: an instance passes through, an id (a
;; `project.instances` row's :id) is looked up; nil when it is gone.
(def as-instance (x)
  (if (number? x) (instance-ref x) x))

;; True when `inst`'s kind registered a glance.
(def has-glance? (inst)
  (let ((it (as-instance inst)))
    (if it (if (glance-renderer it.kind) true false) false)))

;; `hint` with its defaults filled in.
(def glance-hint (hint)
  (let ((h (if hint hint (dict))))
    (dict :height (if (get h :height) (get h :height) 4)
          :color (get h :color)
          :key (if (get h :key) (get h :key) "glance"))))

;; The glance of no glance: the kind's icon (the :icon its def-kind
;; declares, what its step tab draws) and the instance's name.
(def fallback-glance (inst hint)
  (let ((it (as-instance inst)))
    (if it
      (let ((h (glance-hint hint))
            (icon (instance-icon it.id)))
        (h-stack :key (str (get h :key) "-fallback-" it.id)
          :debug-name "glance-fallback" :gap 0.3 :align :center
          (if icon
            (box :width 1.4 :height 0.9 :corner-radius 10
              :background-color :mixer-label-bg
              :padding 0.12 :h-align :center :v-align :center
              (box :width 1.1 :height 0.62 :background icon :selected 0))
            (box :width 0 :height 0))
          (label (substring it.label 0 16) :font-size 10 :height 0.9
            :v-align :center :color :white :bg :transparent)))
      nil)))

;; `inst`'s glance: its kind's renderer, else the fallback. Nil only when
;; `inst` is no live instance.
(def glance-for (inst hint)
  (let ((it (as-instance inst)))
    (if it
      (let ((f (glance-renderer it.kind)))
        (if f (f it (glance-hint hint)) (fallback-glance it hint)))
      nil)))

;; `inst`'s glance under a caption (icon and name), for a view listing
;; several sequencers: the fallback alone when its kind has none (the
;; fallback is the caption).
(def glance-card (inst hint)
  (let ((it (as-instance inst)))
    (if it
      (let ((f (glance-renderer it.kind)) (h (glance-hint hint)))
        (if f
          (v-stack :key (str (get h :key) "-card-" it.id)
            :debug-name "glance-card" :gap 0.2 :width :fill
            (fallback-glance it h)
            (f it h))
          (fallback-glance it h)))
      nil)))
