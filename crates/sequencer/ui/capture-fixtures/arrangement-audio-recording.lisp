;; Audio recording capture fixture (docs/audio-tracks-spec.md §12.8): an
;; audio track mid-take. Its header shows the input row (input picker,
;; monitor mode lit while armed, input meter), and its lane a region beside
;; the growing "Recording" item drawing the take's waveform so far. (The
;; header's arm dot stays dark here: the capture harness does not run the
;; tick that copies the engine's arm flags to the UI.)

(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :audio :name "Guitar"
    :samples ("../../../../content/impulses/prepared/king-tubby.wav")
    :recording 6))

(def capture-after-sync ()
  (eseq.seq-panels/seq-open-arrangement))
