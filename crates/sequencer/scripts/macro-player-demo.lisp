;; Loadable project macro-mapping playground.
;;
;; 1. Load this script from the script browser.
;; 2. Open the Player tab and click a macro's map button.
;; 3. In the FX panel, click any green instrument/effect parameter to map it.
;; 4. Edit mapping ranges or remove mappings in the Macro Mappings sidebar.
;;
;; Re-loading the script is safe: stable keys reuse the same project macros and
;; preserve their values, mappings, ranges, and names.

(macro-ensure :delay-push "Delay Push")
(macro-ensure :space "Space")
(macro-ensure :texture "Texture")

(def macro-player-control (key)
  (box
    :width 8.4 :height 5.4 :padding 0.55
    :background-color :mixer-strip-bg
    :border-color :mixer-strip-border
    :corner-radius 14
    (v-stack :gap 0.65 :align :center
      (macro-knob :macro key)
      (macro-map-button :macro key))))

(def macro-player-surface ()
  (box
    :width 38 :height 15 :padding 1.0
    :background-color :buffer-bg
    (v-stack :gap 0.65
      (label "PLAYER SURFACE" :key "macro-player-title"
        :width 34 :height 1.2 :font-size 12 :color :foreground :bg :transparent)
      (label "Click map, then click a green parameter in the FX panel."
        :width 34 :height 1.0 :font-size 9 :color :dim :bg :transparent)
      (label "Mapped controls lock · edit ranges in the sidebar"
        :width 34 :height 1.0 :font-size 8 :color :dim :bg :transparent)
      (h-stack :gap 0.8 :align :top
        (macro-player-control :delay-push)
        (macro-player-control :space)
        (macro-player-control :texture)))))

(effect-buffer "*macro-player*" (macro-player-surface))
(seq-register-script-step-sequencer-tab "Player" "*macro-player*" "" "")
