;; Audio effect modulation source selection and editor controls.
(module eseq.effects.effect-modulation)

(import eseq.effects.state :as st :refer (effect-selected-mod-slot))
(import eseq.effects.param-controls :as pc)
(import eseq.effects.param-grid :as pg)
(import eseq.effects.instrument-modulation :as im)
(import eseq.effects.panel-frame :as pf)
(import eseq.effects.custom-ui-lego :as lego)

(export mod-control-panel)


(def set-selected-mod-slot (slot)
  (set! eseq.effects.state/effect-selected-mod-slot slot))

(def mod-selector-row (fx modulator)
  (let ((slot (get modulator :slot))
        (label-text (if (get modulator :name) (get modulator :name) (get modulator :label)))
        (source-p (get modulator :source-param)))
    (subtree :key (str "effect-mod-selector-" (pf/fx-effect-chain-kind fx) "-" (get fx :bus-idx) "-" (get fx :slot-idx) "-" slot)
      (h-stack :gap 0.18 :align :center
        (button label-text
          :width 3.9 :height 1.1
          :padding 0
          :font-size 9
          :background-color (if (= eseq.effects.state/effect-selected-mod-slot slot)
            (rgba 0.95 0.48 0.18 0.82)
            :instrument-control-bg)
          :color (if (= eseq.effects.state/effect-selected-mod-slot slot) :white :dim)
          :on-click (lambda (info) (set-selected-mod-slot slot)))
        (dropdown :value (if source-p (get source-p :text-value) "off")
          :options (if source-p (get source-p :options) '())
          :on-change (lambda (v) (if source-p (pc/param-set-option fx source-p v) false))
          :width 4.8 :height 1.1 :font-size 8.5)))))

(def mod-selector (fx)
  (box :debug-name "effect-mod-selector"
       :width 9.4
       :height :fill
       :padding 0.25
    (v-stack :gap 0.18 :align :start
      (label "mods" :font-size 9 :color :dim :bg :transparent)
      (v-stack :gap 0.18 :align :start
        (each (get fx :sources) |modulator mi|
          (mod-selector-row fx modulator))))))

(def selected-mod-source-section (fx)
  (nth (filter |section| (= (get section :slot) eseq.effects.state/effect-selected-mod-slot)
         (get fx :sources))
       0))

(def source-param (section name)
  (find-by-key (get section :params) :name name))

(def source-param-value (fx p fallback)
  (if p (pc/fx-param-value-for fx p) fallback))

(def source-set-param-value (fx p v)
  (if p (pc/param-set-control-value fx p v) false))

(def source-button (fx p title width)
  (let ((active (> (reactive-value (source-param-value fx p 0)) 0.5)))
    (v-stack :width width :height 1.72 :gap 0.10 :align :start
      (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
      (button (if active "ON" "OFF")
        :width width :height 0.88 :padding 0 :font-size 9
        :background-color (if active (lego/ui-accent-orange) :mixer-control-bg)
        :color (if active :black :dim)
        :on-click |x y r|
          (source-set-param-value fx p (if active 0 1))))))

(def source-dropdown (fx p title width)
  (v-stack :width width :height 1.72 :gap 0.10 :align :start
    (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
    (dropdown :value (if p (get p :text-value) "")
      :options (if p (get p :options) '())
      :on-change (lambda (v) (if p (pc/param-set-option fx p v) false))
      :width width :height 0.88 :font-size 8.5)))

(def source-number (fx p title decimals unit width)
  (v-stack :width width :height 1.72 :gap 0.10 :align :start
    (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
    (number-picker :value (source-param-value fx p 0)
      :min (if p (pc/param-control-min fx p) 0)
      :max (if p (pc/param-control-max fx p) 0)
      :decimals decimals
      :unit unit
      :noui true :font-size 9.3
      :text-color :dim :edit-color :yellow
      :text-align :left
      :width width :height 0.82
      :on-change (lambda (v) (source-set-param-value fx p v)))))

(def source-compact-knob (fx p title decimals)
  (box :debug-name (str "effect-source-compact-knob-" title)
       :width 4.4 :height 2.25 :padding 0
    (knob-number :label title
      :value (source-param-value fx p 0)
      :min (if p (pc/param-control-min fx p) 0)
      :max (if p (pc/param-control-max fx p) 0)
      :decimals decimals
      :font-size 9.4 :label-font-size 8.2
      :text-color :dim :label-color :dim
      :width 4.4 :height 2.05
      :on-change (lambda (v) (source-set-param-value fx p v)))))

(def lfo-source-editor (fx section)
  (let ((rate (source-param section "rate"))
        (sync (source-param section "sync"))
        (division (source-param section "division"))
        (shape (source-param section "shape"))
        (pulse-width (source-param section "pulse width"))
        (retrigger (source-param section "retrigger")))
    (lego/ui-readout-panel-medium-s 0
      (h-stack :debug-name "effect-lfo-source-editor"
               :width :fill :height :fill :gap 0.38 :align :start
        (v-stack :width 13.8 :height :fill :gap 0.12 :align :start
          (h-stack :gap 0.25 :align :start
            (source-number fx rate "rate" 2 false 6.4)
            (source-button fx sync "sync" 5.0))
          (h-stack :gap 0.25 :align :start
            (source-dropdown fx division "division" 6.4)
            (source-dropdown fx shape "shape" 5.0)))
        (v-stack :width 5.0 :height :fill :gap 0.18 :align :center
          (source-compact-knob fx pulse-width "pw" 2)
          (box :debug-name "effect-lfo-retrigger-button"
               :width 4.4 :height 1.55 :padding 0
            (source-button fx retrigger "retrig" 4.4)))))))

(def selected-mod-source-editor (fx)
  (box :debug-name "effect-selected-mod-source-editor"
       :width 25.5
       :height :fill
       :padding 0.35
    (let ((section (selected-mod-source-section fx)))
      (if section
        (let ((source-type (im/source-type section)))
          (v-stack :width :fill :height :fill :gap 0.3 :align :start
            (label (get section :name) :font-size 9 :color :dim :bg :transparent)
            (if (= source-type "lfo")
              (lfo-source-editor fx section)
              (if (or (= source-type "rand") (= source-type "drift"))
                (pg/fx-param-grid (get section :params) fx)
                (box :width :fill :height 6 :h-align :center :v-align :center
                  (label "no source controls" :font-size 12 :color :dim :bg :transparent))))))
        (box :width :fill :height :fill :h-align :center :v-align :center
          (label "no source controls" :font-size 12 :color :dim :bg :transparent))))))

(def mod-control-panel (fx)
  (box :debug-name "effect-mod-control-panel"
       :width 36.4
       :height st/fx-panel-body-content-height
       :background-color :black
       :corner-radius 10
       :padding 0.25
    (h-stack :height :fill :gap 0.25 :align :stretch
      (mod-selector fx)
      (selected-mod-source-editor fx))))
