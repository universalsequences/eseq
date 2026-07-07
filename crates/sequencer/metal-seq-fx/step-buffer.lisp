;; *step* buffer root. Loaded by metal-seq-grid.lisp after sequencer helpers.

(effect-buffer "*step*"
  (if (= SEQ.num-tracks 0)
    (fx-empty-track-fallback)
    (box :padding 0.5
      (v-stack :gap 0.1
        (fx-step-parameters-panel)
        (fx-track-plocks-panel)))))

(set-buffer-mode-for "*step*" "seq-plock-panel-mode")
