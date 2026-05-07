;; Custom Synth tab body for instruments/emulations/monomachine-fmplus-stat-v1/dsp.lisp
(defstate monomachine-fmplus-stat-v1-selected-section 0)
(def mfstat-select (section)
  (set! monomachine-fmplus-stat-v1-selected-section section))
(def mfstat-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= monomachine-fmplus-stat-v1-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def mfstat-cell-width 4.0)
(def mfstat-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mfstat-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (mfstat-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mfstat-param-cell-step-section (name title decimals step section)
  (mfstat-param-cell-step-section-width name title decimals step section mfstat-cell-width))
(def mfstat-param-cell-section (name title decimals section)
  (mfstat-param-cell-step-section name title decimals 0 section))
(def mfstat-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key "mfstat-base-note-cell"
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width mfstat-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (mfstat-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def mfstat-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def mfstat-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def mfstat-param-number-section (name title decimals unit section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mfstat-number-" name)
        (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
          (label title :font-size 10 :color :dim :bg :transparent)
          (number-picker :value (get p :value)
            :min (get p :min) :max (get p :max) :decimals decimals
            :unit unit
            :noui true :font-size 10.5
            :text-align :center
            :text-color :widget_focus_bg :edit-color :yellow
            :width 5.0 :height 0.95
            :on-change (lambda (v)
              (do
                (mfstat-select section)
                (fx-set-instrument-value p v))))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mfstat-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (mfstat-param-value attack 2)
    :decay (mfstat-param-value decay 120)
    :sustain (mfstat-param-value sustain 0.78)
    :release (mfstat-param-value release 90)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (mfstat-select section)
        (mfstat-set-param attack (get env :attack))
        (mfstat-set-param decay (get env :decay))
        (mfstat-set-param sustain (get env :sustain))
        (mfstat-set-param release (get env :release))))))
(def mfstat-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (mfstat-param-number-section attack "atk" 0 "ms" section)
      (mfstat-param-number-section decay "dec" 0 "ms" section)
      (mfstat-param-number-section sustain "sus" 2 false section)
      (mfstat-param-number-section release "rel" 0 "ms" section))))
(def mfstat-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def mfstat-adsr-panel-for (title attack decay sustain release section)
  (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 8 :padding 0.15
    (v-stack :width :fill :gap 0.10
      (mfstat-adsr-view attack decay sustain release section)
      (mfstat-adsr-controls attack decay sustain release section)
      (mfstat-adsr-caption title))))
(def mfstat-adsr-panel ()
  (if (= monomachine-fmplus-stat-v1-selected-section 1)
    (mfstat-adsr-panel-for "MOD1 ENV" "op1_attack_ms" "op1_decay_ms" "op1_sustain" "op1_release_ms" 1)
    (if (= monomachine-fmplus-stat-v1-selected-section 2)
      (mfstat-adsr-panel-for "FILTER ENV" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms" 2)
      (mfstat-adsr-panel-for "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0))))
(def mfstat-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def mfstat-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (mfstat-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mfstat-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mfstat-row-label title)
      c1)))
(def mfstat-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (mfstat-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mfstat-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mfstat-row-label title)
      c1 c2)))
(def mfstat-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (mfstat-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mfstat-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mfstat-row-label title)
      c1 c2 c3 c4)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (mfstat-panel-1 "GLOB" 0
        (mfstat-base-note-cell 0))
      (mfstat-panel-4 "MOD1" 1
        (mfstat-param-cell-step-section "op1_frq" "1frq" 0 1 1)
        (mfstat-param-cell-section "op1_fin" "1fin" 0 1)
        (mfstat-param-cell-section "op1_fb" "1fb" 2 1)
        (mfstat-param-cell-section "op1_env" "1env" 2 1))
      (mfstat-panel-4 "MOD2" 0
        (mfstat-param-cell-step-section "op2_frq" "2frq" 0 1 0)
        (mfstat-param-cell-section "op2_vol" "2vol" 2 0)
        (mfstat-param-cell-section "tone" "tone" 2 0)
        (mfstat-param-cell-section "tune_cents" "tune" 0 0)))
    (v-stack :width 23.1 :gap 0.10
      (mfstat-adsr-panel))
    (v-stack :width 27.2 :gap 0.10
      (mfstat-panel-4 "FILT" 2
        (mfstat-param-cell-section "cutoff" "cut" 0 2)
        (mfstat-param-cell-section "resonance" "res" 2 2)
        (mfstat-param-cell-section "keytrack" "key" 2 2)
        (mfstat-param-cell-section "filter_env_amt" "env" 0 2))
      (mfstat-panel-2 "OUT" 0
        (mfstat-param-cell-section "drive" "drv" 2 0)
        (mfstat-param-cell-section "gain" "gain" 2 0)))))
