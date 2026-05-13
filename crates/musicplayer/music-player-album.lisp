
(defwidget album-panel
  :width 1 :height 1
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.128)
      (material
        :lighting (lighting :edge-min -0.01962 :edge-max 0.38234
          :light (vec3 -0.31 -0.45 0.6294) :shininess 98.0)
        :color
        (mix (rgba 0.12 0.12 .12 1)
          (if (= active 1)
            (let ((base (rgba 0.20 0.24 0.26 1.0))
                (lit (+ 0.1 (* 0.1 diffuse)))
                (shine (* 0.38 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
            (if hit/hover
              (let ((base (rgba 0.20 0.21 0.23 1.0))
                  (lit (+ 0.04 (* 0.035 diffuse)))
                  (shine (* 0.54 specular)))
                (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
              (let ((base (rgba 0.02 0.02 0.02 1.0))
                  (lit (+ 0.01 (* 0.15 diffuse)))
                  (shine (* 0.50 specular)))
                (+ base (rgba lit lit lit 1) (rgba shine shine shine 0))
                )
              )) 
          (smoothstep -0.013 -0.00019 d))
        ))))

(effect-buffer "*album*"
  (box :background "album-panel" :active false
  (v-stack :gap 0.55 :width :fill :flex 1 :padding 1.5
    (subtree :key "album-title"
      (label MP.current_album_label :font-size 16 :color :white :bg :transparent))
    (box :width :fill :background "player-browser-bg" :padding 0 :flex 1
      (subtree :key "album-track-list"
        (scroll :key "album-track-scroll" :width :fill :flex 1
          (tree
            :width :fill
            :items MP.current_album_tracks
            :selected-path MP.current_path
            :row-bg-even '(0.12 0.12 0.13)
            :row-bg-odd '(0.15 0.15 0.16)
            :selected-bg '(0.00 0.35 0.82)
            :folder-color '(0.88 0.88 0.89)
            :file-color '(0.64 0.65 0.68)
            :chevron-color '(0.56 0.56 0.58)
            :on-select (lambda (item) (play-tree-item item))
            :on-activate (lambda (item) (play-tree-item item)))))))))
