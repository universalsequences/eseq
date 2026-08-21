;; *step* buffer root. Loaded by ui/main.lisp after sequencer helpers.

(effect-buffer "*step*"
  (if (= SEQ.num-tracks 0)
    (eseq.effects.buffers/empty-track-fallback)
    (box :padding 0.5
      (v-stack :gap 0.1
        ;; Sound palette overlay (takes spec 17.6), opened from the
        ;; instrument panel's binding badge. Collapses to nothing while
        ;; closed.
        (subtree :key "step-sound-palette"
          (eseq.sound-palette/panel))
        ;; Each panel is its own subtree so selection/p-lock updates rerun
        ;; only the panel that reads them instead of the whole buffer.
        (subtree :key "step-parameters-panel"
          (eseq.effects.track-panels/step-parameters-panel))
        (subtree :key "step-track-plocks-panel"
          (eseq.effects.track-panels/track-plocks-panel))))))

(set-buffer-mode-for "*step*" "eseq.effects.buffers/seq-plock-panel-mode")
