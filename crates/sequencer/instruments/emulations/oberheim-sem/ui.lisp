;; Custom Synth tab body for instruments/emulations/oberheim-sem/dsp.lisp
(defstate oberheim-sem-selected-section 0)
(def oberheim_sem-select (section)
  (set! oberheim-sem-selected-section section))
(def oberheim_sem-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= oberheim-sem-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def oberheim_sem-cell-width 4.0)
(def oberheim_sem-param-cell-step-section-width (name title decimals step section width)
  (let ((p (eseq.effects.custom-ui-runtime/inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "oberheim_sem-cell-" name)
        (knob-number :label title
          :value (eseq.effects.param-controls/fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (oberheim_sem-select section)
              (eseq.effects.param-controls/fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def oberheim_sem-param-cell-step-section (name title decimals step section)
  (oberheim_sem-param-cell-step-section-width name title decimals step section oberheim_sem-cell-width))
(def oberheim_sem-param-cell-section (name title decimals section)
  (oberheim_sem-param-cell-step-section name title decimals 0 section))
(def oberheim_sem-base-note-cell (section)
  (let ((p (eseq.effects.custom-ui-runtime/inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "oberheim_sem-base-note-cell")
        (knob-number :label "note"
          :value (eseq.effects.param-controls/fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width oberheim_sem-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (oberheim_sem-select section)
              (eseq.effects.param-controls/fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def oberheim_sem-param-number-section (name title decimals unit section)
  (if name
    (let ((p (eseq.effects.custom-ui-runtime/inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "oberheim_sem-adsr-number-" name)
          (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
            (label title :font-size 10 :color :dim :bg :transparent)
            (number-picker :value (eseq.effects.param-controls/fx-param-value p)
              :min (get p :min) :max (get p :max) :decimals decimals
              :unit unit
              :noui true :font-size 10.5
              :text-align :center
              :text-color :widget_focus_bg :edit-color :yellow
              :width 5.0 :height 0.95
              :on-change (lambda (v)
                (do
                  (oberheim_sem-select section)
                  (eseq.effects.param-controls/fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :dim :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :dim :edit-color :dim
          :width 5.0 :height 0.95)))))
(def oberheim_sem-param-value (name fallback)
  (if name
    (let ((p (eseq.effects.custom-ui-runtime/inst-param synth-ui-current-inst name)))
      (if p (eseq.effects.param-controls/fx-param-value p) fallback))
    fallback))
(def oberheim_sem-set-param (name value)
  (if name
    (let ((p (eseq.effects.custom-ui-runtime/inst-param synth-ui-current-inst name)))
      (if p (eseq.effects.param-controls/fx-set-instrument-value p value) false))
    false))
(def oberheim_sem-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (oberheim_sem-param-value attack 4)
    :decay (oberheim_sem-param-value decay 400)
    :sustain (oberheim_sem-param-value sustain 0.5)
    :release (oberheim_sem-param-value release 0)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (oberheim_sem-select section)
        (oberheim_sem-set-param attack (get env :attack))
        (oberheim_sem-set-param decay (get env :decay))
        (oberheim_sem-set-param sustain (get env :sustain))
        (oberheim_sem-set-param release (get env :release))))))
(def oberheim_sem-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-param-number-section attack "atk" 0 "ms" section)
      (oberheim_sem-param-number-section decay "dec" 0 "ms" section)
      (oberheim_sem-param-number-section sustain "sus" 2 false section)
      (oberheim_sem-param-number-section release "rel" 0 "ms" section))))

(def oberheim_sem-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def oberheim_sem-selected-adsr ()
  (if (= oberheim-sem-selected-section 1)
    (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (oberheim_sem-adsr-view "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (oberheim_sem-adsr-controls "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (oberheim_sem-adsr-caption "FILTER ENV")))
    (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (oberheim_sem-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (oberheim_sem-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (oberheim_sem-adsr-caption "AMP ENV")))))
(def oberheim_sem-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def oberheim_sem-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (oberheim_sem-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (oberheim_sem-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-row-label title)
      c1)))
(def oberheim_sem-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (oberheim_sem-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (oberheim_sem-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-row-label title)
      c1 c2)))
(def oberheim_sem-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (oberheim_sem-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (oberheim_sem-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-row-label title)
      c1 c2 c3)))
(def oberheim_sem-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (oberheim_sem-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (oberheim_sem-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-row-label title)
      c1 c2 c3 c4)))
(def oberheim_sem-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (oberheim_sem-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (oberheim_sem-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-row-label title)
      c1 c2 c3 c4 c5)))
(def oberheim_sem-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (oberheim_sem-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (oberheim_sem-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def oberheim_sem-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (oberheim_sem-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (oberheim_sem-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def oberheim_sem-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (oberheim_sem-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (oberheim_sem-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (oberheim_sem-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (oberheim_sem-panel-1 "GLOB" 0
        (oberheim_sem-base-note-cell 0))
      (oberheim_sem-panel-4 "OSC" 0
        (oberheim_sem-param-cell-step-section "osc_a_semi" "a st" 0 1 0)
        (oberheim_sem-param-cell-step-section "osc_b_semi" "b st" 0 1 0)
        (oberheim_sem-param-cell-section "osc_b_detune" "b det" 0 0)
        (oberheim_sem-param-cell-section "osc_b_keytrack" "key" 2 0))
      (oberheim_sem-panel-4 "LVL" 0
        (oberheim_sem-param-cell-section "osc_a_level" "a" 2 0)
        (oberheim_sem-param-cell-section "osc_b_level" "b" 2 0)
        (oberheim_sem-param-cell-section "noise_level" "noise" 2 0)
        (oberheim_sem-param-cell-section "vintage" "vint" 2 0)))
    (v-stack :width 23.1 :gap 0.10
      (oberheim_sem-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (oberheim_sem-panel-5 "WAV" 0
        (oberheim_sem-param-cell-section "osc_a_saw" "a saw" 2 0)
        (oberheim_sem-param-cell-section "osc_a_pulse" "a pls" 2 0)
        (oberheim_sem-param-cell-section "osc_b_saw" "b saw" 2 0)
        (oberheim_sem-param-cell-section "osc_b_pulse" "b pls" 2 0)
        (oberheim_sem-param-cell-section "pulse_width" "pw" 2 0))
      (oberheim_sem-panel-7 "FILT" 1
        (oberheim_sem-param-cell-section "cutoff" "cut" 0 1)
        (oberheim_sem-param-cell-section "resonance" "res" 2 1)
        (oberheim_sem-param-cell-section "filter_env_amt" "env" 0 1)
        (oberheim_sem-param-cell-section "filter_mode" "mode" 2 1)
        (oberheim_sem-param-cell-section "keytrack" "key" 2 1)
        (oberheim_sem-param-cell-section "filter_vel_amt" "vel" 2 1)
        (oberheim_sem-param-cell-section "filter_drive" "drive" 2 1))
      (oberheim_sem-panel-2 "OUT" 0
        (oberheim_sem-param-cell-section "amp_vel_amt" "vel" 2 0)
        (oberheim_sem-param-cell-section "gain" "gain" 2 0)))))
