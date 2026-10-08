;; The macro view retains the rack title and preset save control without the list.
(capture-project
  (track :layer-rack :name "Macro Rack"))

(def capture-after-sync ()
  (do
    (eseq.effects.state/rack-panel-set-view
      (get (eseq.effects.panel-data/current-instrument-panel) :track-id) false true false)))
