;; Audio track capture fixture (docs/audio-tracks-spec.md §8.1): a sampler
;; track above an audio track holding two dropped samples laid end to end,
;; so the view shows waveform regions beside MIDI clips.

(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :audio :name "Audio"
    :samples ("../../../../content/impulses/prepared/king-tubby.wav"
              "../../../../content/packages/universalsequences.factory-samples/samples/vcsl/Snare2_HitSN_v3_rr1_Mid.wav")))

(def capture-after-sync ()
  (eseq.seq-panels/seq-open-arrangement))
