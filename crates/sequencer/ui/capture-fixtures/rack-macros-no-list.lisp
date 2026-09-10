;; The macro view retains the rack title and preset save control without the list.
(capture-project
  (track :layer-rack :name "Macro Rack"))

(def capture-after-sync ()
  (do
    (set! eseq.effects.state/rack-panel-slot-list-open false)
    (set! eseq.effects.state/rack-panel-selected-chain-open false)
    (set! eseq.effects.state/rack-panel-macros-open true)))
