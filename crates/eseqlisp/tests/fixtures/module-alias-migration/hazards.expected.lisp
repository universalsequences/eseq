(do
  (eseq.seq-layout/apply-fx-layout)
  (seq-apply-fx-layout-extra)
  eseq.effects.state/effect-mods-open
  eseq.seq-core-state/current-step
  (set! eseq.seq-core-state/current-step 9)
  ; eseq.seq-layout/apply-fx-layout
  "seq-apply-fx-layout"
  'current-step)
