;; Number picker demo — Max/MSP-style number box

(load "mac-osx-dark.lisp")

(defwidget panel-bg
  :width 1 :height 1
  :shader (sdf/layer
    (sdf/fill 
      ;; modulate sdf to create kool thingy!
      (+ 
        (* 0.05 (smoothstep 0 0.1 (* x y))) 
        (sdf/rounded-rect width height 0.08)
        )
      (material :color 
        (+ (* (smoothstep (+ (* 0.01 (cos itime )) -0.01) 0.03 d)) (rgba 0.16 0.16 0.17 1))))))

;; ── State ──

(def freq (state 440.0))
(def gain (state 0.75))
(def pan  (state 0.0))

;; ── UI ──

(effect-buffer "*numbers*"
  (box :padding 2
    (box :background "panel-bg" :padding 2
      (v-stack :padding 1.5 :gap 1.5
        (label "simplex" :color :white :bg :transparent :font-size 16)
  
        ;; Single number picker
        (h-stack :gap 1 :align :center
          (label "freq:" :color :gray :bg :transparent :font-size 13 :width 5)
          (number-picker :value freq :min 20 :max 20000 :decimals 1
            :on-change (lambda (v) (set! freq v))
            :width 10 :height 1.5 :font-size 14))
        
        (h-stack :gap 1 :align :center
          (label "gain:" :color :gray :bg :transparent :font-size 13 :width 5)
          (number-picker :value gain :min 0 :max 1 :decimals 3
            :on-change (lambda (v) (set! gain v))
            :width 10 :height 1.5 :font-size 14))
        
        (h-stack :gap 1 :align :center
          (label "pan:"  :color :gray :bg :transparent :font-size 13 :width 5)
          (number-picker :value pan :min -1 :max 1 :decimals 2
            :on-change (lambda (v) (set! pan v))
            :width 10 :height 1.5 :font-size 14))
        
        ;; Combined: label + number-picker + slider
        (label "──── With sliders ────" :color :gray :bg :transparent :font-size 12)
        
        (h-stack :gap 1 :align :center
          (label "gain:" :color :gray :bg :transparent :font-size 13 :width 5)
          (number-picker :value gain :min 0 :max 1 :decimals 3
            :on-change (lambda (v) (set! gain v))
            :width 8 :height 1.3 :font-size 13)
          (hslider :value gain :min 0 :max 1 :width 20
            :on-change (lambda (v) (set! gain v)))))))
  )

(delete-other-windows)
(split-window-right "*numbers*")