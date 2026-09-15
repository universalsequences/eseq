;; Manual illustration; real project state through the production capture path.
(capture-project
  (track :layer-rack :name "Layer Rack"
    :samples ("../../../../content/packages/universalsequences.factory-samples/samples/vcsl/Snare2_HitSN_v7_rr1_Mid.wav"
              "../../../../content/packages/universalsequences.factory-samples/samples/vcsl/Claves1_Hit_v2_rr1_Mid.wav"))
  (rack-slot-macro 0 0 0 gain 0.5 1.5))
(def capture-after-sync ()
  (eseq.effects.state/rack-panel-set-view
      (get (nth SEQ.instrument-panel 0) :track-id) true true false))
