
(defmacro chasis (w h)
  `(let (
      (xmod (smoothstep 0 0.2 (pow (abs x ) 1.5)))
      (ymod (smoothstep 0 0.2 (pow (abs y) 1.5)))
      )
    (sdf/rotate 3 (+ (* 0.5 xmod) (* 0.5 ymod) (sdf/rounded-rect ,w ,h 2.0))))
    )

(defwidget spore
  :width 32 :height 16
  :shader
  (sdf/layer
    (sdf/fill (chasis 1.5 1.5)
      (material :color 
        (mix 
          (mix :white :red y)
          (rgba d 0 (* x y) (* x y))
          (smoothstep 0.3 -0.5 d))))
    (sdf/fill (sdf/circle 0.25) 
      (material :color 
        (mix
          :white
          (rgba 0.1 0.5 0.8 0.8)
          (smoothstep 0.2 -0.1 (* 16 y d))))
      )))

(effect-buffer "*spore*" 
  (box :padding 0
    (spore))
  )
(delete-other-windows)
(split-window-right "*spore*")