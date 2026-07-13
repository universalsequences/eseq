;; Experimental project-owned macro player surface.
;; Re-evaluating these declarations reuses the same stable-keyed macros.

(macro-ensure :delay-push "Delay Push")
(macro-ensure :space "Space")
(macro-ensure :texture "Texture")

(def macro-player-control (key)
  (box
    :width 8.4 :height 7.0 :padding 0.55
    :background-color :mixer-strip-bg
    :border-color :mixer-strip-border
    :corner-radius 14
    (v-stack :gap 0.55 :align :center
      (macro-knob :macro key)
      (macro-map-button :macro key))))

(def macro-player-surface ()
  (box
    :width 34 :height 12 :padding 1.0
    :background-color :buffer-bg
    (v-stack :gap 0.8
      (label "PLAYER SURFACE" :key "macro-player-title"
        :width 30 :height 1.2 :font-size 12 :color :foreground :bg :transparent)
      (label "project macros · script-authored controls"
        :width 30 :height 1.0 :font-size 9 :color :dim :bg :transparent)
      (h-stack :gap 0.8 :align :top
        (macro-player-control :delay-push)
        (macro-player-control :space)
        (macro-player-control :texture)))))

(effect-buffer "*macro-player*" (macro-player-surface))
(seq-register-script-step-sequencer-tab "Player" "*macro-player*" "" "")
