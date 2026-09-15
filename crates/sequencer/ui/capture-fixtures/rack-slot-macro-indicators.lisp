;; Mapped gain/voice controls on layer 1; the same controls on layer 2 are free.
(capture-project
  (track :layer-rack :name "Macro Indicators"
    :samples ("../../../../content/impulses/lexicon-300-rich-plate.wav"
              "../../../../content/impulses/lexicon-300-rich-plate.wav"))
  (rack-slot-macro 0 0 0 gain 0.5 1.5)
  (rack-slot-macro 0 1 0 max-polyphony 4 12))

(def capture-after-sync ()
  (do
    (eseq.effects.state/rack-panel-set-view
      (get (nth SEQ.instrument-panel 0) :track-id) true true false)))
