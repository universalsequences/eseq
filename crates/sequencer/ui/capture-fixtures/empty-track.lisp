;; An editable track without a sample buffer, voice pool, or instrument engine.
(capture-project
  (track :empty :steps (0 (4 7) (8 12))))

(def capture-after-sync ()
  (eseq.sequencer/select-track-for-edit 0)
  (eseq.browser/open-device-picker))
