;; Custom Synth tab body for instruments/emulations/digitone/dsp.lisp

(defstate digitone-selected-section 0)

(def digitone-section (title body)
  (v-stack :gap 0.35
    (label title :font-size 11 :color :dim :bg :transparent)
    body))

(def digitone-select (section)
  (set! digitone-selected-section section))

(def digitone-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= digitone-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))

(def digitone-cell-width 4.0)
(def digitone-filter-cell-width 5.15)

(def digitone-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "digitone-op-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (digitone-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def digitone-param-cell-step-section (name title decimals step section)
  (digitone-param-cell-step-section-width name title decimals step section digitone-cell-width))

(def digitone-filter-mode-label (value)
  (if (= value 1)
    "highpass"
    (if (= value 2) "bandpass" "lowpass")))

(def digitone-filter-mode-value (label)
  (if (= label "highpass")
    1
    (if (= label "bandpass") 2 0)))

(def digitone-mode-dropdown-section (name section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "digitone-mode-dropdown-" name)
        (v-stack :width 4.9 :height 2.05 :gap 0.0 :align :center
          (label "mode" :font-size 10
                 :color :dim :bg :transparent)
          (box :width 0.1 :height 0.18)
          (dropdown :value (digitone-filter-mode-label (get p :value))
            :options '("lowpass" "highpass" "bandpass")
            :width 5.6 :height 1.05 :font-size 8.5
            :on-change (lambda (v)
              (do
                (digitone-select section)
                (fx-set-instrument-value p (digitone-filter-mode-value v)))))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def digitone-param-cell-step (name title decimals step)
  (digitone-param-cell-step-section name title decimals step digitone-selected-section))

(def digitone-param-cell-section (name title decimals section)
  (digitone-param-cell-step-section name title decimals 0 section))

(def digitone-param-cell-section-width (name title decimals section width)
  (digitone-param-cell-step-section-width name title decimals 0 section width))

(def digitone-param-cell (name title decimals)
  (digitone-param-cell-step-section name title decimals 0 digitone-selected-section))

(def digitone-param-number-section (name title decimals unit section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "digitone-adsr-number-" name)
        (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
          (label title :font-size 10
                 :color :dim :bg :transparent)
          (number-picker :value (get p :value)
            :min (get p :min) :max (get p :max) :decimals decimals
            :unit unit
            :noui true :font-size 10.5
            :text-align :center
            :text-color :widget_focus_bg :edit-color :yellow
            :width 5.0 :height 0.95
            :on-change (lambda (v)
              (do
                (digitone-select section)
                (fx-set-instrument-value p v))))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def digitone-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))

(def digitone-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))

(def digitone-adsr-view (title attack decay sustain release section)
  (adsr-editor
    :attack (digitone-param-value attack 4)
    :decay (digitone-param-value decay 400)
    :sustain (digitone-param-value sustain 0.5)
    :release (digitone-param-value release 0)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (digitone-select section)
        (digitone-set-param attack (get env :attack))
        (digitone-set-param decay (get env :decay))
        (digitone-set-param sustain (get env :sustain))
        (digitone-set-param release (get env :release))))))

(def digitone-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (digitone-param-number-section attack "atk" 0 "ms" section)
      (digitone-param-number-section decay "dec" 0 "ms" section)
      (digitone-param-number-section sustain "sus" 2 false section)
      (if release
        (digitone-param-number-section release "rel" 0 "ms" section)
        (box :width 5.2 :height 1.75
          (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
            (label "rel" :font-size 10
                   :color :dim :bg :transparent)
            (number-picker :value 0
              :min 0 :max 0 :decimals 0
              :unit "ms"
              :noui true :font-size 10.5
              :text-align :center
              :text-color :dim :edit-color :dim
              :width 5.0 :height 0.95)))))))


(def digitone-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def digitone-selected-adsr ()
  (if (= digitone-selected-section 1)
    (box :width :fill :height 6.55
         :background-color :instrument-control-bg
         :border-width 1 :corner-radius 16 :padding 0.15
      (v-stack :width :fill :gap 0.10
      (digitone-adsr-view "A ENV" "a_env_attack" "a_env_decay" "a_env_sustain" false 1)
        (digitone-adsr-controls "a_env_attack" "a_env_decay" "a_env_sustain" false 1)
        (digitone-adsr-caption "A ENV")))
    (if (= digitone-selected-section 2)
      (box :width :fill :height 6.55
           :background-color :instrument-control-bg
           :border-width 1 :corner-radius 16 :padding 0.15
        (v-stack :width :fill :gap 0.10
        (digitone-adsr-view "B ENV" "b_env_attack" "b_env_decay" "b_env_sustain" false 2)
          (digitone-adsr-controls "b_env_attack" "b_env_decay" "b_env_sustain" false 2)
          (digitone-adsr-caption "B ENV")))
      (if (= digitone-selected-section 3)
        (box :width :fill :height 6.55
             :background-color :instrument-control-bg
             :border-width 1 :corner-radius 16 :padding 0.15
          (v-stack :width :fill :gap 0.10
          (digitone-adsr-view "F ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release" 3)
            (digitone-adsr-controls "filt_attack" "filt_decay" "filt_sustain" "filt_release" 3)
            (digitone-adsr-caption "FILTER ENV")))
        (box :width :fill :height 6.55
             :background-color :instrument-control-bg
             :border-width 1 :corner-radius 16 :padding 0.15
          (v-stack :width :fill :gap 0.10
          (digitone-adsr-view "AMP" "amp_attack" "amp_decay" "amp_sustain" "amp_release" 0)
            (digitone-adsr-controls "amp_attack" "amp_decay" "amp_sustain" "amp_release" 0)
            (digitone-adsr-caption "AMP ENV")))))))

