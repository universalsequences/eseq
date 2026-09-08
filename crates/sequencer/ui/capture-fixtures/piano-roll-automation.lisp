;; Piano roll with the automation lane (bead eseq-2k9p.21): the velocity
;; lane under the note grid, one dot + duration bar per note, aligned with
;; the timeline's time axis.

(capture-project
  (track :sampler :name "T1" :steps (0 (2 3) 4 (6 -2) 8 12)))

(def capture-after-sync ()
  (eseq.seq-panels/seq-open-piano-roll-bottom-for-track 0))
