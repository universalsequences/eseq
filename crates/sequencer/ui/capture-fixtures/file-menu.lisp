;; The toolbar File menu open over the transport.
(capture-project
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (eseq.transport/open-file-menu (dict :col 14 :row 2)))
