;; sdf-aqua-demo.lisp — Mac OS X Aqua button style

;; Aspect-aware rounded rect that fills the box
(defmacro sdf/fill-rounded-rect (inset r)
  `(sdf/rounded-rect (- (max aspect 1.0) ,inset)
    (- (max (/ 1.0 aspect) 1.0) ,inset)
    ,r))

;; ── Aqua Button ─────────────────────────────────────────────────────────

(defmacro aqua-color (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
            (__base (mix ,base1
                ,base2
                (smoothstep -0.5 0.5 __ny)))
            (__glass (smoothstep 0.1 -0.65 __ny))
            (__edge-fade (smoothstep 0.0 -0.26 d))
            (__hi (* __glass __edge-fade 0.655))
            (__spec (* specular __edge-fade 0.3))
            (__bot (* (smoothstep 0.3 0.5 __ny)
                (smoothstep 0.65 0.5 __ny)
                __edge-fade 0.12))
            (__rim (smoothstep -0.53 -0.033 d)))
          (+ (* __base (rgba __rim __rim __rim 1.0))
            (rgba (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              0.0)))
  )

(defwidget aqua-button
  :width 4 :height 3
  :paint-margin 1
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (+ (* 0.2 (smoothstep 0 0.3 (* y x))) (sdf/fill-rounded-rect -0.01 0.85))
      (material
        :lighting 
        (lighting :edge-min -0.35 :edge-max 0.5
          :light (vec3 (cos (* 0.3 itime)) -1.0 (+ (* 0.2 (cos itime)) 1.5)) :shininess 32.0)
        :color
        (* (if active 1 0.7) (aqua-color (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0)))
        :shadow (shadow
          :color (rgba 0 0 0 0.3)
          :blur 0.15
          :offset (vec2 0 0.05)))))) 

;; Graphite variant
(defwidget aqua-graphite
  :width 14 :height 3
  :paint-margin 1
  :shader
  (sdf/layer
    (sdf/fill (+ (* (mix 0.417 0.1927 (- y x)) (smoothstep 0 1.2 (mix (- y x) (cos (* 7 (* x y))) (* x y) ))) (sdf/fill-rounded-rect 0.05 0.35))
      (material
        :lighting (lighting :edge-min -0.1 :edge-max 0.3
          :light (vec3 0.0 -0.930 1.5) :shininess 82.0)
        :color
        (let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
            (__base (mix (rgba 0.10 0.12 0.168 1.0)
                (rgba 0.03 0.03 0.06 1.0)
                (smoothstep -0.5 0.5 __ny)))
            (__glass (smoothstep 0.9 -0.35 __ny))
            (__edge-fade (smoothstep 0.0 -0.06 d))
            (__hi (* __glass __edge-fade 0.0145))
            (__spec (* specular __edge-fade 0.45))
            (__bot (* (smoothstep 0.4 0.95 __ny)
                (smoothstep 0.05 0.5 __ny)
                __edge-fade 0.2))
            (__rim (smoothstep 0.50 -0.05 d)))
          (+ (* __base (rgba __rim __rim __rim 1))
            (rgba (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              0.0)))
        :shadow (shadow
          :color (rgba 1 1 1 0.25)
          :blur 0.2
          :spread 0.01
          :offset (vec2 0 0.04))))))

;; White/silver variant
(defwidget aqua-white
  :width 14 :height 3
  :paint-margin 1
  :shader
  (sdf/layer
    (sdf/fill (sdf/fill-rounded-rect 0.05 0.35)
      (material
        :lighting (lighting :edge-min -0.5 :edge-max 0.03
          :light (vec3 0.0 -1.0 1.5) :shininess 24.0)
        :color
        (let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
            (__base (mix (rgba 0.72 0.74 0.78 1.0)
                (rgba 0.88 0.89 0.92 1.0)
                (smoothstep -0.5 0.5 __ny)))
            (__glass (smoothstep 0.1 -0.35 __ny))
            (__edge-fade (smoothstep 0.0 -0.06 d))
            (__hi (* __glass __edge-fade 0.45))
            (__spec (* specular __edge-fade 0.2))
            (__rim (smoothstep 0.0 -0.12 d)))
          (+ (* __base (rgba __rim __rim __rim 1.0))
            (rgba (+ __hi __spec) (+ __hi __spec) (+ __hi __spec) 0.0)))
        :shadow (shadow
          :color (rgba 0 0 0 0.2)
          :blur 0.10
          :offset (vec2 0 0.03))))))

(defwidget tick
  :width 2 :height 2
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 1)
      (material
        :lighting (lighting :edge-min -0.35 :edge-max 0.5
          :light (vec3 0.0 -1.0 1.5) :shininess 32.0)
        :color
        (* (if active 1 0.3) (aqua-color (rgba (if active 0.3 0.1) (if active 0.3 0.1) 0.75 1.0) (rgba 0.90 0.50 0.92 1)))
        ))))

;; ── Aqua VSlider ──────────────────────────────────────────────────────
;; value is 0-1, maps to thumb position along the vertical track.
;; y in SDF coords: -height_extent = top, +height_extent = bottom.
;; value=1 → thumb at top, value=0 → thumb at bottom.

(defwidget aqua-vslider
  :width 2 :height 4
  :state (value yo)
  :shader
  (let ((track-h (- (max (/ 1.0 aspect) 1.0) 0.3))
        (thumb-r 0.85)
        (thumb-y (- track-h (* 2.0 track-h value))))
    (sdf/layer
      ;; Track groove — thin vertical rounded rect, inset look
      (sdf/fill (sdf/rounded-rect 0.32 track-h 0.30)
        (material
          :color (mix (rgba 0.08 0.08 0.12 1.0)
                      (rgba 0.15 0.15 0.20 1.0)
                      (smoothstep -0.05 0.05 d))))

      ;; Fill bar — from bottom up to thumb position
      (sdf/paint
        (sdf/intersect
          (sdf/rounded-rect 0.08 track-h 0.06)
          (sdf/translate 0 (+ thumb-y track-h)
            (sdf/rect 0.2 track-h)))
        (material
          :color (* (if yo 0.3 1) (mix (rgba 0.10 0.35 0.85 1.0)
                      (rgba 0.25 0.55 0.95 1.0)
                      (smoothstep (- track-h) track-h y)))))

      ;; Thumb — aqua glass sphere
      ;; Rebind y relative to thumb center so the aqua gradient is consistent
      (sdf/fill
        (sdf/translate 0 thumb-y (sdf/circle thumb-r))
        (material
          :lighting (lighting :edge-min -0.25 :edge-max 0.4
                      :light (vec3 0.0 -1.0 1.5) :shininess 48.0)
          :color
          (let ((y (- y thumb-y)))
             (aqua-color (* (if yo 0.3 1) (rgba 0.20 0.35 0.80 1.0))  (rgba (if yo 0 0.35) (if yo 0.3 0.60) (if yo 0.3 0.95) 1.0))))))))

;; ── Demo ───────────────────────────────────────────────────────────────

(defstate slider1 0.5)
(defstate slider2 0.3) 
(defstate slider3 0.7)
(defstate sliders (map |x| 0.5 (range 0 16)))
(defstate toggles (map |x| 1 (range 0 16)))
(defstate ra 0)
(def on-drag-s1 (x y region)
  (set! slider1 (clamp (+ 0.5 (* -0.5 y)) 0 1)))
(def on-drag-s2 (x y region)
  (set! slider2 (clamp (+ 0.5 (* -0.5 y)) 0 1)))
(def on-drag-s3 (x y region)
  (set! slider3 (clamp (+ 0.5 (* -0.5 y)) 0 1)))

(def conv (y)
  (clamp (+ 0.5 (* -0.5 y)) 0 1))


(effect-buffer "*aqua*"
  (v-stack :padding 2 :gap 2
    (box :background "aqua-graphite"
      :padding 4.5
      (v-stack
        :gap 1.5
        (h-stack :gap 1.5
          (label "vel" :color :black :bg :transparent)
          (label "xpose" :color :dim :bg :transparent)
          (label "dur" :color :dim :bg :transparent)
          (label "aux a" :color :dim :bg :transparent)
          (label "aux b" :color :dim :bg :transparent)                    
          )
        (h-stack
          :gap 0.25
          (each (zip sliders toggles (range 0 16)) |(v t i)|
            (v-stack :align :center :gap 0.5
              (aqua-vslider 
                :value v 
                :yo (if t 0 1)
                :on-drag |x y r| (set! sliders (set-nth sliders i (conv y))))
              (box 
                :on-click |x y r| (set! toggles (set-nth toggles i (if (> t 0.5) 0 1)))
                :active t 
                :background "aqua-button" :align :center :padding 0.25 :width 4 :height 2
                (tick 
                  :active t
                  )
                )
              (label (+ i 1) :font-size 8 :color :white :bg :transparent)
              )))))
    (h-stack :gap 3 :align :baseline
      (label "aqua" :font-size 64)
      (label "mar 23, 2025" :font-size 16 :color :dim)
      )))
 

(delete-other-windows)
(split-window-right "*aqua*")