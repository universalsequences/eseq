;; Rack-owned macro bank regression fixture. The slot list is closed so the
;; toolbar toggle and the complete 4x2 public macro surface are easy to inspect.
(capture-project
  (track :layer-rack
    :name "Macro Rack"
    :samples ("../../assets/ir/lexicon-300-rich-plate.wav")
    :rack-slot-audio-fx ("OTT")))

(def capture-after-sync ()
  (set! eseq.effects.state/rack-panel-slot-list-open false)
  (set! eseq.effects.state/rack-panel-selected-chain-open false)
  (set! eseq.effects.state/rack-panel-macros-open true))
