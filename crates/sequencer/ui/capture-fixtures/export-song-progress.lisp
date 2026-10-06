;; Running export: no worker is started and no recording is written.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (present-fixture "song-export"
    (dict :project "Night Drive"
          :output-name "Night Drive (2).wav"
          :folder "recordings"
          :busy true
          :done false
          :percent 37
          :message "Exporting audio — 37%"))
  (eseq.export-song/open))
