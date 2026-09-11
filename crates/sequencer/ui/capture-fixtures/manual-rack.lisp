;; Manual illustration; real project state through the production capture path.
(capture-project
  (track :layer-rack :name "Layer Rack"
    :samples ("../../../../content/packages/universalsequences.factory-samples/samples/vcsl/Snare2_HitSN_v7_rr1_Mid.wav"
              "../../../../content/packages/universalsequences.factory-samples/samples/vcsl/Claves1_Hit_v2_rr1_Mid.wav"))
  (rack-slot-macro 0 0 0 gain 0.5 1.5))
(def capture-after-sync ()
  (set! eseq.effects.state/rack-panel-slot-list-open true)
  (set! eseq.effects.state/rack-panel-selected-chain-open false)
  (set! eseq.effects.state/rack-panel-macros-open true))
