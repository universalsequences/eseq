;; Minimal test: SDF background on a box container

(defstate val1 1)
(defstate val2 0.1)
(defstate val3 0.1)
(defstate val4 0.1)
(defstate val5 0.1)
(defstate val6 0.1)
(defstate vals (range 0 8))
(defstate bools (range 0 8))

(defwidget gradient
  :width 32 :height 32
  :shader (sdf/layer (sdf/fill (sdf/rect 8 8)
      (material
        :color (mix (mix (rgba 0 0.0 0.1 0.1) :black (smoothstep 0 0.2 (cos (* 0.3 itime (cos y) 4 d d)))) :black (- x y)))
      )))

(defwidget rounded-bg
  :width 1 :height 8
  :shader (sdf/layer
    (sdf/fill (sdf/circle (+ (cos itime) 0.75) )
      (material
        :color (mix :red (rgba 1 0.1513 (* x y) 0.1) (smoothstep -0.6 0.03 d)))
      )
    (sdf/fill 
      (+
        (* (+ 0.3 (* 0.03 (cos (* 5 itime)))) 
          (smoothstep 0 0.2 (mix (cos (* 32 (cos (* 0.3 itime)) y)) (abs (- x y)) (abs y))))
        (sdf/rounded-rect 2 1 1))
      
      (material 
        :color (mix :white :black (smoothstep 0.5 -0.2 (* x d)))
        :shadow 
        (shadow
          :color (rgba 1 1 1  0.52)
          :offset (vec2 0.005 0.02)
          :blur 0.2
          :spread 0.01
          )
        )))) 

(defwidget starry
  :width 8 :height 8
  :state (radius)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle radius) 
      (material :color 
        (mix :black :white (smoothstep -0.3 0.3 (* y d)))
        :shadow
        (shadow :color (rgba 1 1 1 0.9)
          :offset (vec2 0.125 0.125)
          :blur 0.5
          :spread 0.1)
        ))))


(effect-buffer "*panel*"
  (box :background "gradient"
    (v-stack
      :padding 2
      :gap 2
      (h-stack :align :baseline
        (label "winamp" :font-size 32 :bg :transparent)
        (label "(enterprise edition)" :font-size 16 :bg :transparent)
        )
      (box :background "rounded-bg" :state val2 :padding 2 
        (h-stack
          :align :baseline
          :gap 1
          (starry :radius val2 )
          (knob :size 16 :bind val2)
          (v-stack :gap 0.1
            (label "equalizer" :color :white :bg :transparent)
            (slider :bind val3)
            (slider :bind val4)
            (slider :bind val5)))
        )
      (h-stack :gap 3
        (each (zip vals bools (range 1 9)) |(y w n)| 
          (v-stack :align :center
            :gap 1
            (toggle :bind w)
            (vslider :bind y :min 0 :max 100)
            (label n)
            )))
      )))

(delete-other-windows)
(split-window-right "*panel*")