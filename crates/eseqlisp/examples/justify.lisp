










(defstate t (map |x| true (range 0 16)))
(defstate k (range 0 16)) 
(defstate k2 (range 0 16)) 
(def nums (range 1 17))

(effect
  (h-stack :align :center
    :padding 2
    :gap 0.1
    (each (zip t k k2 nums) |(a b c num)| 
      (v-stack :align :center :gap 0.5
        (toggle :bind a)
        (vslider :bind b :min 0 :max 100 )
        (knob :size 3.2 :bind c :min 0 :max 100)
        (label (fmt "{:.0}" num) :color :gray)
        )))) 