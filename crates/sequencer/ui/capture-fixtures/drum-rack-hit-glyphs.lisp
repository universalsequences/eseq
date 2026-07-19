;; Compact drum-rack capture for reviewing monochrome pad-shape outlines.
(capture-project
  (track :drum-rack :name "Drum Rack" :num-steps 16
    :samples (
      "../../assets/ir/lexicon-300-rich-plate.wav"
      "../../assets/ir/lexicon-300-rich-plate.wav"
      "../../assets/ir/lexicon-300-rich-plate.wav"
      "../../assets/ir/lexicon-300-rich-plate.wav"
      "../../assets/ir/lexicon-300-rich-plate.wav"
      "../../assets/ir/lexicon-300-rich-plate.wav")))

(seq-toggle-step 0)
(seq-set-step-param 0 :transpose 0)
(seq-toggle-step 2)
(seq-set-step-param 2 :transpose 1)
(seq-toggle-step 4)
(seq-set-step-param 4 :transpose 2)
(seq-toggle-step 7)
(seq-set-step-param 7 :transpose 3)
(seq-toggle-step 10)
(seq-set-step-param 10 :transpose 4)
(seq-toggle-step 13)
(seq-set-step-param 13 :transpose 5)
