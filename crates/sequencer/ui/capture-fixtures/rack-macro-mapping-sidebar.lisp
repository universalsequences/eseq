;; Rack macro mapping-mode regression fixture. It uses the production sidebar
;; hooks so the capture proves that arming (not disarming) mounts rack details.
(capture-project
  (track :layer-rack
    :name "Macro Rack"
    :samples ("../../assets/ir/lexicon-300-rich-plate.wav")))

(def capture-after-sync ()
  (do
    (eseq.effects.state/rack-panel-set-view
      (get (nth SEQ.instrument-panel 0) :track-id) false true false)
    (eseq.effects.instrument-panel/rack-macro-arm
      (nth (get (nth SEQ.instrument-panel 0) :macros) 0))))
