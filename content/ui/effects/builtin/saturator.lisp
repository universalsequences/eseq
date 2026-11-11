;; Saturator built-in FX panel.
;;
;; Saturator is Roar's engine pinned to one stage (effects/mod.rs
;; builtin_saturator), so the panel is Roar's shaper view plus six knobs:
;; the transfer curve with its live drive region and curve picker on the
;; left, Drive / Amount / Bias over Tone / Output / Dry-Wet and the Auto Gain
;; switch on the right. The default curve, "triode", and the four after it
;; are solved circuits (effects/triode.rs and its siblings); the display shows
;; each one's memoryless transfer.

(module eseq.effects.builtin.saturator)

(import eseq.effects.builtin.filter-core :refer (builtin-fx-param))
(import eseq.effects.builtin.roar :as roar)
(import eseq.effects.param-controls :refer
  (param-effective-ratio
   param-effective-value))
(import eseq.effects.param-grid :refer (fx-param-grid))

(export panel)

(def curve-box (fx curve-p amount-p bias-p)
  (box :width 12.4 :height 9.70 :padding 0.36
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.24 :align :center
      (roar-shaper
        :width 11.6 :height 7.0
        :source (roar/effect-source fx)
        :stage 0
        ;; Effective-value bindings so the curve follows knob drags and
        ;; modulation without a panel rebuild (see roar.lisp shaper-view).
        :shaper (eseq.effects.param-controls/param-effective-value curve-p)
        :amount (eseq.effects.param-controls/param-effective-ratio amount-p)
        :bias (eseq.effects.param-controls/param-effective-ratio bias-p))
      (subtree :key "saturator-curve-option"
        (roar/option fx curve-p 10.0)))))

(def knob-box (fx drive-p amount-p bias-p tone-p output-p mix-p auto-p)
  (box :width 15.0 :height 9.70 :padding 0.36
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.62 :align :center
      (box :height 0.10)
      (h-stack :gap 0.30 :align :start
        (roar/parameter-knob fx "drive" drive-p 1)
        (roar/percent-knob fx "amount" amount-p)
        (roar/parameter-knob fx "bias" bias-p 2))
      (h-stack :gap 0.30 :align :start
        (roar/percent-knob fx "tone" tone-p)
        (roar/parameter-knob fx "output" output-p 1)
        (roar/percent-knob fx "dry/wet" mix-p))
      ;; Level-matches the output as drive and amount go up.
      (roar/parameter-toggle fx auto-p "Auto Gain" 5.6))))

(def panel (fx)
  (let ((params (get fx :params)))
    (let ((drive-p (eseq.effects.builtin.filter-core/builtin-fx-param params "drive"))
          (curve-p (eseq.effects.builtin.filter-core/builtin-fx-param params "curve"))
          (amount-p (eseq.effects.builtin.filter-core/builtin-fx-param params "amount"))
          (bias-p (eseq.effects.builtin.filter-core/builtin-fx-param params "bias"))
          (tone-p (eseq.effects.builtin.filter-core/builtin-fx-param params "tone"))
          (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output"))
          (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "dry/wet"))
          (auto-p (eseq.effects.builtin.filter-core/builtin-fx-param params "auto gain")))
      (if (and drive-p curve-p amount-p bias-p tone-p output-p mix-p auto-p)
        (h-stack :gap 0.35 :align :start
          (curve-box fx curve-p amount-p bias-p)
          (knob-box fx drive-p amount-p bias-p tone-p output-p mix-p auto-p))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
