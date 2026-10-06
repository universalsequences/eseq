;; Export settings over the production sequencer panel, with deterministic
;; saved-project metadata so this fixture never writes user recordings.
(capture-project (track :sampler :name "Sampler"))
(def capture-after-sync ()
  (present-fixture "song-export"
    (dict :default-name "Night Drive (2)"
          :output-name "Night Drive (2).wav"
          :project "Night Drive"
          :folder "recordings"
          :end 64
          :percent -1
          :busy false
          :done false
          :message ""
          :reveal-label "Show in Finder"))
  (eseq.export-song/reset)
  (set! eseq.export-song/range-draft "Beat range")
  (eseq.export-song/open))
