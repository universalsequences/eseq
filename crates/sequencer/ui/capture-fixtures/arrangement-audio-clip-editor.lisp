;; Audio clip editor capture fixture (docs/audio-tracks-spec.md §8.2): the
;; audio-track fixture with its first region double-clicked, so the lower
;; panel shows the region's waveform between its start and end markers.

(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :audio :name "Audio"
    :samples ("../../../../content/impulses/prepared/king-tubby.wav")))

(def capture-after-sync ()
  (do
    (eseq.seq-panels/seq-open-arrangement)
    (let ((t (eseq.kinds/track 1))
          (r (first t.audio-regions)))
      (eseq.arrangement/audio-lane-action 1
        (dict :type :double-click-item :ids (list r.cid) :time 0)))))
