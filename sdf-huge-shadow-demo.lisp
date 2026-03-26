;; sdf-huge-shadow-demo.lisp — stress test for oversized SDF shadows
;;
;; This demo is intentionally exaggerated:
;; - small logical widget sizes
;; - very large shadow blur/offset/spread
;; - minimal surrounding content so overflow is easy to inspect

(defwidget huge-shadow-dot
  :width 2 :height 2
  :shader
  (let ((shape (sdf/circle 0.62)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (mix :accent :white
                      (smoothstep -0.08 0.03 d))
          :shadow (shadow
                    :color (rgba 0 0 0 0.30)
                    :blur 1.0
                    :offset (vec2 0.25 0.35)
                    :spread 0.35))))))

(defwidget huge-shadow-badge
  :width 4 :height 2
  :shader
  (let ((shape (sdf/rounded-rect 0.88 0.52 0.14)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (mix :fg :accent
                      (smoothstep -1.0 1.0 y))
          :shadow (shadow
                    :color (rgba 0 0 0 0.26)
                    :blur 1.2
                    :offset (vec2 0.3 0.45)
                    :spread 0.45))))))

(defwidget huge-shadow-button
  :width 6 :height 2
  :shader
  (let ((shape (sdf/rounded-rect 0.9 0.55 0.16)))
    (sdf/layer
      (sdf/fill shape
        (material
          :color (if hit/active
                   :primary
                   (if hit/hover
                     (mix :accent :white (smoothstep -0.1 0.04 d))
                     (mix :dim :accent (smoothstep -0.2 0.0 d))))
          :shadow (shadow
                    :color (rgba 0 0 0 0.34)
                    :blur 1.3
                    :offset (vec2 0.35 0.5)
                    :spread 0.4))))))

(effect
  (v-stack :padding 2 :gap 3
    (label "Huge Shadow Demo" :font-size 18 :color :accent)
    (label "These widgets have small logical sizes but intentionally oversized shadows." :color :dim :font-size 10)

    (h-stack :gap 8 :align :center
      (v-stack :gap 1 :align :center
        (label "2x2 dot" :color :dim)
        (huge-shadow-dot))
      (v-stack :gap 1 :align :center
        (label "4x2 badge" :color :dim)
        (huge-shadow-badge)))

    (v-stack :gap 1 :align :center
      (label "6x2 interactive button" :color :dim)
      (h-stack :gap 3 :align :center
        (huge-shadow-button)
        (huge-shadow-button)))

    (label "If overflow is working, the shadow should extend far beyond each widget's reserved layout box." :color :dim :font-size 10)))
