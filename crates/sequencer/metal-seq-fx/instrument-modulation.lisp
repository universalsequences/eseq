;; Instrument modulation source selection and editor controls.
(def instrument-mod-base-name (name)
  (if (string-ends-with? name " amt")
    (substring name 0 (- (len name) 4))
    (if (string-ends-with? name " src")
      (substring name 0 (- (len name) 4))
      name)))

(def instrument-mod-source-param (params base)
  (nth (filter |p| (= (get p :name) (str base " src")) params) 0))

(def instrument-mod-amount-params (params)
  (filter |p| (string-ends-with? (get p :name) " amt") params))

(def instrument-mod-row (params amount-p subtree-key)
  (let ((base (instrument-mod-base-name (get amount-p :name)))
        (source-p (instrument-mod-source-param params base)))
    (subtree :key subtree-key
      (box :width 12.6 :height 2.35
           :background-color :instrument-group-bg
           :border-width 1
           :corner-radius 16
           :padding 0.25
        (h-stack :width :fill :gap 0.45 :align :center
          (if source-p
            (dropdown :value (get source-p :text-value)
              :options (get source-p :options)
              :on-change (lambda (v) (fx-set-instrument-option source-p v))
              :width 4.8 :height 1.15 :font-size 10)
            (box :width 5.3 :height 1.15))
          (knob-number :label (substring base 0 12)
            :value (fx-param-value amount-p)
            :min (get amount-p :min) :max (get amount-p :max) :decimals 2
            :font-size 10.5 :label-font-size 9
            :text-color :dim :label-color :dim
            :width 5.2 :height 2.05
            :on-change (lambda (v) (fx-set-instrument-value amount-p v))))))))

(def instrument-mod-grid (params)
  (let ((amounts (instrument-mod-amount-params params)))
    (h-stack :gap 0.45 :padding 0
      (each (chunks amounts 3) |chunk ci|
        (v-stack :gap 0.18
          (each chunk |p pi|
            (instrument-mod-row params p
              (str "instrument-mod-row-" ci "-param-" (get p :idx)))))))))

