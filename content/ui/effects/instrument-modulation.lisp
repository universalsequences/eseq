;; Instrument modulation source selection and editor controls.
(module eseq.effects.instrument-modulation)

(import eseq.effects.state :refer (instrument-selected-mod-slot))
(import eseq.effects.param-controls :as pc)
(import eseq.effects.param-grid :as pg)
(import eseq.effects.custom-ui-lego :as lego)

(export source-type
        mod-control-panel)

;; Migration alias (module spec §10): the unconverted panel-bodies.lisp and
;; sampler-panel.lisp call the panel entry point by its old flat name.
;; Converted callers (effect-modulation) import this module instead.

(def mod-base-name (name)
  (if (string-ends-with? name " amt")
    (substring name 0 (- (len name) 4))
    (if (string-ends-with? name " src")
      (substring name 0 (- (len name) 4))
      name)))

(def mod-source-param (params base)
  (find-by-key params :name (str base " src")))

(def mod-amount-params (params)
  (filter |p| (string-ends-with? (get p :name) " amt") params))

(def mod-row (params amount-p subtree-key)
  (let ((base (mod-base-name (get amount-p :name)))
        (source-p (mod-source-param params base)))
    (subtree :key subtree-key
      (box :width 12.6 :height 2.35
           :background-color :instrument-group-bg
           :border-width 1
           :corner-radius 16
           :padding 0.25
        (h-stack :width :fill :gap 0.45 :align :center
          (if source-p
            (dropdown :value (pc/fx-param-text-value-for false source-p)
              :options (get source-p :options)
              :on-change (lambda (v) (pc/fx-set-instrument-option source-p v))
              :width 4.8 :height 1.15 :font-size 10)
            (box :width 5.3 :height 1.15))
          (knob-number :label (substring base 0 12)
            :value (pc/fx-param-value amount-p)
            :min (get amount-p :min) :max (get amount-p :max) :decimals 2
            :font-size 10.5 :label-font-size 9
            :text-color :dim :label-color :dim
            :width 5.2 :height 2.05
            :on-change (lambda (v) (pc/fx-set-instrument-value amount-p v))))))))

(def mod-grid (params)
  (let ((amounts (mod-amount-params params)))
    (h-stack :gap 0.45 :padding 0
      (each (chunks amounts 3) |chunk ci|
        (v-stack :gap 0.18
          (each chunk |p pi|
            (mod-row params p
              (str "instrument-mod-row-" ci "-param-" (get p :idx)))))))))

