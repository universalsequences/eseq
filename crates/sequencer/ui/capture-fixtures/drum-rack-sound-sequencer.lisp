;; Drum-rack Sound-mode sequencer capture with occupied pads, exercising the
;; abbreviated vertical sound labels used by the expanded step controls.
(capture-project
  (track :drum-rack :name "Drum Rack" :num-steps 16
    :samples (
      "../../assets/ir/lexicon-300-rich-plate.wav"
      "../../assets/ir/lexicon-300-rich-plate.wav")))

(def capture-after-sync ()
  (do
    (eseq.sequencer/track-menu-click 0)
    (eseq.sequencer/set-track-param-mode (nth SEQ.track-ids 0) 3)))
