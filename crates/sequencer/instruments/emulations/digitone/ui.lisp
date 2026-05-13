(def digitone-filter-mode-label (value)
  (if (= value 1)
    "highpass"
    (if (= value 2) "bandpass" "lowpass")))

(def digitone-filter-mode-value (label)
  (if (= label "highpass")
    1
    (if (= label "bandpass") 2 0)))

(def digitone-mode-dropdown (section)
  (let ((p (inst-param synth-ui-current-inst "filt_mode")))
    (if p
      (let ((scope (custom-ui-current-scope)))
        (subtree :key (str "digitone-mode-dropdown-" synth-ui-current-name)
          (v-stack :width 5.0 :height 1.12 :gap 0.08 :align :start
            (label "mode" :font-size 8.2 :width 5.0 :color :dim :bg :transparent)
            (dropdown :value (digitone-filter-mode-label (get p :value))
              :options '("lowpass" "highpass" "bandpass")
              :width 5.6 :height 0.78 :font-size 8.0
              :on-change (lambda (v)
                (do
                  (custom-ui-select-section-in-scope scope section)
                  (custom-ui-set-param-in-scope scope p (digitone-filter-mode-value v))))))))
      (label "missing: filt_mode" :font-size 9 :color :red :bg :transparent))))

(def digitone-op-c-block ()
  (ui-control-block-medium-s "OP C" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "c_ratio" "ratio" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "c_level" "level" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "c_harmonics" "harm" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "c_octave" "oct" 4.8 (ui-accent-orange) 0))))

(def digitone-op-c-detail ()
  (ui-readout-block-small-s "OP C DETAIL" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-num-s 0 "c_detune" "detune" 4.7 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "algorithm" "algo" 4.7 0 false (ui-accent-violet))
      (ui-lego-num-s 0 "mix_xy" "x/y" 4.7 2 false (ui-accent-blue)))))

(def digitone-carrier-readout ()
  (ui-readout-block-small-s "CARRIER" (ui-accent-cyan) 0
    (ui-lego-text-row-3
      (label "carrier" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "mix" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "feedback" :font-size 9.0 :color (ui-accent-violet) :bg :transparent))))

(def digitone-op-a-block ()
  (ui-control-block-medium-s "OP A" (ui-accent-blue) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "a_ratio" "ratio" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 1 "a_level" "level" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 1 "a_index" "index" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 1 "a_octave" "oct" 4.8 (ui-accent-orange) 0))))

(def digitone-op-a-detail ()
  (ui-readout-block-small-s "OP A DETAIL" (ui-accent-blue) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-num-s 1 "a_detune" "detune" 4.7 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "a_harmonics" "harm" 4.7 2 false (ui-accent-blue))
      (ui-lego-num-s 1 "feedback" "feed" 4.7 2 false (ui-accent-violet)))))

(def digitone-op-a-readout ()
  (ui-readout-block-small-s "A ENV" (ui-accent-blue) 1
    (ui-lego-text-row-3
      (label "modulator A" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "index" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "env" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def digitone-op-b-block ()
  (ui-control-block-medium-s "OP B" (ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 2 "b_ratio" "ratio" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 2 "b_level" "level" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 2 "b_index" "index" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 2 "b_octave" "oct" 4.8 (ui-accent-orange) 0))))

(def digitone-op-b-detail ()
  (ui-readout-block-small-s "OP B DETAIL" (ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (ui-lego-num-s 2 "b_detune" "detune" 4.7 2 false (ui-accent-orange))
      (ui-lego-num-s 2 "b_harmonics" "harm" 4.7 2 false (ui-accent-violet))
      (ui-lego-num-s 0 "vel_sensitivity" "vel" 4.7 2 false (ui-accent-green)))))

(def digitone-op-b-readout ()
  (ui-readout-block-small-s "B ENV" (ui-accent-violet) 2
    (ui-lego-text-row-3
      (label "modulator B" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "index" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "env" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def digitone-filter-block ()
  (ui-control-block-medium-s "FILTER" (ui-accent-green) 3
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 3 "filt_cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 3 "filt_res" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 3 "filt_env_depth" "env" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "gain" "gain" 4.8 (ui-accent-orange) 2))))

(def digitone-filter-detail ()
  (ui-readout-block-small-s "FILTER DETAIL" (ui-accent-green) 3
    (h-stack :gap 0.32 :align :start
      (digitone-mode-dropdown 3)
      (ui-lego-num-s 0 "algorithm" "algo" 4.7 0 false (ui-accent-violet))
      (ui-lego-num-s 0 "mix_xy" "mix" 4.7 2 false (ui-accent-blue)))))

(def digitone-global-readout ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-num-s 0 "base_note" "note" 4.7 0 false (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.7 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "vel_sensitivity" "vel" 4.7 2 false (ui-accent-green)))))

(def digitone-adsr-column ()
  (ui-lego-column-full
    (if (= custom-ui-selected-section 1)
      (ui-lego-adsr-s 1 "A ENV" "a_env_attack" "a_env_decay" "a_env_sustain" false)
      (if (= custom-ui-selected-section 2)
        (ui-lego-adsr-s 2 "B ENV" "b_env_attack" "b_env_decay" "b_env_sustain" false)
        (if (= custom-ui-selected-section 3)
          (ui-lego-adsr-s 3 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release")
          (ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (digitone-op-c-block)
      (digitone-op-c-detail)
      (digitone-carrier-readout))
    (ui-lego-column
      (digitone-op-a-block)
      (digitone-op-a-detail)
      (digitone-op-a-readout))
    (digitone-adsr-column)
    (ui-lego-column
      (digitone-op-b-block)
      (digitone-op-b-detail)
      (digitone-op-b-readout))
    (ui-lego-column
      (digitone-filter-block)
      (digitone-filter-detail)
      (digitone-global-readout))))
