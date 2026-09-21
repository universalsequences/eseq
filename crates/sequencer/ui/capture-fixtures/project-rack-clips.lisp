;; Use --project PATH to inspect real saved rack clip banks in both the
;; sequencer header and mixer. The host supplies all project and script state.
(capture-project)

(def capture-after-sync ()
  (setopt eseq.seq-core-state/mixer-show-clip-grid true))
