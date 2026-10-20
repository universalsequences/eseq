;; *step* buffer root. Loaded by ui/main.lisp after sequencer helpers. The
;; body is eseq.effects.buffers/step-buffer-body, so a package can override it.

(effect-buffer "*step*"
  (eseq.effects.buffers/step-buffer-body))

(set-buffer-mode-for "*step*" "eseq.effects.buffers/seq-plock-panel-mode")
