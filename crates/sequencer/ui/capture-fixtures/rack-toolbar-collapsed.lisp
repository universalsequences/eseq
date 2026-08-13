;; Rack view-toolbar regression fixture. Both optional rack regions are closed
;; so the compact header and always-visible toolbar can be inspected together.
(capture-project
  (track :layer-rack
    :name "Collapsed Layer Rack"
    :samples ("../../assets/ir/lexicon-300-rich-plate.wav")
    :rack-slot-audio-fx ("OTT")))

(def capture-after-sync ()
  (set! eseq.effects.state/rack-panel-slot-list-open false)
  (set! eseq.effects.state/rack-panel-selected-chain-open false))
