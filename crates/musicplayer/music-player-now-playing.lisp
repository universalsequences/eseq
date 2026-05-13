(defwidget player-transport-bg
  :width 1 :height 1
  :paint-margin 0.35
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.88)
      (material
        :lighting (lighting :edge-min -0.42 :edge-max 1.04
          :light (vec3 -0.31 -0.15 0.14) :shininess 48.0)
        :color
        (mix (rgba 0.28 0.28 0.28 1)
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
                  (lit (+ 0.1 (* 0.1 diffuse)))
                  (shine (* 0.20 specular)))
                (+ base (rgba lit lit lit 1) (rgba shine shine shine 0))
                )
              )) 
          (smoothstep -0.1 0 d))
        :shadow (shadow :color (rgba 0 0 0 0.48) :blur 0.08 :offset (vec2 0 0.03))))))

(defwidget player-play-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) (rgba 1 1 1 1.0) (rgba 0.78 0.79 0.82 1.0))))
    (sdf/layer
      (sdf/fill
        (let ((p1x -0.30) (p1y -0.48) (p2x -0.30) (p2y 0.48) (p3x 0.56) (p3y 0.0))
          (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
            (max (max d1 d2) d3)))
        (material :color fg-col)))))

(defwidget player-pause-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :shader
  (let ((fg-col (rgba 0.90 0.91 0.94 1.0)))
    (sdf/layer
      (sdf/fill (sdf/translate -0.20 0.0 (sdf/rounded-rect 0.13 0.52 0.06))
        (material :color fg-col))
      (sdf/fill (sdf/translate 0.20 0.0 (sdf/rounded-rect 0.13 0.52 0.06))
        (material :color fg-col)))))

