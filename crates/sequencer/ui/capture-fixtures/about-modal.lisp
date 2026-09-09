;; The File > About modal over the production sequencer panel.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (eseq.file-dialogs/open-about "0.1.0"))
