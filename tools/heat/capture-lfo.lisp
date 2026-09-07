(capture-project
  (track :instrument "user:Heat Development" :name "Heat"))

(def capture-after-sync ()
  (do
    (custom-instrument-synth-ui (nth SEQ.instrument-panel 0))
    (eseq.effects.custom-ui-sections/ui-select-section 4)))