(defwidget player-prev-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :shader
  (let ((fg-col (rgba 0.78 0.79 0.82 1.0)))
    (sdf/layer
      (sdf/fill (sdf/translate -0.42 0.0 (sdf/rounded-rect 0.055 0.62 0.03))
        (material :color fg-col))
      (sdf/fill
        (sdf/translate -0.08 0
          (let ((p1x 0.35) (p1y 0.48) (p2x 0.35) (p2y -0.48) (p3x -0.42) (p3y 0.0))
            (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                  (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                  (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
              (max (max d1 d2) d3))))
        (material :color fg-col)))))

(defwidget player-next-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :shader
  (let ((fg-col (rgba 0.78 0.79 0.82 1.0)))
    (sdf/layer
      (sdf/fill (sdf/translate 0.42 0.0 (sdf/rounded-rect 0.055 0.62 0.03))
        (material :color fg-col))
      (sdf/fill
        (sdf/translate 0.08 0
          (let ((p1x -0.35) (p1y -0.48) (p2x -0.35) (p2y 0.48) (p3x 0.42) (p3y 0.0))
            (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                  (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                  (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
              (max (max d1 d2) d3))))
        (material :color fg-col)))))

(defwidget player-volume-icon
  :width 2.4 :height 1.8
  :paint-margin 0.4
  :state (level)
  :shader
  (let ((fg-col (rgba 0.82 0.83 0.86 1.0))
        (wave-col (rgba 0.64 0.66 0.70 1.0))
        (lvl (min 1.0 (max 0.0 level))))
    (sdf/layer
      (sdf/fill (sdf/translate -0.46 0.0 (sdf/rounded-rect 0.15 0.26 0.04))
        (material :color fg-col))
      (sdf/fill
        (let ((p1x -0.32) (p1y -0.25) (p2x -0.32) (p2y 0.25) (p3x 0.02) (p3y 0.44) (p4x 0.02) (p4y -0.44))
          (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                (d3 (- (* (- p4x p3x) (- y p3y)) (* (- p4y p3y) (- x p3x))))
                (d4 (- (* (- p1x p4x) (- y p4y)) (* (- p1y p4y) (- x p4x)))))
            (max (max d1 d2) (max d3 d4))))
        (material :color fg-col))
      (if (> lvl 0.02)
        (sdf/fill (sdf/rounded-rect width height 0.01)
          (material
            :color
            (let ((r (sqrt (+ (* (- x 0.02) (- x 0.02)) (* y y))))
                  (ring (- (abs (- r 0.34)) 0.055))
                  (right (- x 0.06))
                  (cap (- 0.38 (abs y))))
              (let ((aa (max (* 1.6 (fwidth ring)) 0.008))
                    (right-aa (max (fwidth right) 0.004))
                    (cap-aa (max (fwidth cap) 0.004)))
                (let ((mask (* (smoothstep aa (- aa) ring)
                              (smoothstep (- right-aa) right-aa right)
                              (smoothstep (- cap-aa) cap-aa cap))))
                  (rgba 0.64 0.66 0.70 mask))))))
        (rgba 0 0 0 0))
      (if (> lvl 0.45)
        (sdf/fill (sdf/rounded-rect width height 0.01)
          (material
            :color
            (let ((r (sqrt (+ (* (- x 0.02) (- x 0.02)) (* y y))))
                  (ring (- (abs (- r 0.56)) 0.055))
                  (right (- x 0.10))
                  (cap (- 0.56 (abs y))))
              (let ((aa (max (* 1.6 (fwidth ring)) 0.008))
                    (right-aa (max (fwidth right) 0.004))
                    (cap-aa (max (fwidth cap) 0.004)))
                (let ((mask (* (smoothstep aa (- aa) ring)
                              (smoothstep (- right-aa) right-aa right)
                              (smoothstep (- cap-aa) cap-aa cap))))
                  (rgba 0.64 0.66 0.70 mask))))))
        (rgba 0 0 0 0))
      (if (> lvl 0.75)
        (sdf/fill (sdf/rounded-rect width height 0.01)
          (material
            :color
            (let ((r (sqrt (+ (* (- x 0.02) (- x 0.02)) (* y y))))
                  (ring (- (abs (- r 0.78)) 0.055))
                  (right (- x 0.14))
                  (cap (- 0.74 (abs y))))
              (let ((aa (max (* 1.6 (fwidth ring)) 0.008))
                    (right-aa (max (fwidth right) 0.004))
                    (cap-aa (max (fwidth cap) 0.004)))
                (let ((mask (* (smoothstep aa (- aa) ring)
                              (smoothstep (- right-aa) right-aa right)
                              (smoothstep (- cap-aa) cap-aa cap))))
                  (rgba 0.64 0.66 0.70 mask))))))
        (rgba 0 0 0 0)))))

(def percent (value)
  (fmt "{:.0}%" (* value 100)))

(def time-label (seconds)
  (let ((mins (floor (/ seconds 60)))
        (secs (floor (mod seconds 60))))
    (fmt "{:02}:{:02}" mins secs)))

(def marquee-text (text visible)
  (let ((n (len text)))
    (if (<= n visible)
      text
      (let ((hold 2)
            (gap "   ")
            (gap-len 3)
            (tick (floor MP.position)))
        (if (< tick hold)
          (str (substring text 0 (- visible 3)) "...")
          (let ((start (mod (- tick hold) (+ n gap-len)))
                (joined (str text gap text)))
            (substring joined start (+ start visible))))))))

(def transport-icon-button (which action active)
  (box :background "player-transport-bg"
    :active active
    :width 5.0 :height 1.5 :padding 0.18
    :on-click action
    (if (= which "prev")
      (player-prev-icon)
      (if (= which "next")
        (player-next-icon)
        (if (= which "pause")
          (player-pause-icon)
          (player-play-icon :active active))))))

(def now-playing-cover ()
  (if (= MP.current_cover_path "")
    (box :width :fill :aspect 1)
    (image :src MP.current_cover_path
      :width :fill :aspect 1 :fit :cover
      :clip :circle
      :rotation (* MP.position 0.35)
      :rotation-speed (if MP.playing 0.35 0))))

(def now-playing-title ()
  (label (marquee-text MP.current_title 38)
    :width 28 :h-align :center :font-size 10 :color :white :bg :transparent))

(def now-playing-transport ()
  (h-stack :gap 0.45 :align :center
    (transport-icon-button "prev" (lambda (evt) (mp-prev)) 0)
    (transport-icon-button (if MP.playing "pause" "play")
      (lambda (evt) (mp-toggle-play))
      (if MP.playing 1 0))
    (transport-icon-button "next" (lambda (evt) (mp-next)) 0)))

(def now-playing-time ()
  (h-stack :gap 0.2 :align :center
    (label (time-label MP.position) :width 5 :font-size 12 :color :white :bg :transparent)
    (label "/" :width 1.5 :font-size 12 :color :white :bg :transparent)
    (label (time-label MP.duration) :width 5 :font-size 12 :color :white :bg :transparent)))

(def playbar-duration ()
  (if (> MP.duration 0) MP.duration 1))

(def playbar-position ()
  (if (> MP.duration 0)
    (if (> MP.position MP.duration) MP.duration MP.position)
    0))

(defmacro playbar-color (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
         (__base (mix ,base1 ,base2 (smoothstep -0.1 2 __ny)))
         (__glass (smoothstep 0.15 -0.815 __ny))
         (__edge-fade (smoothstep 0.51 -0.116 d))
         (__hi (* __glass __edge-fade 0.28))
         (__spec (* specular __edge-fade 0.92))
         (__bot (* (smoothstep 0.9 0.15 __ny)
                   (smoothstep 0.65 0.5 __ny)
                   __edge-fade 0.04))
         (__rim (smoothstep 0.1 -0.0183 d)))
     (+ (* __base (rgba __rim __rim __rim 1.0))
        (rgba (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              0.0))))

(defmacro playbar-material ()
  `(material
     :lighting (lighting :edge-min -0.115 :edge-max 0.613
       :light (vec3 -0.61 -0.31 1.5) :shininess 81.0)
     :color (playbar-color (rgba 0.21 0.22 0.23 1.0) (rgba 0.12 0.34 0.36 1.0))))

(def now-playing-playbar ()
  (hslider :min 0 :max (playbar-duration)
    :value (playbar-position)
    :width 18
    :material (playbar-material)
    :on-change (lambda (v) (mp-seek v))))

(def now-playing-status ()
  (label MP.status :font-size 12 :color :dim :bg :transparent))

(defwidget transport-panel
  :width 1 :height 1
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.98)
      (material
        :lighting (lighting :edge-min -0.0962 :edge-max 0.8234
          :light (vec3 -0.31 -0.45 0.94) :shininess 98.0)
        :color
        (mix (rgba 0.15 0.15 .15 1)
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
          (smoothstep (+ (* 0.32 
                (smoothstep 0 3 (abs x))
                ) -0.37) 
            -0.0419 d))
        ))))

(effect-buffer "*now-playing*"
  (v-stack :gap 1 :width :fill :flex 1 :padding 1 :align :center
    (subtree :key "now-playing-cover" (now-playing-cover))
    (box :background "transport-panel" :padding 0.5
      (v-stack :gap 0.5 :align :center
        (subtree :key "now-playing-title" (now-playing-title))
        (subtree :key "now-playing-transport" (now-playing-transport))
        (box :gap 0.7 :align :center :width 28
          (subtree :key "now-playing-playbar" (now-playing-playbar)))))
    (box :flex 1)
    (subtree :key "now-playing-status" (now-playing-status))))
