;; Rack view-toolbar regression fixture. Both optional rack regions are closed
;; so the compact header and always-visible toolbar can be inspected together.
(capture-project
  (track :layer-rack
    :name "Collapsed Layer Rack"
    :samples ("../../assets/ir/lexicon-300-rich-plate.wav")
    :rack-slot-audio-fx ("OTT")))

(def capture-after-sync ()
  (eseq.effects.state/rack-panel-set-view
      (get (nth SEQ.instrument-panel 0) :track-id) false false false))
