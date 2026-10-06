;; ui/view-kit.lisp — Small helpers the factory views share.
;;
;; Side-effect free: no state, no buffers, no host calls at import, so any
;; view can import it (unlike ui/menus.lisp, which registers menus when
;; loaded). Context menus here follow the views' convention: a `:key ()`
;; singleton with `open` and `at` (where it opens), plus what it targets.

(module eseq.view-kit)

(export open-menu! menu-of nothing listed? index-of prop-if rgb-part color-rgba)

;; m's context menu opens at the pointer event's grid point.
(def open-menu! (m event)
  (set! m.at event.at)
  (set! m.open true))

;; m's context menu holding `items`.
(def menu-of (m &rest items)
  (context-menu :is-open m.open
    :anchor m.at
    :on-close (lambda () (set! m.open false))
    items))

;; An empty slot where a widget shows only sometimes.
(def nothing () (box :width 0 :height 0))

;; Whether x is one of xs (instances compare by identity).
(def listed? (x xs) (reduce |found y| (or found (= y x)) false xs))

;; The position of the first `value` in xs, or -1.
(def index-of (xs value)
  (reduce |found i|
    (if (>= found 0) found (if (= (nth xs i) value) i found))
    -1
    (range 0 (len xs))))

;; `(k v)` to splice into a widget's props when v is set, else nothing.
(def prop-if (k v) (if v (list k v) (list)))

;; A component (0 r, 1 g, 2 b) of an :rgb value `(rgb r g b)`.
(def rgb-part (c i) (nth c (+ i 1)))

;; Color c (an :rgb value) as an rgba with `alpha`.
(def color-rgba (c alpha)
  (rgba (rgb-part c 0) (rgb-part c 1) (rgb-part c 2) alpha))
