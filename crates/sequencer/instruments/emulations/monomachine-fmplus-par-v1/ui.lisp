;; Custom Synth tab body for instruments/emulations/monomachine-fmplus-par-v1/dsp.lisp
(defstate monomachine-fmplus-par-v1-selected-section 0)
(def mfpar-select (section)
  (set! monomachine-fmplus-par-v1-selected-section section))
(def mfpar-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= monomachine-fmplus-par-v1-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def mfpar-cell-width 4.0)
(def mfpar-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mfpar-cell-" name)
        (knob-number :label title
          :value (fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (mfpar-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mfpar-param-cell-step-section (name title decimals step section)
  (mfpar-param-cell-step-section-width name title decimals step section mfpar-cell-width))
(def mfpar-param-cell-section (name title decimals section)
  (mfpar-param-cell-step-section name title decimals 0 section))
(def mfpar-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key "mfpar-base-note-cell"
        (knob-number :label "note"
          :value (fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width mfpar-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (mfpar-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def mfpar-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-param-value p) fallback))
    fallback))
(def mfpar-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def mfpar-param-number-section (name title decimals unit section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mfpar-number-" name)
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
                (mfpar-select section)
                (fx-set-instrument-value p v))))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mfpar-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (mfpar-param-value attack 2)
    :decay (mfpar-param-value decay 120)
    :sustain (mfpar-param-value sustain 0.78)
    :release (mfpar-param-value release 90)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (mfpar-select section)
        (mfpar-set-param attack (get env :attack))
        (mfpar-set-param decay (get env :decay))
        (mfpar-set-param sustain (get env :sustain))
        (mfpar-set-param release (get env :release))))))
(def mfpar-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (mfpar-param-number-section attack "atk" 0 "ms" section)
      (mfpar-param-number-section decay "dec" 0 "ms" section)
      (mfpar-param-number-section sustain "sus" 2 false section)
      (mfpar-param-number-section release "rel" 0 "ms" section))))
(def mfpar-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def mfpar-adsr-panel-for (title attack decay sustain release section)
  (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 8 :padding 0.15
    (v-stack :width :fill :gap 0.10
      (mfpar-adsr-view attack decay sustain release section)
      (mfpar-adsr-controls attack decay sustain release section)
      (mfpar-adsr-caption title))))
(def mfpar-adsr-panel ()
  (if (= monomachine-fmplus-par-v1-selected-section 1)
    (mfpar-adsr-panel-for "MOD1 ENV" "op1_attack_ms" "op1_decay_ms" "op1_sustain" "op1_release_ms" 1)
    (if (= monomachine-fmplus-par-v1-selected-section 2)
      (mfpar-adsr-panel-for "MOD2 ENV" "op2_attack_ms" "op2_decay_ms" "op2_sustain" "op2_release_ms" 2)
      (if (= monomachine-fmplus-par-v1-selected-section 3)
        (mfpar-adsr-panel-for "MOD3 ENV" "op3_attack_ms" "op3_decay_ms" "op3_sustain" "op3_release_ms" 3)
        (if (= monomachine-fmplus-par-v1-selected-section 4)
          (mfpar-adsr-panel-for "FILTER ENV" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms" 4)
          (mfpar-adsr-panel-for "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0))))))
(def mfpar-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def mfpar-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (mfpar-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mfpar-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mfpar-row-label title)
      c1)))
(def mfpar-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (mfpar-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mfpar-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mfpar-row-label title)
      c1 c2)))
(def mfpar-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (mfpar-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mfpar-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mfpar-row-label title)
      c1 c2 c3)))
(def mfpar-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (mfpar-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mfpar-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mfpar-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def mfpar-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (mfpar-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mfpar-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mfpar-row-label title)
      c1 c2 c3 c4)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (mfpar-panel-1 "GLOB" 0
        (mfpar-base-note-cell 0))
      (mfpar-panel-2 "MOD1" 1
        (mfpar-param-cell-step-section "op1_frq" "1frq" 0 1 1)
        (mfpar-param-cell-section "op1_env" "1env" 2 1))
      (mfpar-panel-2 "MOD2" 2
        (mfpar-param-cell-step-section "op2_frq" "2frq" 0 1 2)
        (mfpar-param-cell-section "op2_env" "2env" 2 2))
      (mfpar-panel-2 "MOD3" 3
        (mfpar-param-cell-step-section "op3_frq" "3frq" 0 1 3)
        (mfpar-param-cell-section "op3_env" "3env" 2 3)))
    (v-stack :width 23.1 :gap 0.10
      (mfpar-adsr-panel))
    (v-stack :width 27.2 :gap 0.10
      (mfpar-panel-4 "FILT" 4
        (mfpar-param-cell-section "cutoff" "cut" 0 4)
        (mfpar-param-cell-section "resonance" "res" 2 4)
        (mfpar-param-cell-section "keytrack" "key" 2 4)
        (mfpar-param-cell-section "filter_env_amt" "env" 0 4))
      (mfpar-panel-6 "WAVE" 0
        (mfpar-param-cell-step-section "op1_wave" "w1" 0 1 0)
        (mfpar-param-cell-section "op1_mix" "m1" 2 0)
        (mfpar-param-cell-step-section "op2_wave" "w2" 0 1 0)
        (mfpar-param-cell-section "op2_mix" "m2" 2 0)
        (mfpar-param-cell-step-section "op3_wave" "w3" 0 1 0)
        (mfpar-param-cell-section "op3_mix" "m3" 2 0))
      (mfpar-panel-4 "CARR" 0
        (mfpar-param-cell-step-section "car_wave" "wave" 0 1 0)
        (mfpar-param-cell-section "car_mix" "mix" 2 0)
        (mfpar-param-cell-section "tone" "tone" 2 0)
        (mfpar-param-cell-section "tune_cents" "tune" 0 0)
        )
      (mfpar-panel-2 "OUT" 0
        (mfpar-param-cell-section "drive" "drv" 2 0)
        (mfpar-param-cell-section "gain" "gain" 2 0)))))
