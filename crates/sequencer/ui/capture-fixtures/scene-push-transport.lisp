(capture-project
  (track :sampler :name "Drums"))

(def capture-after-sync ()
  (set! eseq.transport/scene-push-target 0)
  (set! eseq.transport/scene-push-value 1.0))
