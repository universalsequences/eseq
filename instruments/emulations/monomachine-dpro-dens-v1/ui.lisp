;; Custom Synth tab body for instruments/emulations/monomachine-dpro-dens-v1/dsp.lisp
(defstate monomachine-dpro-dens-v1-selected-section 0)
(def mdens-select (section)
  (set! monomachine-dpro-dens-v1-selected-section section))
(def mdens-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= monomachine-dpro-dens-v1-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def mdens-cell-width 4.0)
(def mdens-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mdens-cell-" name)
        (knob-number :label title
          :value (fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (mdens-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mdens-param-cell-step-section (name title decimals step section)
  (mdens-param-cell-step-section-width name title decimals step section mdens-cell-width))
(def mdens-param-cell-section (name title decimals section)
  (mdens-param-cell-step-section name title decimals 0 section))
(def mdens-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key "mdens-base-note-cell"
        (knob-number :label "note"
          :value (fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width mdens-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (mdens-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def mdens-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-param-value p) fallback))
    fallback))
(def mdens-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def mdens-param-number-section (name title decimals unit section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mdens-number-" name)
        (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
          (label title :font-size 10 :color :dim :bg :transparent)
          (number-picker :value (fx-param-value p)
            :min (get p :min) :max (get p :max) :decimals decimals
            :unit unit
            :noui true :font-size 10.5
            :text-align :center
            :text-color :widget_focus_bg :edit-color :yellow
            :width 5.0 :height 0.95
            :on-change (lambda (v)
              (do
                (mdens-select section)
                (fx-set-instrument-value p v))))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mdens-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (mdens-param-value attack 2)
    :decay (mdens-param-value decay 120)
    :sustain (mdens-param-value sustain 0.78)
    :release (mdens-param-value release 90)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (mdens-select section)
        (mdens-set-param attack (get env :attack))
        (mdens-set-param decay (get env :decay))
        (mdens-set-param sustain (get env :sustain))
        (mdens-set-param release (get env :release))))))
(def mdens-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (mdens-param-number-section attack "atk" 0 "ms" section)
      (mdens-param-number-section decay "dec" 0 "ms" section)
      (mdens-param-number-section sustain "sus" 2 false section)
      (mdens-param-number-section release "rel" 0 "ms" section))))
(def mdens-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def mdens-adsr-panel-for (title attack decay sustain release section)
  (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 8 :padding 0.15
    (v-stack :width :fill :gap 0.10
      (mdens-adsr-view attack decay sustain release section)
      (mdens-adsr-controls attack decay sustain release section)
      (mdens-adsr-caption title))))
(def mdens-adsr-panel ()
  (if (= monomachine-dpro-dens-v1-selected-section 2)
    (mdens-adsr-panel-for "FILTER ENV" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms" 2)
    (mdens-adsr-panel-for "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 1)))
(def mdens-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def mdens-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (mdens-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdens-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdens-row-label title)
      c1)))
(def mdens-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (mdens-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdens-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdens-row-label title)
      c1 c2)))
(def mdens-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (mdens-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdens-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdens-row-label title)
      c1 c2 c3)))
(def mdens-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (mdens-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdens-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdens-row-label title)
      c1 c2 c3 c4)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (mdens-panel-1 "GLOB" 0
        (mdens-base-note-cell 0))
      (mdens-panel-4 "DENS" 0
        (mdens-param-cell-step-section "wave" "wave" 0 1 0)
        (mdens-param-cell-step-section "pch2" "pch2" 0 1 0)
        (mdens-param-cell-step-section "pch3" "pch3" 0 1 0)
        (mdens-param-cell-step-section "pch4" "pch4" 0 1 0))
      (mdens-panel-3 "CHOR" 0
        (mdens-param-cell-section "chrl" "lev" 2 0)
        (mdens-param-cell-section "chrw" "wid" 2 0)
        (mdens-param-cell-section "tune_cents" "tune" 0 0)))
    (v-stack :width 23.1 :gap 0.10
      (mdens-adsr-panel))
    (v-stack :width 27.2 :gap 0.10
      (mdens-panel-4 "FILT" 2
        (mdens-param-cell-section "cutoff" "cut" 0 2)
        (mdens-param-cell-section "resonance" "res" 2 2)
        (mdens-param-cell-section "keytrack" "key" 2 2)
        (mdens-param-cell-section "filter_env_amt" "env" 0 2))
      (mdens-panel-2 "OUT" 0
        (mdens-param-cell-section "drive" "drv" 2 0)
        (mdens-param-cell-section "gain" "gain" 2 0)))))
