;; ui/view-kit.lisp — Small helpers the factory views share.
;;
;; Side-effect free: no state, no buffers, no host calls at import, so any
;; view can import it (unlike ui/menus.lisp, which registers menus when
;; loaded). Context menus here follow the views' convention: a `:key ()`
;; singleton with `open` and `at` (where it opens), plus what it targets.

(module eseq.view-kit)

(export open-menu! menu-of menu-tree nothing listed? index-of find clamp without named prop-if
        rgb-part color-rgba dimmed dimmed-part track-color-part)

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

;; Menu items for a tree of rows (dicts, as the browser's trees are): a row
;; with :children opens a submenu of them, a :kind "header" row is a disabled
;; label, any other row calls (pick row) when chosen. Labels key the items,
;; so siblings with one label share a key.
(def menu-tree (rows pick &key (path ""))
  (map (lambda (row)
         (let ((label (get row :label))
               (key (str "menu-tree" path "/" label))
               (kids (get row :children)))
           (if (= (get row :kind) "header")
             (menu-item label :key key :disabled true)
             (if kids
               (apply menu-item label :key key (menu-tree kids pick :path (str path "/" label)))
               (menu-item label :key key :on-select (lambda (e) (pick row)))))))
       rows))

;; An empty slot where a widget shows only sometimes.
(def nothing () (box :width 0 :height 0))

;; Whether x is one of xs (instances compare by identity).
(def listed? (x xs) (reduce |found y| (or found (= y x)) false xs))

;; x held to lo..hi.
(def clamp (x lo hi) (min hi (max lo x)))

;; The first of xs that (pred x) holds for, or nil.
(def find (pred xs) (first (filter pred xs)))

;; A props list (:k v …) without the pairs keyed by any of `keys`: for a
;; function that takes some keys itself and hands the rest on.
(def without (props keys)
  (reduce |acc pair|
    (if (listed? (first pair) keys) acc (append acc pair))
    (list)
    (chunks props 2)))

;; The position of the first `value` in xs, or -1.
(def index-of (xs value)
  (reduce |found i|
    (if (>= found 0) found (if (= (nth xs i) value) i found))
    -1
    (range 0 (len xs))))

;; The first of xs (instances or anything with a `name` field) named
;; `name`, or nil.
(def named (xs name)
  (first (filter (lambda (x) (= x.name name)) xs)))

;; `(k v)` to splice into a widget's props when v is set, else nothing.
(def prop-if (k v) (if v (list k v) (list)))

;; A component (0 r, 1 g, 2 b) of an :rgb value `(rgb r g b)`.
(def rgb-part (c i) (nth c (+ i 1)))

;; Color c (an :rgb value) as an rgba with `alpha`.
(def color-rgba (c alpha)
  (rgba (rgb-part c 0) (rgb-part c 1) (rgb-part c 2) alpha))

;; Component value v (component i: 0 r, 1 g, 2 b) pulled toward a dark gray
;; while `dim` (a track or bus that is not heard draws its color so).
(def dimmed (v i dim)
  (if dim (+ (* v 0.34) (* (if (= i 2) 0.11 0.10) 0.66)) v))

;; Component i of color c (an :rgb value), dimmed while `dim`.
(def dimmed-part (c i dim) (dimmed (rgb-part c i) i dim))

;; Component i of track t's color, dimmed while `dim`; a stock blue's when
;; there is no t.
(def track-color-part (t i dim)
  (if t
    (dimmed-part t.color i dim)
    (dimmed (nth (list 0.34 0.48 0.98) i) i dim)))
