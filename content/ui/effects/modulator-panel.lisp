;; Modulator instrument panel controls.
(module eseq.effects.modulator-panel)

(import eseq.effects.state :as st)
(import eseq.effects.param-controls :as pc)
(import eseq.effects.effect-panels :as ep)
(import eseq.effects.panel-frame :as pf)

(export modulator-panel)

;; No compat aliases: the only external caller of `modulator-panel` is the
;; converted eseq.effects.instrument-panel, which imports this module. Every
;; Rust mention of "modulator-panel" is a :debug-name string, not a call.
;; `ui-accent-orange` stays bare: owned by effects/custom-ui-lego.lisp,
;; reached through its identity compat alias while that file converts.

(def modulator-param (inst name)
  (find-by-key (get inst :synth) :name name))

(def modulator-knob (p label-text key)
  (subtree :key key
    (knob-number :label label-text
      :value (pc/fx-param-value p)
      :min (pc/instrument-param-control-min p) :max (pc/instrument-param-control-max p) :decimals 0
      :font-size 12 :label-font-size 11
      :text-color :dim :label-color :dim
      :track-color :dim
      :width 7.0 :height 4.15 :knob-size 2.55
      :value-align :center
      :on-change (lambda (v) (pc/instrument-set-param-control-value p v)))))

(def modulator-panel (inst)
  (let ((rise-p (modulator-param inst "rise"))
      (fall-p (modulator-param inst "fall")))
    (box :background "fx-panel-bg" :color :instrument-panel-bg :header :fx-panel-header-bg :selected-header :fx-panel-header-selected-bg :selected 0 :padding 0
      :height st/fx-fixed-panel-height
      :debug-name "modulator-panel"
      (v-stack :gap 0 :height :fill
        (box :debug-name "modulator-header-box" :width :fill :height st/fx-panel-header-height :padding 0 :v-align :center :h-align :start
          (h-stack :gap 0.5 :align :center :width :fill
            (pf/fx-panel-header-leading-spacer)
            (ep/enabled-toggle (ep/enabled-param (get inst :synth)) false "modulator-enabled")
            (label "Modulator" :font-size 11 :color :white :bg :transparent)
            (box :flex 1 :height 0.15)
            (pf/instrument-header-actions-menu inst)))
        (box :height 0.2)
        (pf/fx-panel-body "modulator-panel-content"
          (box

            :padding 0.15
            :debug-name "modulator-panel-body"
            (box

              :background-color :mixer-strip-bg
              :corner-radius 16
              :padding 1
              :width :fill
              :height 9.5
              (h-stack :width :fill :height :fill :gap 1.05 :align :center
                (if rise-p
                  (modulator-knob rise-p "rise ms" "modulator-rise-knob")
                  (label "missing: rise" :font-size 10 :color :red :bg :transparent))
                (if fall-p
                  (modulator-knob fall-p "fall ms" "modulator-fall-knob")
                  (label "missing: fall" :font-size 10 :color :red :bg :transparent))
                (box :width 0.45 :height 1)
                (box :width 12.8 :height 5.9 :padding 0.22
                  :background-color :black
                  :corner-radius 8
                  :debug-name "modulator-curve-wrapper"
                  (modulator-curve
                    :width 12.25 :height 5.45
                    :rise (if rise-p (pc/fx-param-value rise-p) 0)
                    :fall (if fall-p (pc/fx-param-value fall-p) 0)
                    :phase (bind-seq (get inst :phase-field))
                    :level (bind-seq (get inst :level-field))
                    :max-ms (if rise-p (get rise-p :max) 5000)
                    :background-color :instrument-control-bg
                    :grid-color :dim
                    :curve-color (eseq.effects.custom-ui-lego/ui-accent-orange)
                    :fill-color (rgba 1.0 0.48 0.18 0.16)))))))))))
