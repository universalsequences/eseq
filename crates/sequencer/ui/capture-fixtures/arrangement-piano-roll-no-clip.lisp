;; Arrangement lower-panel capture with no clip selected. This exercises the
;; explicit arrangement piano-roll mode independently of the session/live
;; piano-roll fallback.

(capture-project
  (track :sampler :name "Empty arrangement track"))

(def capture-after-sync ()
  (eseq.seq-panels/seq-open-arrangement-piano-roll-bottom-for-track 0))
