;; Deterministic keys-tab capture with a C major triad marked active.
(capture-project
  (track :instrument "core/drift"))

;; The headless capture has no running note source. Override only the activity
;; source so the production keys widget can be inspected in its lit state, with
;; two keys selected to show the selection tint.
(def eseq.effects.panel-bodies/instrument-key-active-notes (inst)
  '(60 64 67))

(def capture-after-sync ()
  (do
    (set! eseq.effects.state/instrument-panel-tab 1)
    (set! eseq.effects.state/instrument-key-lock-selected-notes '(62 69))))
