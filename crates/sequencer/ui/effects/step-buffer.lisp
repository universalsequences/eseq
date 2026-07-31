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
        (fx-step-parameters-panel)
        (fx-track-plocks-panel)))))

(set-buffer-mode-for "*step*" "seq-plock-panel-mode")
