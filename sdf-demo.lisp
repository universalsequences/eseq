;; sdf-demo.lisp — SDF widget showcase
;;
;; Demonstrates defwidget: defining GPU-rendered widgets entirely in Lisp.
;; Each widget is a signed distance field compiled to a Metal fragment shader.

;; ── Define SDF widgets ────────────────────────────────────────────────

;; A simple filled circle
(defwidget sdf-dot
  :width 3 :height 3
  :shader (sdf/layer
             (sdf/fill (sdf/circle 0.7) :accent)))

;; A ring (circle outline via stroke)
(defwidget sdf-ring
  :width 4 :height 4
  :shader (sdf/layer
             (sdf/stroke (sdf/circle 0.7) 0.05 :accent)))

;; A rounded rectangle badge
(defwidget sdf-badge
  :width 8 :height 3
  :shader (sdf/layer
             (sdf/fill (sdf/rounded-rect 0.9 0.8 0.15) :accent)))

;; Bullseye: concentric circle + ring
(defwidget sdf-bullseye
  :width 5 :height 5
  :shader (sdf/layer
             (sdf/fill (sdf/circle 0.8) :dim)
             (sdf/stroke (sdf/circle 0.8) 0.03 :accent)
             (sdf/fill (sdf/circle 0.4) :accent)
             (sdf/stroke (sdf/circle 0.4) 0.03 :primary)))

;; Union of two shapes
(defwidget sdf-blob
  :width 8 :height 4
  :shader (sdf/layer
             (sdf/fill
               (sdf/smooth-union 0.3
                 (sdf/translate -0.3 0 (sdf/circle 0.5))
                 (sdf/translate 0.3 0 (sdf/circle 0.5)))
               :accent)))

;; Crosshair
(defwidget sdf-crosshair
  :width 5 :height 5
  :shader (sdf/layer
             (sdf/fill (sdf/circle 0.85) :dim)
             (sdf/paint (sdf/rect 0.02 0.8) :accent)
             (sdf/paint (sdf/rect 0.8 0.02) :accent)
             (sdf/stroke (sdf/circle 0.5) 0.02 :accent)))

;; ── Render the demo ───────────────────────────────────────────────────

(effect
  (v-stack :padding 1 :gap 1
    (label "SDF Widget Demo" :font-size 18 :color :accent)

    (h-stack :gap 2 :align :center
      (label "dot:" :color :dim)
      (sdf-dot)
      (label "ring:" :color :dim)
      (sdf-ring)
      (label "badge:" :color :dim)
      (sdf-badge))

    (h-stack :gap 2 :align :center
      (label "bullseye:" :color :dim)
      (sdf-bullseye)
      (label "blob:" :color :dim)
      (sdf-blob))

    (h-stack :gap 2 :align :center
      (label "crosshair:" :color :dim)
      (sdf-crosshair))

    (label "All shapes defined in Lisp, compiled to Metal shaders" :color :dim :font-size 10)))
