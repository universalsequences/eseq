











(defstate matrix (range 0 96))
(defstate steps (range 0 16))
(defstate c 4)
(defstate d true)
(effect
  (v-stack 
    (toggle :bind d)
    (h-stack 
      (label "cirklon sequencer" :color :blue :focusable true)
      (hslider :min 0 :max 100 :bind c)
      (label "transpose" :color :yellow :focusable true)
      (label "velocity" :color :green :focusable true)
      (label "duration" :color :red :focusable true)
      (label "aux_a" :color :cyan :focusable true)
      )
    (h-stack :padding 1
      (each steps |v| (vslider :min 0 :max 100 :bind v))
      (grid :cols 16 :col-width 3
        (each matrix |v| (knob :min 0 :max 1000 :size 2 :bind v))
        ))))

