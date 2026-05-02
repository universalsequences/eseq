;; Custom Synth tab body for instruments/emulations/digitone/dsp.lisp

(defstate digitone-selected-section 0)

(def digitone-section (title body)
  (v-stack :gap 0.35
    (label title :font-size 11 :color :gray :bg :transparent)
    body))

(def digitone-select (section)
  (set! digitone-selected-section section))

(def digitone-panel-bg (section)
  (if (= digitone-selected-section section)
    (rgba 0.19 0.19 0.19 1)
    (rgba 0.14 0.14 0.14 1)))

(def digitone-param-cell-step-section (name title decimals step section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "digitone-op-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width 4.75 :height 2.05
          :on-change (lambda (v)
            (do
              (digitone-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def digitone-param-cell-step (name title decimals step)
  (digitone-param-cell-step-section name title decimals step digitone-selected-section))

(def digitone-param-cell-section (name title decimals section)
  (digitone-param-cell-step-section name title decimals 0 section))

(def digitone-param-cell (name title decimals)
  (digitone-param-cell-step-section name title decimals 0 digitone-selected-section))

(def digitone-param-number-section (name title decimals section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "digitone-adsr-number-" name)
        (v-stack :width 4.75 :height 1.75 :gap 0.0 :align :center
          (label title :font-size 10 :width 4.6
                 :color :gray :bg :transparent)
          (number-picker :value (get p :value)
            :min (get p :min) :max (get p :max) :decimals decimals
            :noui true :font-size 10.5
            :text-color :gray :edit-color :yellow
            :width 4.6 :height 0.95
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
    :width 22.0 :height 4.0
    :background-color (rgba 0.0 0.0 0.0 1)
    :on-change (lambda (env)
      (do
        (digitone-select section)
        (digitone-set-param attack (get env :attack))
        (digitone-set-param decay (get env :decay))
        (digitone-set-param sustain (get env :sustain))
        (digitone-set-param release (get env :release))))))

(def digitone-adsr-controls (attack decay sustain release section)
  (h-stack :width :fill :gap 0.35 :align :start
    (digitone-param-number-section attack "atk" 0 section)
    (digitone-param-number-section decay "dec" 0 section)
    (digitone-param-number-section sustain "sus" 2 section)
    (if release
      (digitone-param-number-section release "rel" 0 section)
      (box :width 4.75 :height 1.75))))

(def digitone-selected-adsr ()
  (if (= digitone-selected-section 1)
    (box :width :fill :height 6.35
         :background-color (rgba 0.0 0.0 0.0 1)
         :border-width 1 :corner-radius 16 :padding 0.15
      (v-stack :width :fill :gap 0.10
      (digitone-adsr-view "A ENV" "a_env_attack" "a_env_decay" "a_env_sustain" false 1)
        (digitone-adsr-controls "a_env_attack" "a_env_decay" "a_env_sustain" false 1)))
    (if (= digitone-selected-section 2)
      (box :width :fill :height 6.35
           :background-color (rgba 0.0 0.0 0.0 1)
           :border-width 1 :corner-radius 16 :padding 0.15
        (v-stack :width :fill :gap 0.10
        (digitone-adsr-view "B ENV" "b_env_attack" "b_env_decay" "b_env_sustain" false 2)
          (digitone-adsr-controls "b_env_attack" "b_env_decay" "b_env_sustain" false 2)))
      (if (= digitone-selected-section 3)
        (box :width :fill :height 6.35
             :background-color (rgba 0.0 0.0 0.0 1)
             :border-width 1 :corner-radius 16 :padding 0.15
          (v-stack :width :fill :gap 0.10
          (digitone-adsr-view "F ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release" 3)
            (digitone-adsr-controls "filt_attack" "filt_decay" "filt_sustain" "filt_release" 3)))
        (box :width :fill :height 6.35
             :background-color (rgba 0.0 0.0 0.0 1)
             :border-width 1 :corner-radius 16 :padding 0.15
          (v-stack :width :fill :gap 0.10
          (digitone-adsr-view "AMP" "amp_attack" "amp_decay" "amp_sustain" "amp_release" 0)
            (digitone-adsr-controls "amp_attack" "amp_decay" "amp_sustain" "amp_release" 0)))))))

(def digitone-row-label (title)
  (box :width 5.0 :height 2.1 :h-align :start :v-align :center :padding 0.35
    (label title :font-size 10.0 :width 4.8
           :color :gray :bg :transparent)))

(def digitone-panel-4-section (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (digitone-panel-bg section)
       :border-width 1
       :corner-radius 16
       :padding 0.1
       :on-click (lambda (info) (digitone-select section))
    (h-stack :width :fill :gap 0.35 :align :start
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
    (h-stack :width :fill :gap 0.35 :align :start
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
    (h-stack :width :fill :gap 0.35 :align :start
      (box :width 3.0 :height 2.1 :h-align :center :v-align :center
        (v-stack :gap 0.1 :align :center
          (label op-name :font-size 11 :width 2.2
                 :color :gray :bg :transparent)))
      (digitone-param-cell-step-section ratio "ratio" 2 0.25 section)
      (digitone-param-cell-section detune "detune" 2 section)
      (digitone-param-cell-section level "level" 2 section)
      (if index
        (digitone-param-cell-section index "index" 2 section)
        (box :width 4.75 :height 2.05))
      (digitone-param-cell-section harmonics "harm" 2 section)
      (digitone-param-cell-step-section octave "oct" 0 1 section))))

(defsynth-ui
  (h-stack :width :fill :gap 0.5 :align :start
    (v-stack :width 32 :gap 0.10
      (digitone-op-row "C" 0
        "c_ratio" "c_detune" "c_level" false "c_harmonics" "c_octave")
      (digitone-op-row "A" 1
        "a_ratio" "a_detune" "a_level" "a_index" "a_harmonics" "a_octave")
      (digitone-op-row "B" 2
        "b_ratio" "b_detune" "b_level" "b_index" "b_harmonics" "b_octave"))
    (v-stack :width 23 :gap 0.10
      (digitone-selected-adsr))
    (v-stack :width 32 :gap 0.10
      (digitone-panel-5-section "GLOBAL" 0
        (digitone-param-cell-step-section "base_note" "note" 0 1 0)
        (digitone-param-cell-step-section "algorithm" "algo" 0 1 0)
        (digitone-param-cell-section "mix_xy" "x/y" 2 0)
        (digitone-param-cell-section "feedback" "feed" 2 0)
        (digitone-param-cell-section "gain" "gain" 2 0))
      (digitone-panel-4-section "PERF" 0
        (digitone-param-cell-section "vel_sensitivity" "vel" 2 0)
        (box :width 4.75 :height 2.05)
        (box :width 4.75 :height 2.05)
        (box :width 4.75 :height 2.05))
      (digitone-panel-4-section "FILTER" 3
        (digitone-param-cell-step-section "filt_mode" "mode" 0 1 3)
        (digitone-param-cell-section "filt_cutoff" "cut" 0 3)
        (digitone-param-cell-section "filt_res" "res" 2 3)
        (digitone-param-cell-section "filt_env_depth" "env" 2 3)))))
