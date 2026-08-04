;; *step* buffer root. Loaded by ui/main.lisp after sequencer helpers.

(effect-buffer "*step*"
  (if (= SEQ.num-tracks 0)
    (fx-empty-track-fallback)
    (box :padding 0.5
      (v-stack :gap 0.1
        ;; Sound palette overlay (takes spec 17.6), opened from the
        ;; instrument panel's binding badge. Collapses to nothing while
        ;; closed.
        (subtree :key "step-sound-palette"
          (sound-palette-panel))
        ;; Each panel is its own subtree so selection/p-lock updates rerun
        ;; only the panel that reads them instead of the whole buffer.
        (subtree :key "step-parameters-panel"
          (fx-step-parameters-panel))
        (subtree :key "step-track-plocks-panel"
          (fx-track-plocks-panel))))))

(set-buffer-mode-for "*step*" "seq-plock-panel-mode")
