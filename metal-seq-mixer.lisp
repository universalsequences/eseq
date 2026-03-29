;; metal-seq-mixer.lisp — Track list with volume sliders + record arm
;; Renders to *mixer* buffer. Loaded by metal-seq-grid.lisp.

;; Record arm indicator (small circle)
(defwidget rec-arm-dot
  :width 1.5 :height 1.5
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.8)
      (material
        :lighting (lighting :edge-min -0.35 :edge-max 0.5
          :light (vec3 0.0 -1.0 1.5) :shininess 82.0)
        :color (* (if (= active 1) 1.0 0.3)
                  (aqua-color (rgba 0.85 0.05 0.05 1.0) (rgba 0.99 0.15 0.15 1.0)))))))

(effect-buffer "*mixer*"
  (v-stack :padding 1 :gap 0.5
    (each SEQ.track-names |name i|
      (h-stack :gap 0.5 :align :center
        (box :width 2 :height 1.5
             :background "rec-arm-dot"
             :active (if (nth SEQ.record-armed i) 1 0)
             :on-click |x y r| (seq-toggle-record-arm i))
        (box :width 12 :height 1
             :bg (if (= SEQ.current-track i) :blue :dark-gray)
             :on-click |x y r| (seq-set-track i)
          (label (substring name 0 16) :font-size 11 :width 12
                 :color (if (= SEQ.current-track i) :white :gray)
                 :bg :transparent))
        (hslider :min 0 :max 1 :width 5
                 :value (nth SEQ.track-volumes i)
                 :material (aqua-slider-material)
                 :on-change (lambda (v) (seq-set-track-volume i v)))))))
