;; Rack-owned macro bank regression fixture. The slot list is closed so the
;; toolbar toggle and the complete 4x2 public macro surface are easy to inspect.
(capture-project
  (track :layer-rack
    :name "Macro Rack"
    :samples ("../../../../content/impulses/lexicon-300-rich-plate.wav")
    :rack-slot-audio-fx ("OTT"))
  (rack-slot-macro 0 0 0 gain 0.5 1.5))

(def capture-after-sync ()
  (eseq.effects.state/rack-panel-set-view
      (get (nth SEQ.instrument-panel 0) :track-id) false true false))
