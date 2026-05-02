;; Custom Synth tab body for instruments/emulations/digitone/dsp.lisp

(defstate digitone-section-tab 0)

(def digitone-section (title body)
  (v-stack :gap 0.35
    (label title :font-size 11 :color :gray :bg :transparent)
    body))

(def digitone-param-cell-step (name title decimals step)
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
          :on-change (lambda (v) (fx-set-instrument-value p v))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def digitone-param-cell (name title decimals)
  (digitone-param-cell-step name title decimals 0))

(def digitone-row-label (title)
  (box :width 5.0 :height 2.1 :h-align :start :v-align :center :padding 0.35
    (label title :font-size 10.0 :width 4.8
           :color :gray :bg :transparent)))

(def digitone-panel-4 (title c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (rgba 0.14 0.14 0.14 1)
       :border-width 1
       :corner-radius 16
       :padding 0.1
    (h-stack :width :fill :gap 0.35 :align :start
      (digitone-row-label title)
      c1 c2 c3 c4)))

(def digitone-panel-5 (title c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (rgba 0.14 0.14 0.14 1)
       :border-width 1
       :corner-radius 16
       :padding 0.1
    (h-stack :width :fill :gap 0.35 :align :start
      (digitone-row-label title)
      c1 c2 c3 c4 c5)))

(def digitone-op-row (op-name ratio detune level index harmonics octave)
  (box :width :fill :height 2.35
       :background-color (rgba 0.14 0.14 0.14 1)
       :border-width 1
       :corner-radius 16
       :padding 0.1
    (h-stack :width :fill :gap 0.35 :align :start
      (box :width 3.0 :height 2.1 :h-align :center :v-align :center
        (v-stack :gap 0.1 :align :center
          (label op-name :font-size 11 :width 2.2
                 :color :gray :bg :transparent)))
      (digitone-param-cell-step ratio "ratio" 2 0.25)
      (digitone-param-cell detune "detune" 2)
      (digitone-param-cell level "level" 2)
      (if index
        (digitone-param-cell index "index" 2)
        (box :width 4.75 :height 2.05))
      (digitone-param-cell harmonics "harm" 2)
      (digitone-param-cell-step octave "oct" 0 1))))

(def digitone-op-env-row ()
  (box :width :fill :height 2.35
       :background-color (rgba 0.14 0.14 0.14 1)
       :border-width 1
       :corner-radius 16
       :padding 0.1
    (h-stack :width :fill :gap 0.35 :align :start
      (box :width 3.0 :height 2.1 :h-align :center :v-align :center
        (v-stack :gap 0.1 :align :center
          (label "ENV" :font-size 12 :width 2.8
                 :color :gray :bg :transparent)
          (label "A/B" :font-size 9 :width 2.8
                 :color :gray :bg :transparent)))
      (digitone-param-cell "a_env_attack" "a atk" 0)
      (digitone-param-cell "a_env_decay" "a dec" 0)
      (digitone-param-cell "a_env_sustain" "a sus" 2)
      (digitone-param-cell "b_env_attack" "b atk" 0)
      (digitone-param-cell "b_env_decay" "b dec" 0)
      (digitone-param-cell "b_env_sustain" "b sus" 2))))

(defsynth-ui
  (tabs :items (list "global" "operators" "filter")
        :bind digitone-section-tab
        :compact true
        :gap 0.75
        :tab-padding 0
        :header-height 1.2

    (v-stack :width :fill :gap 0.10
      (digitone-panel-5 "GLOBAL"
        (digitone-param-cell-step "base_note" "note" 0 1)
        (digitone-param-cell-step "algorithm" "algo" 0 1)
        (digitone-param-cell "mix_xy" "x/y" 2)
        (digitone-param-cell "feedback" "feed" 2)
        (digitone-param-cell "gain" "gain" 2))
      (digitone-panel-4 "AMP"
        (digitone-param-cell "amp_attack" "atk" 0)
        (digitone-param-cell "amp_decay" "dec" 0)
        (digitone-param-cell "amp_sustain" "sus" 2)
        (digitone-param-cell "amp_release" "rel" 0))
      (digitone-panel-4 "PERF"
        (digitone-param-cell "vel_sensitivity" "vel" 2)
        (box :width 4.75 :height 2.05)
        (box :width 4.75 :height 2.05)
        (box :width 4.75 :height 2.05)))
    (v-stack :width :fill :gap 0.10
      (digitone-op-row "C"
        "c_ratio" "c_detune" "c_level" false "c_harmonics" "c_octave")
      (digitone-op-row "A"
        "a_ratio" "a_detune" "a_level" "a_index" "a_harmonics" "a_octave")
      (digitone-op-row "B"
        "b_ratio" "b_detune" "b_level" "b_index" "b_harmonics" "b_octave")
      (digitone-op-env-row))
    (v-stack :width :fill :gap 0.10
      (digitone-panel-4 "FILTER"
        (digitone-param-cell-step "filt_mode" "mode" 0 1)
        (digitone-param-cell "filt_cutoff" "cut" 0)
        (digitone-param-cell "filt_res" "res" 2)
        (digitone-param-cell "filt_env_depth" "env" 2))
      (digitone-panel-4 "F ENV"
        (digitone-param-cell "filt_attack" "atk" 0)
        (digitone-param-cell "filt_decay" "dec" 0)
        (digitone-param-cell "filt_sustain" "sus" 2)
        (digitone-param-cell "filt_release" "rel" 0)))))
