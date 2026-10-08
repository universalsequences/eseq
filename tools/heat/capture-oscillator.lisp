(capture-project
  (track :instrument "Synths/Heat" :name "Heat"))

(def capture-after-sync ()
  (do
    (custom-instrument-synth-ui (eseq.effects.panel-data/current-instrument-panel))
    (eseq.effects.custom-ui-sections/ui-select-section 1)))