(def mod-selector-row (modulator)
  (let ((slot (get modulator :slot))
        (label-text (if (get modulator :name) (get modulator :name) (get modulator :label)))
        (source-p (get modulator :source-param)))
    (subtree :key (str "instrument-mod-selector-" slot)
      (h-stack :gap 0.18 :align :center
        (button label-text
          :width 3.9 :height 1.1
          :padding 0
          :font-size 9
          :background-color (if (= eseq.effects.state/instrument-selected-mod-slot slot)
            (rgba 0.95 0.48 0.18 0.82)
            :instrument-control-bg)
          :color (if (= eseq.effects.state/instrument-selected-mod-slot slot) :white :dim)
          :on-click (lambda (info) (set! eseq.effects.state/instrument-selected-mod-slot slot)))
        (dropdown :value (if source-p (pc/fx-param-text-value-for false source-p) "off")
          :options (if source-p (get source-p :options) '())
          :on-change (lambda (v) (if source-p (pc/fx-set-instrument-option source-p v) false))
          :width 4.8 :height 1.1 :font-size 8.5)))))

(def mod-selector-sections (inst)
  (if (> (len (get inst :sources)) 0)
    (get inst :sources)
    (get inst :modulators)))

(def mod-selector (inst)
  (box :debug-name "instrument-mod-selector"
       :width 9.4
       :height 7
       :padding 0.25
    (v-stack :gap 0.18 :align :start
      (v-stack :gap 0.18 :align :start
        (each (mod-selector-sections inst) |modulator mi|
          (mod-selector-row modulator))))))

(def selected-mod-source-section (inst)
  (nth (filter |section| (= (get section :slot) (pc/instrument-mod-selected-slot))
         (get inst :sources))
       0))

(def source-param (section name)
  (find-by-key (get section :params) :name name))

(def source-param-value (p fallback)
  (if p (pc/fx-param-value p) fallback))

(def source-set-param-value (p v)
  (if p (pc/instrument-set-param-control-value p v) false))

(def source-button (p title width)
  (let ((active (> (reactive-value (source-param-value p 0)) 0.5)))
    (v-stack :width width :height 1.72 :gap 0.10 :align :start
      (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
      (button (if active "ON" "OFF")
        :width width :height 0.88 :padding 0 :font-size 9
        :background-color (if active (lego/ui-accent-orange) :mixer-control-bg)
        :color (if active :black :dim)
        :on-click |x y r|
          (source-set-param-value p (if active 0 1))))))

(def source-dropdown (p title width)
  (v-stack :width width :height 1.72 :gap 0.10 :align :start
    (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
    (dropdown :value (if p (pc/fx-param-text-value-for false p) "")
      :options (if p (get p :options) '())
      :on-change (lambda (v) (if p (pc/fx-set-instrument-option p v) false))
      :width width :height 0.88 :font-size 8.5)))

(def source-number (p title decimals unit width)
  (v-stack :width width :height 1.72 :gap 0.10 :align :start
    (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
    (number-picker :value (source-param-value p 0)
      :min (if p (pc/instrument-param-control-min p) 0)
      :max (if p (pc/instrument-param-control-max p) 0)
      :decimals decimals
      :unit unit
      :noui true :font-size 9.3
      :text-color :dim :edit-color :yellow
      :text-align :left
      :width width :height 0.82
      :on-change (lambda (v) (source-set-param-value p v)))))

(def source-compact-knob (p title decimals)
  (box :debug-name (str "instrument-source-compact-knob-" title)
       :width 4.4 :height 2.25 :padding 0
    (knob-number :label title
      :value (source-param-value p 0)
      :min (if p (pc/instrument-param-control-min p) 0)
      :max (if p (pc/instrument-param-control-max p) 0)
      :decimals decimals
      :font-size 9.4 :label-font-size 8.2
      :text-color :dim :label-color :dim
      :width 4.4 :height 2.05
      :on-change (lambda (v) (source-set-param-value p v)))))

(def source-adsr-number (p title decimals unit)
  (v-stack :width 4.8 :height 1.18 :gap 0.16 :align :start
    (label title :font-size 10 :width 3.8 :height 0.52 :color :dim :bg :transparent :v-align :center)
    (number-picker :value (source-param-value p 0)
      :min (if p (pc/instrument-param-control-min p) 0)
      :max (if p (pc/instrument-param-control-max p) 0)
      :decimals decimals
      :unit unit
      :noui true :font-size 10
      :text-color :dim :edit-color :yellow
      :text-align :left
      :width 4.8 :height 0.50
      :on-change (lambda (v) (source-set-param-value p v)))))

(def env-source-editor (section)
  (let ((attack (source-param section "attack"))
        (decay (source-param section "decay"))
        (sustain (source-param section "sustain"))
        (release (source-param section "release")))
    (lego/ui-lego-panel-s 7.0 0 :instrument-control-bg
      (v-stack :debug-name "instrument-env-source-editor"
               :width :fill :height :fill :gap 0.2 :align :start
        (adsr-editor
          :attack (source-param-value attack 5)
          :decay (source-param-value decay 120)
          :sustain (source-param-value sustain 0.7)
          :release (source-param-value release 120)
          :attack-max (if attack (get attack :max) 20000)
          :decay-max (if decay (get decay :max) 20000)
          :release-max (if release (get release :max) 20000)
          :width :fill :height 4.5
          :background-color :instrument-control-bg
          :on-change (lambda (env)
            (do
              (source-set-param-value attack (get env :attack))
              (source-set-param-value decay (get env :decay))
              (source-set-param-value sustain (get env :sustain))
              (source-set-param-value release (get env :release)))))
        (h-stack :width :fill :gap 0.3 :align :start
          (source-adsr-number attack "atk" 0 "ms")
          (source-adsr-number decay "dec" 0 "ms")
          (source-adsr-number sustain "sus" 2 false)
          (source-adsr-number release "rel" 0 "ms"))))))

(def lfo-shape-curve (section shape pulse-width phase width height)
  (lfo-curve
    :width width :height height
    :shape (source-param-value shape 0)
    :pw (source-param-value pulse-width 0.5)
    :phase-offset (source-param-value phase 0)
    :phase (if (get section :phase-field) (bind-seq (get section :phase-field)) -1)
    :background-color :instrument-control-bg
    :grid-color :dim
    :curve-color (lego/ui-accent-orange)
    :fill-color (rgba 1.0 0.48 0.18 0.16)))

;; The section's params are already filtered by the host
;; (`selected_source_param_indices`): with sync on the rate is absent and the
;; division present, and pulse width only exists for the pulse shape. So each
;; control is drawn only when its param is, instead of as an empty widget.
(def lfo-source-editor (section)
  (let ((rate (source-param section "rate"))
        (sync (source-param section "sync"))
        (division (source-param section "division"))
        (shape (source-param section "shape"))
        (phase (source-param section "phase"))
        (pulse-width (source-param section "pulse width"))
        (retrigger (source-param section "retrigger")))
    (lego/ui-lego-panel-s 7.0 0 :instrument-control-bg
      (h-stack :debug-name "instrument-lfo-source-editor"
               :width :fill :height :fill :gap 0.38 :align :start
        (v-stack :width 11.8 :height :fill :gap 0.12 :align :start
          (h-stack :gap 0.25 :align :start
            (if division
              (source-dropdown division "division" 6.4)
              (source-number rate "rate" 2 "Hz" 6.4))
            (source-button sync "sync" 5.0))
          (h-stack :gap 0.25 :align :start
            (source-dropdown shape "shape" 6.4)
            (source-number phase "phase" 0 "°" 5.0))
          (h-stack :gap 0.25 :align :start
            (box :debug-name "instrument-lfo-retrigger-button"
                 :width 4.4 :height 1.72 :padding 0
              (source-button retrigger "retrig" 4.4))
            (if pulse-width
              (source-compact-knob pulse-width
                (if (= (reactive-value (source-param-value shape 0)) 0) "peak" "pw") 2)
              (box :width 4.4 :height 1.72))))
        (box :debug-name "instrument-lfo-curve-wrapper"
             :width 12.2 :height 5.7 :padding 0.22
             :background-color :black
             :corner-radius 8
          (lfo-shape-curve section shape pulse-width phase 11.7 5.25))))))

(def source-type (section)
  (let ((source-p (get section :source-param)))
    (if source-p (pc/fx-param-text-value-for false source-p) "off")))

(def selected-mod-source-editor (inst)
  (let ((slot (pc/instrument-mod-selected-slot)))
    (box :debug-name "instrument-selected-mod-source-editor"
         :width 25.5
         :height 8
         :padding 0.35
      (let ((section (selected-mod-source-section inst)))
        (if section
          (let ((kind (source-type section)))
            (v-stack :width :fill :height 4 :gap 0.3 :align :start
              (if (= kind "env")
                (env-source-editor section)
                (if (= kind "lfo")
                  (lfo-source-editor section)
                  (if (or (= kind "rand") (= kind "drift"))
                    (pg/fx-param-grid (get section :params) false)
                    (box :width :fill :height 5 :h-align :center :v-align :center
                      (label "no source controls" :font-size 12 :color :dim :bg :transparent)))))))
          (box :width :fill :height :fill :h-align :center :v-align :center
            (label "no source controls" :font-size 12 :color :dim :bg :transparent)))))))

(def mod-control-panel (inst)
  (box :debug-name "instrument-mod-control-panel"
    :width 36.4
    :height 9.5
    :padding 1
    :background-color :black
    :corner-radius 16
    (h-stack :height 7 :gap 0.25 :align :stretch
      (mod-selector inst)
      (selected-mod-source-editor inst))))
