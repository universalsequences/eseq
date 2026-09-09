(capture-project
  (track :instrument "core/drift"))

(def capture-after-sync ()
  (eseq.transport/open-application-menu "Edit" (dict :col 8 :row 2)))
