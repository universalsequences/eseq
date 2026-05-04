;; Custom Synth tab body for instruments/emulations/monomachine-dpro-bbox-v1/dsp.lisp
(defstate monomachine-dpro-bbox-v1-selected-section 0)
(def mbbox-select (section)
  (set! monomachine-dpro-bbox-v1-selected-section section))
(def mbbox-panel-bg (section)
  (if (= section 0)
    (rgba 0.075 0.075 0.075 1)
    (if (= monomachine-dpro-bbox-v1-selected-section section)
      (rgba 0.12 0.12 0.12 1)
      (rgba 0.075 0.075 0.075 1))))
(def mbbox-cell-width 4.0)
(def mbbox-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mbbox-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (mbbox-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mbbox-param-cell-step-section (name title decimals step section)
  (mbbox-param-cell-step-section-width name title decimals step section mbbox-cell-width))
(def mbbox-param-cell-section (name title decimals section)
  (mbbox-param-cell-step-section name title decimals 0 section))
(def mbbox-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key "mbbox-base-note-cell"
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width mbbox-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (mbbox-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def mbbox-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def mbbox-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def mbbox-param-number-section (name title decimals unit section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mbbox-number-" name)
        (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
          (label title :font-size 10 :color :gray :bg :transparent)
          (number-picker :value (get p :value)
            :min (get p :min) :max (get p :max) :decimals decimals
            :unit unit
            :noui true :font-size 10.5
            :text-align :center
            :text-color :widget_focus_bg :edit-color :yellow
            :width 5.0 :height 0.95
            :on-change (lambda (v)
              (do
                (mbbox-select section)
                (fx-set-instrument-value p v))))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mbbox-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (mbbox-param-value attack 1)
    :decay (mbbox-param-value decay 180)
    :sustain (mbbox-param-value sustain 0.0)
    :release (mbbox-param-value release 35)
    :width 22.0 :height 3.55
    :background-color (rgba 0.0 0.0 0.0 1)
    :on-change (lambda (env)
      (do
        (mbbox-select section)
        (mbbox-set-param attack (get env :attack))
        (mbbox-set-param decay (get env :decay))
        (mbbox-set-param sustain (get env :sustain))
        (mbbox-set-param release (get env :release))))))
(def mbbox-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (mbbox-param-number-section attack "atk" 0 "ms" section)
      (mbbox-param-number-section decay "dec" 0 "ms" section)
      (mbbox-param-number-section sustain "sus" 2 false section)
      (mbbox-param-number-section release "rel" 0 "ms" section))))
(def mbbox-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :gray :bg :transparent)))
(def mbbox-adsr-panel-for (title attack decay sustain release section)
  (box :width :fill :height 6.55
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 8 :padding 0.15
    (v-stack :width :fill :gap 0.10
      (mbbox-adsr-view attack decay sustain release section)
      (mbbox-adsr-controls attack decay sustain release section)
      (mbbox-adsr-caption title))))
(def mbbox-adsr-panel ()
  (if (= monomachine-dpro-bbox-v1-selected-section 2)
    (mbbox-adsr-panel-for "FILTER ENV" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms" 2)
    (mbbox-adsr-panel-for "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 1)))
(def mbbox-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :gray :bg :transparent)))
(def mbbox-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (mbbox-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mbbox-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mbbox-row-label title)
      c1)))
(def mbbox-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (mbbox-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mbbox-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mbbox-row-label title)
      c1 c2)))
(def mbbox-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (mbbox-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mbbox-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mbbox-row-label title)
      c1 c2 c3 c4)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (mbbox-panel-1 "GLOB" 0
        (mbbox-base-note-cell 0))
      (mbbox-panel-4 "BBOX" 0
        (mbbox-param-cell-step-section "ptch" "ptch" 0 1 0)
        (mbbox-param-cell-step-section "start" "start" 0 1 0)
        (mbbox-param-cell-step-section "rtrg" "rtrg" 0 1 0)
        (mbbox-param-cell-step-section "rtim" "rtim" 0 1 0)))
    (v-stack :width 23.1 :gap 0.10
      (mbbox-adsr-panel))
    (v-stack :width 27.2 :gap 0.10
      (mbbox-panel-4 "FILT" 2
        (mbbox-param-cell-section "cutoff" "cut" 0 2)
        (mbbox-param-cell-section "resonance" "res" 2 2)
        (mbbox-param-cell-section "keytrack" "key" 2 2)
        (mbbox-param-cell-section "filter_env_amt" "env" 0 2))
      (mbbox-panel-2 "OUT" 0
        (mbbox-param-cell-section "drive" "drv" 2 0)
        (mbbox-param-cell-section "gain" "gain" 2 0)))))
