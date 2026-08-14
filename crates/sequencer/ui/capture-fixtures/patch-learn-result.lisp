;; Patch Learn result state with visible parameter travel.
(capture-project
  (track :instrument "core/drift"))

(effect-buffer "*patch-learn-result*"
  (box :width :fill :height :fill :background-color :buffer-bg :padding 0.55
    (eseq.patch-learn/result-panel
      "samples/drums/kick.wav"
      "Studio Kick"
      27.3
      3.688417
      "ok"
      (list
        (dict :name "amp_release" :from 45 :to 1500 :change 1455)
        (dict :name "cutoff" :from 520 :to 1125.7611 :change 605.7611)
        (dict :name "env_mod" :from 3100 :to -1317.016 :change -4417.016)
        (dict :name "resonance" :from 0.2 :to 0.4831 :change 0.2831))
      ".eseq/learn-jobs/example/seeded.wav"
      ".eseq/learn-jobs/example/final.wav"
      false)))
