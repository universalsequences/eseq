;; Loadable project macro-mapping playground.
;;
;; 1. Load this script from the script browser.
;; 2. Open the Player tab and click a macro's map button.
;; 3. In the FX panel, click any green instrument/effect parameter to map it.
;; 4. Sweep a knob for a latched value, or hold/release the momentary button.
;; 5. Edit mapping ranges/curves or remove mappings in the embedded editor or
;;    in the Macro Mappings sidebar.
;;
;; Re-loading the script is safe: stable keys reuse the same project macros and
;; preserve their values, mappings, ranges, and names.

(eseq.macros/macro-ensure :delay-push "Delay Push")
(eseq.macros/macro-ensure :space "Space")
(eseq.macros/macro-ensure :texture "Texture")

(def macro-player-control (key)
  (box
    :width 10.6 :height 5.8 :padding 0.55
    :background-color :mixer-strip-bg
    :border-color :mixer-strip-border
    :corner-radius 14
    (v-stack :gap 0.65 :align :center
      (eseq.macros/macro-knob :macro key)
      (h-stack :gap 0.4 :align :center
        (eseq.macros/macro-momentary :macro key)
        (eseq.macros/macro-map-button :macro key)))))

(def macro-player-surface ()
  (box
    :width 52 :height 22 :padding 1.0
    :background-color :buffer-bg
    (v-stack :gap 0.65
      (label "PLAYER SURFACE" :key "macro-player-title"
        :width 48 :height 1.2 :font-size 12 :color :foreground :bg :transparent)
      (label "Knob = latched · hold = momentary push · map = choose targets"
        :width 48 :height 1.0 :font-size 9 :color :dim :bg :transparent)
      (label "Mapped controls lock · edit ranges and curves below"
        :width 48 :height 1.0 :font-size 8 :color :dim :bg :transparent)
      (h-stack :gap 0.8 :align :top
        (macro-player-control :delay-push)
        (macro-player-control :space)
        (macro-player-control :texture))
      (eseq.macros/macro-mapping-editor :macro :delay-push))))

(effect-buffer "*macro-player*" (macro-player-surface))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab "Player" "*macro-player*" "" "")
