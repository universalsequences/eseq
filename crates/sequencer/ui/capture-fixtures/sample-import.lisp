;; Sample import modal (ui/sample-import.lisp) over the *sequencer* buffer,
;; staged from the factory impulse folder so the fixture needs no user files.
;; The "prepared" subfolder is selected in the tree and bulk-tagged; the
;; dropped folder itself ("impulses") shows up as a one-click batch suggestion.
(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (seq-sample-import-stage (list (seq-factory-path "impulses")))
  (eseq.sample-import/open)
  (eseq.sample-import/select-node "impulses/prepared")
  (eseq.sample-import/add-selection-tag "plate")
  (eseq.sample-import/select-node "impulses/prepared/yamaha-g5.wav"))
