;; Custom Synth tab body for instruments/emulations/monomachine-superwave/dsp.lisp
(defstate monomachine-superwave-selected-section 0)
(def monomachine_superwave-select (section)
  (set! monomachine-superwave-selected-section section))
(def monomachine_superwave-panel-bg (section)
  (if (= section 0)
    (rgba 0.09 0.09 0.09 1)
    (if (= monomachine-superwave-selected-section section)
      (rgba 0.12 0.12 0.12 1)
      (rgba 0.09 0.09 0.09 1))))
(def monomachine_superwave-cell-width 4.0)
(def monomachine_superwave-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "monomachine_superwave-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (monomachine_superwave-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def monomachine_superwave-param-cell-step-section (name title decimals step section)
  (monomachine_superwave-param-cell-step-section-width name title decimals step section monomachine_superwave-cell-width))
(def monomachine_superwave-param-cell-section (name title decimals section)
  (monomachine_superwave-param-cell-step-section name title decimals 0 section))
(def monomachine_superwave-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "monomachine_superwave-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width monomachine_superwave-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (monomachine_superwave-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def monomachine_superwave-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "monomachine_superwave-adsr-number-" name)
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
                  (monomachine_superwave-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :gray :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :gray :edit-color :gray
          :width 5.0 :height 0.95)))))
(def monomachine_superwave-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def monomachine_superwave-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def monomachine_superwave-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (monomachine_superwave-param-value attack 4)
    :decay (monomachine_superwave-param-value decay 400)
    :sustain (monomachine_superwave-param-value sustain 0.5)
    :release (monomachine_superwave-param-value release 0)
    :width 22.0 :height 3.55
    :background-color (rgba 0.0 0.0 0.0 1)
    :on-change (lambda (env)
      (do
        (monomachine_superwave-select section)
        (monomachine_superwave-set-param attack (get env :attack))
        (monomachine_superwave-set-param decay (get env :decay))
        (monomachine_superwave-set-param sustain (get env :sustain))
        (monomachine_superwave-set-param release (get env :release))))))
(def monomachine_superwave-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-param-number-section attack "atk" 0 "ms" section)
      (monomachine_superwave-param-number-section decay "dec" 0 "ms" section)
      (monomachine_superwave-param-number-section sustain "sus" 2 false section)
      (monomachine_superwave-param-number-section release "rel" 0 "ms" section))))

(def monomachine_superwave-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :gray :bg :transparent)))
(def monomachine_superwave-selected-adsr ()
  (if (= monomachine-superwave-selected-section 1)
    (box :width :fill :height 6.55
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (monomachine_superwave-adsr-view "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (monomachine_superwave-adsr-controls "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (monomachine_superwave-adsr-caption "FILTER ENV")))
    (box :width :fill :height 6.55
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (monomachine_superwave-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (monomachine_superwave-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (monomachine_superwave-adsr-caption "AMP ENV")))))
(def monomachine_superwave-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :gray :bg :transparent)))
(def monomachine_superwave-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (monomachine_superwave-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_superwave-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-row-label title)
      c1)))
(def monomachine_superwave-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (monomachine_superwave-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_superwave-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-row-label title)
      c1 c2)))
(def monomachine_superwave-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (monomachine_superwave-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_superwave-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-row-label title)
      c1 c2 c3)))
(def monomachine_superwave-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (monomachine_superwave-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_superwave-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-row-label title)
      c1 c2 c3 c4)))
(def monomachine_superwave-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (monomachine_superwave-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_superwave-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-row-label title)
      c1 c2 c3 c4 c5)))
(def monomachine_superwave-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (monomachine_superwave-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_superwave-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def monomachine_superwave-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (monomachine_superwave-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_superwave-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def monomachine_superwave-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (monomachine_superwave-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_superwave-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_superwave-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (monomachine_superwave-panel-1 "GLOB" 0
        (monomachine_superwave-base-note-cell 0))
      (monomachine_superwave-panel-5 "MIX" 0
        (monomachine_superwave-param-cell-section "saw_mix" "saw" 2 0)
        (monomachine_superwave-param-cell-section "pulse_mix" "pulse" 2 0)
        (monomachine_superwave-param-cell-section "pulse_width" "pw" 2 0)
        (monomachine_superwave-param-cell-section "sub_level" "sub" 2 0)
        (monomachine_superwave-param-cell-section "noise_level" "noise" 2 0))
      (monomachine_superwave-panel-4 "SUPER" 0
        (monomachine_superwave-param-cell-section "detune_cents" "det" 0 0)
        (monomachine_superwave-param-cell-section "motion_rate" "rate" 2 0)
        (monomachine_superwave-param-cell-section "motion_depth" "depth" 2 0)
        (monomachine_superwave-param-cell-section "phase_smear" "smear" 2 0)))
    (v-stack :width 23.1 :gap 0.10
      (monomachine_superwave-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (monomachine_superwave-panel-6 "FILT" 1
        (monomachine_superwave-param-cell-section "cutoff" "cut" 0 1)
        (monomachine_superwave-param-cell-section "resonance" "res" 2 1)
        (monomachine_superwave-param-cell-section "filter_env_amt" "env" 0 1)
        (monomachine_superwave-param-cell-section "keytrack" "key" 2 1)
        (monomachine_superwave-param-cell-section "drive" "drive" 2 1)
        (monomachine_superwave-param-cell-section "brightness" "bright" 2 1))
      (monomachine_superwave-panel-6 "TEX" 0
        (monomachine_superwave-param-cell-section "swarm" "swarm" 2 0)
        (monomachine_superwave-param-cell-section "comb_amt" "comb" 2 0)
        (monomachine_superwave-param-cell-section "comb_time" "time" 2 0)
        (monomachine_superwave-param-cell-section "fm_smear" "fm" 2 0)
        (monomachine_superwave-param-cell-section "pwm_warp" "pwm" 2 0)
        (monomachine_superwave-param-cell-section "chaos" "chaos" 2 0))
      (monomachine_superwave-panel-1 "OUT" 0
        (monomachine_superwave-param-cell-section "gain" "gain" 2 0)))))
