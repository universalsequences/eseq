(capture-project
  (track :sampler :name "Transpose"))

(def capture-after-sync ()
  (set! eseq.transport/scene-transpose -9))
