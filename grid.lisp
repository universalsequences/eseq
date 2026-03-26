














(defstate matrix (range 0 96))
(defstate matrix2 (range 0 64))
(defstate steps (range 0 16))
(defstate c 4)
(defstate d true)
(effect
  (v-stack 
    :gap 0.5
    (h-stack 
      :align :baseline
      :padding 0.5
      (label "cirklon sequencer" :font-size 36 :color :white :focusable true)
      
      (label "transpose" :color :gray :focusable true)
      (label "velocity" :color :gray :focusable true)
      (label "duration" :color :gray :focusable true)
      (label "aux_a" :color :gray :focusable true)
      (hslider :min 0 :max 100 :bind c :fill :secondary)
      )
    (h-stack
    (v-stack
      (h-stack :padding 1
        
        (each steps |v| (vslider :height 4 :min 0 :max 100 :bind v)))
      (grid :cols 16 :col-width 3 (each matrix2 |v| (knob :color :secondary :min 0 :max 100 :size 3 :bind v))))
    
    (grid :cols 16 :col-width 4
      (each matrix |v| (knob :min 0 :max 1000 :size 4 :bind v :fill :secondary))
      ))))

