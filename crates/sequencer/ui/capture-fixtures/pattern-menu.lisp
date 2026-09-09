(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (eseq.transport/open-application-menu "Pattern" (dict :col 8 :row 2)))
