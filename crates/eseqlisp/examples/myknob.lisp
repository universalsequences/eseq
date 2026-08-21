

(defwidget myknob
  :width 16 :height 16
  :state (radius)
  :shader
  (let ((tau 6.2831853))
    (sdf/layer
      (sdf/fill (sdf/circle 1) :dim)
      (sdf/fill
        (sdf/circle 1)
        (material
          :color
          (let (
              (angle (fract (/ (+ (atan2 y (- 0 x)) 3.14159265 tau) tau)))
              (start 0.6666667)
              (sweep 0.8333333)
              (arc-angle (fract (- start angle)))
              (value-angle (* radius sweep))
              (ring-mask (smoothstep 0 -0.03 (- (abs d) 0.1)))
              (track-mask (* ring-mask
                  (- 1 (smoothstep sweep (+ sweep 0.02) arc-angle))))
              (fill-mask (* track-mask
                  (- 1 (smoothstep value-angle (+ value-angle 0.015) arc-angle))))
              (arc-color (mix (rgba 0.1 0.1 0.1 1) :white fill-mask)))
            (mix (mix (rgba 0.9 0.4 0.2 1) :black (smoothstep -0.9 -0.13 (sdf/circle radius))) arc-color track-mask)
            )
          :shadow
          (shadow :color (mix (rgba 0 1 0 0) :white (smoothstep 5 0.41 d))
            :blur 0.9
            :spread 0.1
            :offset (vec2 0.0 0.2))
          )))))

(defstate rad 0.4)
(defstate raw-y 0)
(def on-drag (x y region) 
  (let ((rms (+ 0.5 (* -1 (* 0.5 y)))))
    (set! rad (clamp rms 0 1))
    (set! raw-y y)
    ))

(defwidget bgd 
  :width 2 :height 2
  :shader
  (sdf/layer
    (sdf/fill 
      (sdf/smooth-union 0.2 (sdf/circle (+ 0.5 (* -0.143 (cos itime))))
        (+ (* 0.04 (smoothstep 0 0.19 (cos (* 0.1 (+ 0.3 (cos itime)) (- y x) 30))))
          (sdf/smooth-union 0.1
            (sdf/translate 0 0.5 (sdf/rounded-rect 0.75 0.5 0.25))
            (sdf/rounded-rect 1 (+ 0.5 (* 0.3 (cos itime))) 0.2) )))
      (material
        :color
        (mix 
          (mix :dim :white 
            (mod (cos (* 30 d)) 
              0.03)) 
          (rgba 0.40 0.43 0.43 1) 
          (smoothstep -0.5 0 d))
        ))))

(defstate s1 0.2)
(defstate s2 0.2)
(effect-buffer "*my-knob*"
  (v-stack :padding 2
    (label (fmt "y: {:.1}" raw-y))
    (box  :background "bgd" :width 32 :height 16 :padding 5
      (v-stack :gap 1
        :align :center
        (myknob :radius rad :on-drag on-drag)
        (hslider :bind s1 :min 0 :max 1 :fill :white)
        (hslider :bind s2 :min 0 :max 1 :fill :white)
        )
      )))

(delete-other-windows)
(split-window-right "*my-knob*")
