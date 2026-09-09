;; File > New Project with unsaved changes: Save / Don't Save / Cancel.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (eseq.file-dialogs/open-unsaved-prompt))
