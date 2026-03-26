(defstate val1 0)
(defstate val2 20)

(defwidget xyz
  :width 4 :height 4
  :state (num)
  :shader
  (sdf/layer
    (sdf/fill (mix 
        (sdf/circle (+ val1 1 (* .1 (cos (* (mod num 2) itime))) ))
        (sdf/rotate 
          (* itime num .12) 
          (sdf/rect (+ 3 (* .2 (sin (* 5 itime)))) (mod num 6)))
        (smoothstep -1 .9 (cos (* (mod (+ 1 num) 4) itime))))
      (material
        :color 
        (mix 
          :black
          :white
          (smoothstep 
            -0.03 val1
            (cos (*
                (- x y) 
                (* (smoothstep -0.2 (* x (cos itime)) y) val2 d)))))
        :shadow (shadow
          :color (* (* 0.1 (mod num 8) (cos itime)) (rgba (mod num 6) 1 1 0.8))
          :blur 2
          :offset (vec2 0.1 0.1)
          :spread 0.3
          ))))) 

(effect-buffer "*control*"
  (v-stack :padding 2
    (v-stack :gap 0
      (h-stack 
        (label "val 1:") (hslider :bind val1 :min 0 :max 4 :width 10))
      (h-stack (label "val 2:") (hslider :bind val2 :min 0 :max 100 :width 10)))
    (box :padding 2
    (grid :col-width 8 :cols 16
      (each (range 0 256) |x| (xyz :num x)) 
      ))))

(delete-other-windows)
(split-window-right "*control*")