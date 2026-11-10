;; Audio fades capture fixture (docs/audio-tracks-spec.md §8.1): a tall
;; audio lane holding a stereo crash (drawn as two waveforms) with a curved
;; fade-in, crossfading into a snare that fades out, above a collapsed
;; audio lane and a stock pattern lane.

(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :audio :name "Crash"
    :samples ("../../../../content/packages/universalsequences.factory-samples/samples/vcsl/cymbal_crash1_mf1.wav"
              "../../../../content/packages/universalsequences.factory-samples/samples/vcsl/Snare2_HitSN_v3_rr1_Mid.wav"))
  (track :audio :name "Tubby"
    :samples ("../../../../content/impulses/prepared/king-tubby.wav")))

(def fade! (id handle value)
  (host-command "audio-region-fade" (dict :id id :handle handle :value value)))

(def capture-after-sync ()
  (do
    (eseq.seq-panels/seq-open-arrangement)
    (let ((t (eseq.kinds/track 1))
          (a (first t.audio-regions))
          (b (nth t.audio-regions 1)))
      ;; 120 bpm: four seconds of the crash end at beat 8; the snare butts
      ;; up against it there.
      (host-command "audio-region-set-window" (dict :id a.cid :offset 0 :length 4))
      (host-command "audio-region-move" (dict :id b.cid :start 8))
      (fade! a.cid :in 2)
      (fade! a.cid :in-curve 0.8)
      ;; The crash's fade-out dragged a beat past its end crossfades into
      ;; the snare over the crash's own tail.
      (fade! a.cid :out 9.5)
      (fade! b.cid :out 10.2)
      (fade! b.cid :out-curve 0.2))
    (let ((tubby (eseq.kinds/track 2))
          (h (eseq.arrangement/arr-lane tubby.tid)))
      (set! h.collapsed true))
    ;; Zoomed in on the two regions.
    (let ((v eseq.arrangement/arr-view))
      (set! v.duration 14))))
