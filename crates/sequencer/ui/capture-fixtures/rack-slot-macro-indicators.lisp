;; Mapped gain/voice controls on layer 1; the same controls on layer 2 are free.
(capture-project
  (track :layer-rack :name "Macro Indicators"
    :samples ("../../../../content/impulses/lexicon-300-rich-plate.wav"
              "../../../../content/impulses/lexicon-300-rich-plate.wav"))
  (rack-slot-macro 0 0 0 gain 0.5 1.5)
  (rack-slot-macro 0 1 0 max-polyphony 4 12))

(def capture-after-sync ()
  (do
    (set! eseq.effects.state/rack-panel-slot-list-open true)
    (set! eseq.effects.state/rack-panel-selected-chain-open false)
    (set! eseq.effects.state/rack-panel-macros-open true)))