(def instrument-mod-selector-row (modulator)
  (let ((slot (get modulator :slot))
        (label-text (if (get modulator :name) (get modulator :name) (get modulator :label)))
        (source-p (get modulator :source-param)))
    (subtree :key (str "instrument-mod-selector-" slot)
      (h-stack :gap 0.18 :align :center
        (button label-text
          :width 3.9 :height 1.1
          :padding 0
          :font-size 9
          :background-color (if (= instrument-selected-mod-slot slot)
            (rgba 0.95 0.48 0.18 0.82)
            :instrument-control-bg)
          :color (if (= instrument-selected-mod-slot slot) :white :dim)
          :on-click (lambda (info) (set! instrument-selected-mod-slot slot)))
        (dropdown :value (if source-p (get source-p :text-value) "off")
          :options (if source-p (get source-p :options) '())
          :on-change (lambda (v) (if source-p (fx-set-instrument-option source-p v) false))
          :width 4.8 :height 1.1 :font-size 8.5)))))

(def instrument-mod-selector-sections (inst)
  (if (> (len (get inst :sources)) 0)
    (get inst :sources)
    (get inst :modulators)))

(def instrument-mod-selector (inst)
  (box :debug-name "instrument-mod-selector"
       :width 9.4
       :height 7
       :padding 0.25
    (v-stack :gap 0.18 :align :start
      (v-stack :gap 0.18 :align :start
        (each (instrument-mod-selector-sections inst) |modulator mi|
          (instrument-mod-selector-row modulator))))))

(def instrument-selected-mod-source-section (inst)
  (nth (filter |section| (= (get section :slot) (instrument-mod-selected-slot))
         (get inst :sources))
       0))

(def instrument-source-param (section name)
  (nth (filter |p| (= (get p :name) name) (get section :params)) 0))

(def instrument-source-param-value (p fallback)
  (if p (fx-param-value p) fallback))

(def instrument-source-set-param-value (p v)
  (if p (instrument-set-param-control-value p v) false))

(def instrument-source-button (p title width)
  (let ((active (> (reactive-value (instrument-source-param-value p 0)) 0.5)))
    (v-stack :width width :height 1.72 :gap 0.10 :align :start
      (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
      (button (if active "ON" "OFF")
        :width width :height 0.88 :padding 0 :font-size 9
        :background-color (if active (ui-accent-orange) :mixer-control-bg)
        :color (if active :black :dim)
        :on-click |x y r|
          (instrument-source-set-param-value p (if active 0 1))))))

(def instrument-source-dropdown (p title width)
  (v-stack :width width :height 1.72 :gap 0.10 :align :start
    (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
    (dropdown :value (if p (get p :text-value) "")
      :options (if p (get p :options) '())
      :on-change (lambda (v) (if p (fx-set-instrument-option p v) false))
      :width width :height 0.88 :font-size 8.5)))

(def instrument-source-number (p title decimals unit width)
  (v-stack :width width :height 1.72 :gap 0.10 :align :start
    (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
    (number-picker :value (instrument-source-param-value p 0)
      :min (if p (instrument-param-control-min p) 0)
      :max (if p (instrument-param-control-max p) 0)
      :decimals decimals
      :unit unit
      :noui true :font-size 9.3
      :text-color :dim :edit-color :yellow
      :text-align :left
      :width width :height 0.82
      :on-change (lambda (v) (instrument-source-set-param-value p v)))))

(def instrument-source-compact-knob (p title decimals)
  (box :debug-name (str "instrument-source-compact-knob-" title)
       :width 4.4 :height 2.25 :padding 0
    (knob-number :label title
      :value (instrument-source-param-value p 0)
      :min (if p (instrument-param-control-min p) 0)
      :max (if p (instrument-param-control-max p) 0)
      :decimals decimals
      :font-size 9.4 :label-font-size 8.2
      :text-color :dim :label-color :dim
      :width 4.4 :height 2.05
      :on-change (lambda (v) (instrument-source-set-param-value p v)))))

(def instrument-source-adsr-number (p title decimals unit)
  (v-stack :width 3.8 :height 1.18 :gap 0.16 :align :start
    (label title :font-size 7.4 :width 3.8 :height 0.52 :color :dim :bg :transparent)
    (number-picker :value (instrument-source-param-value p 0)
      :min (if p (instrument-param-control-min p) 0)
      :max (if p (instrument-param-control-max p) 0)
      :decimals decimals
      :unit unit
      :noui true :font-size 9.0
      :text-color :widget_focus_bg :edit-color :yellow
      :text-align :left
      :width 3.8 :height 0.50
      :on-change (lambda (v) (instrument-source-set-param-value p v)))))

(def instrument-env-source-editor (section)
  (let ((attack (instrument-source-param section "attack"))
        (decay (instrument-source-param section "decay"))
        (sustain (instrument-source-param section "sustain"))
        (release (instrument-source-param section "release")))
    (ui-readout-panel-medium-s 0
      (h-stack :width :fill :height :fill :gap 0.24 :align :stretch
        (adsr-editor
          :attack (instrument-source-param-value attack 5)
          :decay (instrument-source-param-value decay 120)
          :sustain (instrument-source-param-value sustain 0.7)
          :release (instrument-source-param-value release 120)
          :width 13.2 :height :fill
          :background-color :instrument-control-bg
          :on-change (lambda (env)
            (do
              (instrument-source-set-param-value attack (get env :attack))
              (instrument-source-set-param-value decay (get env :decay))
              (instrument-source-set-param-value sustain (get env :sustain))
              (instrument-source-set-param-value release (get env :release)))))
        (v-stack :width 8.2 :height :fill :gap 0.10 :align :start
          (ui-lego-badge-dark "env" 7.7 (ui-accent-blue))
          (h-stack :gap 0.14 :align :start
            (instrument-source-adsr-number attack "atk" 0 "ms")
            (instrument-source-adsr-number decay "dec" 0 "ms"))
          (h-stack :gap 0.14 :align :start
            (instrument-source-adsr-number sustain "sus" 2 false)
            (instrument-source-adsr-number release "rel" 0 "ms")))))))

(def instrument-lfo-source-editor (section)
  (let ((rate (instrument-source-param section "rate"))
        (sync (instrument-source-param section "sync"))
        (division (instrument-source-param section "division"))
        (shape (instrument-source-param section "shape"))
        (pulse-width (instrument-source-param section "pulse width"))
        (retrigger (instrument-source-param section "retrigger")))
    (ui-readout-panel-medium-s 0
      (h-stack :debug-name "instrument-lfo-source-editor"
               :width :fill :height :fill :gap 0.38 :align :start
        (v-stack :width 13.8 :height :fill :gap 0.12 :align :start
          (h-stack :gap 0.25 :align :start
            (instrument-source-number rate "rate" 2 false 6.4)
            (instrument-source-button sync "sync" 5.0))
          (h-stack :gap 0.25 :align :start
            (instrument-source-dropdown division "division" 6.4)
            (instrument-source-dropdown shape "shape" 5.0)))
        (v-stack :width 5.0 :height :fill :gap 0.18 :align :center
          (instrument-source-compact-knob pulse-width "pw" 2)
          (box :debug-name "instrument-lfo-retrigger-button"
               :width 4.4 :height 1.55 :padding 0
            (instrument-source-button retrigger "retrig" 4.4)))))))

(def instrument-source-type (section)
  (let ((source-p (get section :source-param)))
    (if source-p (get source-p :text-value) "off")))

(def instrument-selected-mod-source-editor (inst)
  (let ((slot (instrument-mod-selected-slot)))
    (box :debug-name "instrument-selected-mod-source-editor"
         :width 25.5
         :height 8
         :padding 0.35
      (let ((section (instrument-selected-mod-source-section inst)))
        (if section
          (let ((source-type (instrument-source-type section)))
            (v-stack :width :fill :height 4 :gap 0.3 :align :start
              (if (= source-type "env")
                (instrument-env-source-editor section)
                (if (= source-type "lfo")
                  (instrument-lfo-source-editor section)
                  (if (or (= source-type "rand") (= source-type "drift"))
                    (fx-param-grid (get section :params) false)
                    (box :width :fill :height 5 :h-align :center :v-align :center
                      (label "no source controls" :font-size 12 :color :dim :bg :transparent)))))))
          (box :width :fill :height :fill :h-align :center :v-align :center
            (label "no source controls" :font-size 12 :color :dim :bg :transparent)))))))

(def instrument-mod-control-panel (inst)
  (box :debug-name "instrument-mod-control-panel"
    :width 36.4
    :height 7
    :padding 1
    :background-color :black
    :corner-radius 10
    (h-stack :height 7 :gap 0.25 :align :stretch
      (instrument-mod-selector inst)
      (instrument-selected-mod-source-editor inst))))
