(capture-project
  (track :sampler :name "Transpose"))

(def capture-after-sync ()
  (do
    (set! eseq.transport/scene-transpose -9)
    (eseq.transport/open-transpose-menu (dict :col 65 :row 3))))
