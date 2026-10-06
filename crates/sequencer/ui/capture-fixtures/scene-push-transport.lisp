(capture-project
  (track :sampler :name "Drums"))

(def capture-after-sync ()
  (let ((push eseq.transport/scene-push))
    (set! push.target 0)
    (set! push.value 1.0)))
