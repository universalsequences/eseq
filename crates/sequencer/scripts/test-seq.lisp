

(defmacro sq (x time) 
  `(def-sequencer "my-seq"
    :resolution :16
    :tick (seq-emit 
      :track (mod (gen-tick) 4) :at :now 
      :quantize (if (= 0 (mod (gen-tick) ,x)) :16 ,time)
      :vel 1
      )))

(sq 3 :8t)
  
  