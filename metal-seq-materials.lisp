;; metal-seq-materials.lisp - Shared Metal Sequencer materials and widgets.
;; Loaded before the Metal Seq UI buffers that reference these definitions.

;; ── Step cursor highlight ──

(defwidget cursor-highlight
  :width 1 :height 1
  :shader (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.3)
      (material :color (rgba 0.18 0.25 0.35 0.9)))))

;; ── Aqua material for sliders ──


(defmacro aqua-slider-material2 ()
    `(material
       :lighting (lighting :edge-min -0.2015 :edge-max 0.01413
         :light (vec3 -0.1 -1.1 0.5) :shininess 71.0)
       :color
       (let ((base (mix (rgba 0.4 0.1 0.8 1) (rgba 1.0 1.0 1.0 1)
                        (smoothstep -0.02 0 d)))
             (lit (+ 0.6 (* 0.4 diffuse)))
             (shine (* 0.25 specular)))
         (+ (* base (rgba lit lit lit 1.0))
            (rgba shine shine shine 0.0)))))

(defmacro aqua-slider-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.8413
       :light (vec3 -0.1 -0.61 3.5) :shininess 81.0)
     :color (aqua-color (rgba 0.35 0.35 0.8 1.0) (rgba 0.20 0.20 0.92 1.0))))

     

;; ── Aqua widgets ──

(defmacro aqua-color (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
      (__base (mix ,base1
          ,base2
          (smoothstep 1 5 __ny)))
      (__glass (smoothstep 0.85 -0.865 __ny))
      (__edge-fade (smoothstep 0.61 -0.16 d))
      (__hi (* __glass __edge-fade 0.2655))
      (__spec (* specular __edge-fade 0.3))
      (__bot (* (smoothstep 0.29 -0.15 __ny)
          (smoothstep 0.15 0.5 __ny)
          __edge-fade 0.12))
      (__rim (smoothstep 0.8 -0.16183 d)))
    (+ (* __base (rgba __rim __rim __rim 1.0))
      (rgba (+ __hi __spec __bot)
        (+ __hi __spec __bot)
        (+ __hi __spec __bot)
        0.0))))


(defmacro aqua-color-button (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
            (__base (mix ,base1
                ,base2
                (smoothstep 1 5 __ny)))
            (__glass (smoothstep 0.25 -0.865 __ny))
            (__edge-fade (smoothstep 0.01 -0.16 d))
            (__hi (* __glass __edge-fade 0.2655))
            (__spec (* specular __edge-fade 0.3))
            (__bot (* (smoothstep 0.9 -0.15 __ny)
                (smoothstep 0.65 0.5 __ny)
                __edge-fade 0.12))
            (__rim (smoothstep -0.30 -0.16183 d)))
          (+ (* __base (rgba __rim __rim __rim 1.0))
            (rgba (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              0.0))))
(defwidget aqua-button
  :width 4 :height 3
  :paint-margin 1
  :state (active plocked selected)
  :shader
  (let ((sel-y (if (= selected 1) (* 0.03 (cos (* 3 itime))) 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (+ (* 0.001 (smoothstep 0 0.1 (* y x))) (sdf/fill-rounded-rect -0.01 0.85))
          (material
            :lighting
            (lighting :edge-min -0.25 :edge-max 0.15
              :light (vec3 0.1 -1.0 0.5) :shininess 62.0)
            :color
            (* (if (= active 1) 1 0.7) (aqua-color-button (rgba 0.35 0.35 0.45 1.0) (rgba 0.30 0.30 0.92 1.0)))
            :shadow (shadow
              :color (rgba 0 0 0 0.3)
              :blur 0.15
              :offset (vec2 0 0.05))))))))

(defwidget tick
  :width 1.5 :height 1.5
  :state (active plocked selected)
  :shader
  (let ((sel-y (if (= selected 1) (* 0.1 (cos (* 3 itime))) 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (sdf/circle 1)
          (material
            :lighting (lighting :edge-min -0.35 :edge-max 0.5
              :light (vec3 0.0 -1.0 1.5) :shininess 32.0)
            :color
            (* (if (= active 1) 1 0.3)
               (aqua-color
                 (if (= plocked 1) (rgba 0.75 0.15 0.5 1.0) (rgba 0.3 0.3 0.85 1.0))
                 (if (= plocked 1) (rgba 0.4 0.135 0.95 1.0) (rgba 0.90 0.50 0.82 1.0))))))))))

(defwidget page-playhead-dot
  :width 0.7 :height 0.7
  :state (active)
  :shader
  (if (= active 1)
    (sdf/layer
      (sdf/fill (sdf/circle 0.45)
        (material :color (rgba 1 1 1 1))))
    (rgba 0 0 0 0)))

(defwidget step-playhead-dot
  :width 1.0 :height 0.7
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.45)
      (material :color (if (= active 1) (rgba 1 1 1 1) (rgba 0 0 0 0))))))
