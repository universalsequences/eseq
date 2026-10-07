(do
  (eseq.seq-layout/apply-fx-layout)
  (seq-apply-fx-layout-extra)
  eseq.effects.effect-panels/effect-mods-toggle-button
  eseq.seq-core-state/current-step
  (set! eseq.seq-core-state/current-step 9)
  ; eseq.seq-layout/apply-fx-layout
  "seq-apply-fx-layout"
  'current-step)
