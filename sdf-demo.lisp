;; sdf-demo.lisp — SDF widget showcase
;;
;; Demonstrates defwidget: defining GPU-rendered widgets entirely in Lisp.
;; Each widget is a signed distance field compiled to a Metal fragment shader.

;; ── State ─────────────────────────────────────────────────────────────

(defstate pads (map (lambda (x) false) (range 0 16)))
(defstate pad-x 0.0)
(defstate pad-y 0.0)

;; ── Container backgrounds ─────────────────────────────────────────────

(defwidget rounded-panel
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect 0.95 0.95 0.08) :dim)))

(defwidget blob-panel
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill
              (sdf/smooth-union 0.4
                (sdf/translate -0.3 -0.2 (sdf/circle 0.7))
                (sdf/translate 0.3 0.2 (sdf/circle 0.7)))
              :dim)))

;; ── Define SDF widgets ────────────────────────────────────────────────

;; A simple filled circle
(defwidget sdf-dot
  :width 3 :height 3
  :shader (sdf/layer
             (sdf/fill (sdf/circle 0.7) :accent)))

;; A ring (circle outline via stroke)
(defwidget sdf-ring
  :width 4 :height 4
  :shader (sdf/layer
             (sdf/stroke (sdf/circle 0.7) 0.05 :accent)))

;; A rounded rectangle badge
(defwidget sdf-badge
  :width 8 :height 3
  :shader (sdf/layer
             (sdf/fill (sdf/rounded-rect 0.9 0.8 0.15) :accent)))

;; Bullseye: concentric circle + ring
(defwidget sdf-bullseye
  :width 5 :height 5
  :shader (sdf/layer
             (sdf/fill (sdf/circle 0.8) :dim)
             (sdf/stroke (sdf/circle 0.8) 0.03 :accent)
             (sdf/fill (sdf/circle 0.4) :accent)
             (sdf/stroke (sdf/circle 0.4) 0.03 :primary)))

;; Union of two shapes
(defwidget sdf-blob
  :width 8 :height 4
  :shader (sdf/layer
             (sdf/fill
               (sdf/smooth-union 0.3
                 (sdf/translate -0.3 0 (sdf/circle 0.5))
                 (sdf/translate 0.3 0 (sdf/circle 0.5)))
               :accent)))

;; Crosshair
(defwidget sdf-crosshair
  :width 5 :height 5
  :shader (sdf/layer
             (sdf/fill (sdf/circle 0.85) :dim)
             (sdf/paint (sdf/rect 0.02 0.8) :accent)
             (sdf/paint (sdf/rect 0.8 0.02) :accent)
             (sdf/stroke (sdf/circle 0.5) 0.02 :accent)))

;; Interactive: hover changes color, click highlights
(defwidget sdf-button
  :width 8 :height 3
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect 0.9 0.8 0.15)
              (if hit/active :primary
                (if hit/hover :accent :dim)))))

;; Drumpad: toggleable pad with state uniform
(defwidget drumpad
  :width 4 :height 4
  :state (active)
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect 0.85 (+ 0.85 (* .05 (cos (* itime 4)))) (+ 0.42 (* 3 (cos itime))))
              (if active
                 :primary :dim))))

;; XY Pad: draggable circle on a rectangle
(defwidget xy-pad
  :width 16 :height 10
  :state (pos-x pos-y)
  :shader (sdf/layer
            (sdf/fill (sdf/rect 0.98 0.98) :dim)
            (sdf/paint
              (sdf/union (sdf/rect 0.005 0.95) (sdf/rect 0.95 0.005))
              :bg)
            (sdf/paint
              (sdf/translate pos-x pos-y (sdf/circle (+ 0.3 (* .03 (+  1 (cos (* 9.3 itime)))))))
              (if hit/active :primary :accent))))

;; Multi-region hit test: 3 circles, each a separate sdf/fill.
;; Hovering highlights the hovered region, clicking shows active.
(defwidget sdf-three-regions
  :width 16 :height 5
  :shader (sdf/layer
    ;; Region 0: left circle
    (sdf/fill (sdf/translate -0.55 0 (sdf/circle 0.3))
      (if hit/active :primary (if hit/hover :accent :dim)))
    ;; Region 1: center circle
    (sdf/fill (sdf/circle 0.3)
      (if hit/active :primary (if hit/hover :accent :dim)))
    ;; Region 2: right circle
    (sdf/fill (sdf/translate 0.55 0 (sdf/circle 0.3))
      (if hit/active :primary (if hit/hover :accent :dim)))))

;; ── Render the demo ───────────────────────────────────────────────────

(effect
  (v-stack
    (label "SDF Demo" :font-size 32)
    (h-stack 
      
      (v-stack :padding 1 :gap 1
        
        
        (h-stack :gap 2 :align :center
          (label "dot:" :color :dim)
          (sdf-dot)
          (label "ring:" :color :dim)
          (sdf-ring)
          (label "badge:" :color :dim)
          (sdf-badge))
        
        (h-stack :gap 2 :align :center
          (label "bullseye:" :color :dim)
          (sdf-bullseye)
          (label "blob:" :color :dim)
          (sdf-blob))
        
        (h-stack :gap 2 :align :center
          (label "crosshair:" :color :dim)
          (sdf-crosshair))
        
        (h-stack :gap 2 :align :center
          (label "button:" :color :dim)
          (sdf-button)
          (sdf-button)
          (sdf-button))
        
        (h-stack :gap 2 :align :center
          (label "3 regions:" :color :dim)
          (sdf-three-regions))
        (label "Hover/click the shapes — each circle is a separate hit region" :color :dim :font-size 10)
        
        (label (fmt "XY Pad (drag the dot) {:.1},{:.1}" pad-x pad-y) :font-size 14 :color :accent)
        (xy-pad :pos-x pad-x :pos-y pad-y
          :on-mouse-up (lambda (x y region)
            (do (set! pad-x 0) (set! pad-y 0)))
          :on-drag (lambda (x y region)
            (set! pad-x (clamp x -1 1))
            (set! pad-y (clamp y -1 1))))
        )
      
      (v-stack :gap 2
        
        
        ;; ── Interactive drumpad demo ─────────────────────────────────────────
        (label "Drumpad (click to toggle)" :font-size 14 :color :accent)
        (grid :cols 4
          (each pads |x| (toggle :bind x)))
        (grid :cols 4 :col-width 8
          (each (zip pads (range 0 16)) |(pad i)|
            (drumpad :active pad
              :on-click (lambda (x y region)
                (if (= region 0)
                  (set! pads (set-nth pads i (not pad)) ) 
                  nil
                  )))))
        
        ;; ── Container background demos ─────────────────────────────────────
        (label "SDF Container Backgrounds" :font-size 18 :color :accent)
        
        (h-stack :gap 2
          (box :background "rounded-panel" :padding 2 :width 20 :height 8
            (v-stack :gap 1
              (label "Rounded Panel" :font-size 12 :color :accent)
              (slider :bind pad-x)
              (slider :bind pad-y)))
          
          (box :background "blob-panel" :padding 2 :width 20 :height 8
            (v-stack :gap 1
              (label "Blob Panel" :font-size 12 :color :accent)
              (toggle :bind (first pads))
              (knob :bind pad-x :size 3))))))
    
    )
  )
