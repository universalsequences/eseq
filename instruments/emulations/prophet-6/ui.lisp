;; Custom Synth tab body for instruments/emulations/prophet-6/dsp.lisp
(defstate prophet-6-selected-section 0)
(def prophet_6-select (section)
  (set! prophet-6-selected-section section))
(def prophet_6-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= prophet-6-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def prophet_6-cell-width 4.0)
(def prophet_6-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "prophet_6-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (prophet_6-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def prophet_6-param-cell-step-section (name title decimals step section)
  (prophet_6-param-cell-step-section-width name title decimals step section prophet_6-cell-width))
(def prophet_6-param-cell-section (name title decimals section)
  (prophet_6-param-cell-step-section name title decimals 0 section))
(def prophet_6-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "prophet_6-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width prophet_6-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (prophet_6-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def prophet_6-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "prophet_6-adsr-number-" name)
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
                  (prophet_6-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :dim :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :dim :edit-color :dim
          :width 5.0 :height 0.95)))))
(def prophet_6-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def prophet_6-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def prophet_6-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (prophet_6-param-value attack 4)
    :decay (prophet_6-param-value decay 400)
    :sustain (prophet_6-param-value sustain 0.5)
    :release (prophet_6-param-value release 0)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (prophet_6-select section)
        (prophet_6-set-param attack (get env :attack))
        (prophet_6-set-param decay (get env :decay))
        (prophet_6-set-param sustain (get env :sustain))
        (prophet_6-set-param release (get env :release))))))
(def prophet_6-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-param-number-section attack "atk" 0 "ms" section)
      (prophet_6-param-number-section decay "dec" 0 "ms" section)
      (prophet_6-param-number-section sustain "sus" 2 false section)
      (prophet_6-param-number-section release "rel" 0 "ms" section))))

(def prophet_6-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def prophet_6-selected-adsr ()
  (if (= prophet-6-selected-section 1)
    (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (prophet_6-adsr-view "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (prophet_6-adsr-controls "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (prophet_6-adsr-caption "FILTER ENV")))
    (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (prophet_6-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (prophet_6-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (prophet_6-adsr-caption "AMP ENV")))))
(def prophet_6-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def prophet_6-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (prophet_6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (prophet_6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-row-label title)
      c1)))
(def prophet_6-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (prophet_6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (prophet_6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-row-label title)
      c1 c2)))
(def prophet_6-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (prophet_6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (prophet_6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-row-label title)
      c1 c2 c3)))
(def prophet_6-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (prophet_6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (prophet_6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-row-label title)
      c1 c2 c3 c4)))
(def prophet_6-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (prophet_6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (prophet_6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-row-label title)
      c1 c2 c3 c4 c5)))
(def prophet_6-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (prophet_6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (prophet_6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def prophet_6-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (prophet_6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (prophet_6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def prophet_6-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (prophet_6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (prophet_6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (prophet_6-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (prophet_6-panel-1 "GLOB" 0
        (prophet_6-base-note-cell 0))
      (prophet_6-panel-3 "OSC1" 0
        (prophet_6-param-cell-section "osc1_shape" "shape" 2 0)
        (prophet_6-param-cell-section "osc1_pw" "pw" 2 0)
        (prophet_6-param-cell-section "osc1_mix" "mix" 2 0))
      (prophet_6-panel-5 "OSC2" 0
        (prophet_6-param-cell-section "osc2_shape" "shape" 2 0)
        (prophet_6-param-cell-section "osc2_pw" "pw" 2 0)
        (prophet_6-param-cell-section "osc2_mix" "mix" 2 0)
        (prophet_6-param-cell-section "osc2_detune" "det" 2 0)
        (prophet_6-param-cell-section "osc2_fine" "fine" 2 0)))
    (v-stack :width 23.1 :gap 0.10
      (prophet_6-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (prophet_6-panel-2 "MIX" 0
        (prophet_6-param-cell-section "sub_mix" "sub" 2 0)
        (prophet_6-param-cell-section "noise_mix" "noise" 2 0))
      (prophet_6-panel-4 "FILT" 1
        (prophet_6-param-cell-section "cutoff" "cut" 0 1)
        (prophet_6-param-cell-section "resonance" "res" 2 1)
        (prophet_6-param-cell-section "filter_env_amt" "env" 0 1)
        (prophet_6-param-cell-section "keytrack" "key" 2 1))
      (prophet_6-panel-4 "MOD" 1
        (prophet_6-param-cell-section "lfo_rate" "rate" 2 1)
        (prophet_6-param-cell-section "lfo_pitch_amt" "pitch" 2 1)
        (prophet_6-param-cell-section "lfo_shape_amt" "shape" 2 1)
        (prophet_6-param-cell-section "vibrato_amt" "vib" 2 1))
      (prophet_6-panel-3 "OUT" 0
        (prophet_6-param-cell-section "drift" "drift" 2 0)
        (prophet_6-param-cell-section "drive" "drive" 2 0)
        (prophet_6-param-cell-section "gain" "gain" 2 0)))))
