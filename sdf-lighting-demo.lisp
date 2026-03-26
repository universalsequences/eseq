;; sdf-lighting-demo.lisp — 2D SDF lighting via estimated normals
;;
;; IMPORTANT: sdf/normal needs the raw SDF expression (not a let-bound variable)
;; so it can re-evaluate the SDF at offset coordinates for central differences.

;; ── Widgets ─────────────────────────────────────────────────────────────



(defmacro mysdf (a)
  `(+ 
    (* 0.1 (smoothstep 0 ,a (cos (* 4 (cos itime) (- x y)))))
    (sdf/smooth-union 
      0.25 
      (sdf/translate 
        (* 0.4 (cos itime))
        (* 1.7 (sin itime) )
        (sdf/circle 0.5))
      (sdf/rounded-rect 1 1.5 0.3))))

(defwidget lit-dome
  :width 15 :height 15
  :shader
  (sdf/layer
    (sdf/fill (mysdf 0.5)
      (material
        :color 
        (sdf/lit 
          (mix (rgba 1 0 0.5 1) :white (smoothstep -0.3 0.3 d))
          (mysdf 0.5) -0.3 0.9))))) 

;; Bevel: tight range = sharp rim lighting
(defwidget lit-bevel
  :width 2 :height 1
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.8)
      (material
        :color (sdf/lit (mix :black :dim (smoothstep -0.3 0 d)) (sdf/circle 0.8) -0.52 0.82)))))

;; Rounded rect dome
(defwidget lit-button
  :width 8 :height 3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect 0.9 0.7 0.18)
      (material
        :color (sdf/lit :accent (sdf/rounded-rect 0.9 0.7 0.18) -0.5 0.05)))))

;; Rounded rect bevel
(defwidget lit-button-bevel
  :width 16 :height 3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect 1.5 0.7 0.18)
      (material
        :color (sdf/lit 
          (mix 
            (rgba 0 0.1 0.016 1) 
            (rgba 0 0.1 0.2 1)
            (- y d)) 
          (sdf/rounded-rect 1.5 0.7 0.18)
          -0.12 0.4)))))

;; Lit + shadow
(defwidget lit-shadow-badge
  :width 8 :height 3
  :paint-margin 1
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect 0.92 0.6 0.16)
      (material
        :color (sdf/lit :accent (sdf/rounded-rect 0.92 0.6 0.16) -0.4 0.05)
        :shadow (shadow
                  :color (rgba 0 0 0 0.22)
                  :blur 0.20
                  :offset (vec2 0 0.06))))))

;; Interactive hover
(defwidget lit-interactive
  :width 8 :height 3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect 0.9 0.7 0.18)
      (material
        :color (let ((base (if hit/hover :white :accent)))
                 (sdf/lit base (sdf/rounded-rect 0.9 0.7 0.18) -0.5 0.05))))))

;; ── Demo ───────────────────────────────────────────────────────────────
(defstate v1 0.5)
(defstate v2 0.5)

(effect-buffer "*light*"
  (v-stack :padding 1 :gap 1
    (label "SDF Lighting Demo" :font-size 18 :color :accent)
    (label "edge-min/edge-max control curvature: wide = dome, tight = bevel" :color :dim :font-size 10)
    
    (h-stack :gap 3 :align :center
      (label "sdf-fuck (-0.7, 0.05):" :color :dim)
      (box :background "lit-dome" :padding 2 :width 16 :height 16
        :align :center
        (v-stack :align :center
          (box :background "lit-button-bevel" :width 16 :height 3 :align :center
          (label "hello" :bg :transparent :font-size 32))
          (hslider :min 0 :max 1 :bind v1 :fill :white)
          (hslider :min 0 :max 1 :bind v2 :fill :white)
          (label "we luv sdfs" :bg :transparent)
          (grid :cols 2 (each (range 0 4) |z| (lit-bevel)))
          ))
      
      )
    
    (h-stack :gap 3 :align :center
      (label "rect dome:" :color :dim)
      (lit-button)
      (label "rect bevel:" :color :dim)
      (lit-button-bevel))
    
    (h-stack :gap 3 :align :center
      (label "lit + shadow:" :color :dim)
      (lit-shadow-badge)
      (label "interactive:" :color :dim)
      (lit-interactive))
    
    (label "Hover the interactive button. The sdf/lit macro takes (base-color sdf-expr edge-min edge-max)." :color :dim :font-size 10)))

(delete-other-windows)
(split-window-right "*light*")