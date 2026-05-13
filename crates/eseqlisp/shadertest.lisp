
(def show-shader (s)
  (do 
    (delete-other-windows)
    (create-scratch "*metal2*" (sdf->metal s))
    (split-window-right "*metal2*")
    )
  ) 

(show-shader '(sdf/rect 0.5 0.2)) 