(def digitone-row-label (title)
  (box :width 1.65 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.5 :width 1.3
           :color :dim :bg :transparent)))

(def digitone-panel-3-section (section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (digitone-panel-bg section)
       :border-width 1
       :corner-radius 16
       :padding 0.1
       :on-click (lambda (info) (digitone-select section))
    (h-stack :width :fill :gap 0.30 :align :start
      c1 c2 c3)))

(def digitone-panel-filter-section (section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (digitone-panel-bg section)
       :border-width 1
       :corner-radius 16
       :padding 0.25
       :on-click (lambda (info) (digitone-select section))
    (h-stack :width :fill :gap 0.60 :align :start
      c1 c2 c3 c4)))

(def digitone-panel-4-section (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (digitone-panel-bg section)
       :border-width 1
       :corner-radius 16
       :padding 0.1
       :on-click (lambda (info) (digitone-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (digitone-row-label title)
      c1 c2 c3 c4)))

(def digitone-panel-4 (title c1 c2 c3 c4)
  (digitone-panel-4-section title digitone-selected-section c1 c2 c3 c4))

(def digitone-panel-5-section (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (digitone-panel-bg section)
       :border-width 1
       :corner-radius 16
       :padding 0.1
       :on-click (lambda (info) (digitone-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (digitone-row-label title)
      c1 c2 c3 c4 c5)))

(def digitone-panel-5 (title c1 c2 c3 c4 c5)
  (digitone-panel-5-section title digitone-selected-section c1 c2 c3 c4 c5))

(def digitone-op-row (op-name section ratio detune level index harmonics octave)
  (box :width :fill :height 2.35
       :background-color (digitone-panel-bg section)
       :border-width 1
       :corner-radius 16
       :padding 0.1
       :on-click (lambda (info) (digitone-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (box :width 1.65 :height 2.1 :h-align :center :v-align :center
        (v-stack :gap 0.1 :align :center
          (label op-name :font-size 9.5 :width 1.3
                 :color :dim :bg :transparent)))
      (digitone-param-cell-step-section ratio "ratio" 2 0.25 section)
      (digitone-param-cell-section detune "detune" 2 section)
      (digitone-param-cell-section level "level" 2 section)
      (if index
        (digitone-param-cell-section index "index" 2 section)
        (box :width digitone-cell-width :height 2.05))
      (digitone-param-cell-section harmonics "harm" 2 section)
      (digitone-param-cell-step-section octave "oct" 0 1 section))))

(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (digitone-op-row "C" 0
        "c_ratio" "c_detune" "c_level" false "c_harmonics" "c_octave")
      (digitone-op-row "A" 1
        "a_ratio" "a_detune" "a_level" "a_index" "a_harmonics" "a_octave")
      (digitone-op-row "B" 2
        "b_ratio" "b_detune" "b_level" "b_index" "b_harmonics" "b_octave"))
    (v-stack :width 23.1 :gap 0.10
      (digitone-selected-adsr))
    (v-stack :width 24.2 :gap 0.10
      (digitone-panel-3-section 0
        (digitone-param-cell-step-section "algorithm" "algo" 0 1 0)
        (digitone-param-cell-section "mix_xy" "x/y" 2 0)
        (digitone-param-cell-section "feedback" "feed" 2 0))
      (digitone-panel-3-section 0
        (digitone-param-cell-step-section "base_note" "note" 0 1 0)
        (digitone-param-cell-section "gain" "gain" 2 0)
        (digitone-param-cell-section "vel_sensitivity" "vel" 2 0))
      (digitone-panel-filter-section 3
        (digitone-mode-dropdown-section "filt_mode" 3)
        (digitone-param-cell-section-width "filt_cutoff" "cut" 0 3 digitone-filter-cell-width)
        (digitone-param-cell-section-width "filt_res" "res" 2 3 digitone-filter-cell-width)
        (digitone-param-cell-section-width "filt_env_depth" "env" 2 3 digitone-filter-cell-width)))))
