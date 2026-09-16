;; Presentation seed shared by the full-project capture and modal layout test.
(reactive-set "RETRO" "duration" 30)
(reactive-set "RETRO" "lanes"
  (list (dict :id 0 :label "Kick · C4")
        (dict :id 1 :label "Snare · C4")
        (dict :id 2 :label "Hi-hat · C4")))
(reactive-set "RETRO" "items"
  (append
    (map |t| (dict :id t :lane 0 :start t :end (+ t 0.14))
      (list 17.1 18.04 20.05 20.78 21.92 22.84 23.64 26.02))
    (map |t| (dict :id t :lane 1 :start t :end (+ t 0.1))
      (list 18.49 20.53 21.48 22.43 23.39 26.49))
    (map |t| (dict :id t :lane 2 :start t :end (+ t 0.06))
      (list 20.02 20.25 20.5 20.77 21.0 21.24 21.49 21.73 21.97
            22.21 22.45 22.69 22.94 23.18 23.41 23.65))))
(eseq.retrospective/open 20 24)
