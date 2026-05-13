;; sdf-material-demo.lisp — SDF material system showcase
;;
;; Demonstrates:
;; - legacy flat fills still working
;; - (material :color ...)
;; - rgba-based color construction
;; - implicit x, y, d inside material expressions
;; - soft shadows with :paint-margin

;; ── Widgets ─────────────────────────────────────────────────────────────

(defwidget mat-flat-dot
  :width 3 :height 3
  :shader
  (sdf/layer
    ;; Legacy syntax still works.
    (sdf/fill (sdf/circle 0.7) :accent)))

(defwidget mat-rgba-dot
  :width 3 :height 3
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.7)
      (material
        :color (rgba 1.0 0.75 0.2 1.0)))))

(defwidget mat-y-gradient
  :width 6 :height 3
  :shader
  (let ((shape (sdf/rounded-rect 0.9 0.65 0.18)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (mix :fg :bg
                      (smoothstep -1.0 1.0 y)))))))

(defwidget mat-distance-rim
  :width 5 :height 5
  :shader
  (let ((shape (sdf/circle 0.8)))
    (sdf/layer
      (sdf/fill shape
        (material
          ;; Use d to brighten near the edge.
          :color (mix :accent :white
                      (smoothstep -0.10 0.03 d)))))))

(defwidget mat-xy-mix
  :width 6 :height 4
  :shader
  (let ((shape (sdf/rounded-rect 0.92 0.75 0.16)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (rgba
                    (smoothstep -1.0 1.0 x)
                    (smoothstep -1.0 1.0 y)
                    1.0
                    (smoothstep 0.08 -0.08 d)))))))

(defwidget mat-shadow-dot
  :width 3 :height 3
  :paint-margin 1
  :shader
  (let ((shape (sdf/circle 0.62)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (mix :accent :white
                      (smoothstep -0.08 0.02 d))
          :shadow (shadow
                    :color (rgba 0 0 0 0.22)
                    :blur 0.20
                    :offset (vec2 0 0.06)))))))

(defwidget mat-shadow-badge
  :width 8 :height 3
  :paint-margin 1
  :shader
  (let ((shape (sdf/rounded-rect 0.92 0.6 0.16)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (mix :fg :accent
                      (smoothstep -1.0 1.0 y))
          :shadow (shadow
                    :color (rgba 0 0 0 0.18)
                    :blur 0.18
                    :offset (vec2 0 0.05)
                    :spread 0.01))))))

(defwidget mat-shadow-button
  :width 8 :height 3
  :paint-margin 1
  :shader
  (let ((shape (sdf/rounded-rect 0.9 0.7 0.18)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (if hit/active
                   :primary
                   (if hit/hover
                     (mix :accent :white (smoothstep -0.1 0.03 d))
                     (mix :dim :accent (smoothstep -0.2 0.0 d))))
          :shadow (shadow
                    :color (rgba 0 0 0 0.24)
                    :blur 0.22
                    :offset (vec2 0 0.07)))))))

;; ── Demo ───────────────────────────────────────────────────────────────

(effect
  (v-stack :padding 1 :gap 1
    (label "SDF Materials Demo" :font-size 18 :color :accent)
    (label "flat, rgba, gradients, implicit x/y/d, shadows, and paint overflow" :color :dim :font-size 10)

    (h-stack :gap 2 :align :center
      (label "legacy:" :color :dim)
      (mat-flat-dot)
      (label "rgba:" :color :dim)
      (mat-rgba-dot)
      (label "d rim:" :color :dim)
      (mat-distance-rim))

    (h-stack :gap 2 :align :center
      (label "y gradient:" :color :dim)
      (mat-y-gradient)
      (label "x/y + d:" :color :dim)
      (mat-xy-mix))

    (h-stack :gap 3 :align :center
      (label "shadow dot:" :color :dim)
      (mat-shadow-dot)
      (label "shadow badge:" :color :dim)
      (mat-shadow-badge))

    (h-stack :gap 2 :align :center
      (label "interactive shadow:" :color :dim)
      (mat-shadow-button)
      (mat-shadow-button)
      (mat-shadow-button))

    (label "Hover and click the shadow buttons to test material expressions with hit state." :color :dim :font-size 10)))
