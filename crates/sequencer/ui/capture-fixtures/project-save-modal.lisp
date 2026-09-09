;; The File > Save As name modal over the production sequencer panel.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (eseq.file-dialogs/open-save "Save project as" "Night Drive"))
