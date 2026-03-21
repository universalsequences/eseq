














(defstate matrix (range 0 96))
(defstate steps (range 0 16))
(defstate c 4)
(defstate d true)
(effect
  (v-stack 
    :gap 0.5
    (toggle :bind d)
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
    (h-stack :padding 1
      (each steps |v| (vslider :min 0 :max 100 :bind v))
      (grid :cols 16 :col-width 2
        (each matrix |v| (knob :min 0 :max 1000 :size 2 :bind v))
        ))))

