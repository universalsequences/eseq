(def digitone-filter-mode-label (value)
  (if (= value 1)
    "highpass"
    (if (= value 2) "bandpass" "lowpass")))

(def digitone-filter-mode-value (label)
  (if (= label "highpass")
    1
    (if (= label "bandpass") 2 0)))

(def digitone-mode-dropdown (section)
  (let ((p (eseq.effects.custom-ui-runtime/inst-param synth-ui-current-inst "filt_mode")))
    (if p
      (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (subtree :key (str "digitone-mode-dropdown-" synth-ui-current-name)
          (v-stack :width 5.0 :height 1.12 :gap 0.08 :align :start
            (label "mode" :font-size 8.2 :width 5.0 :color :dim :bg :transparent)
            (dropdown :value (digitone-filter-mode-label (get p :value))
              :options '("lowpass" "highpass" "bandpass")
              :width 5.6 :height 0.78 :font-size 8.0
              :on-change (lambda (v)
                (do
                  (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                  (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (digitone-filter-mode-value v))))))))
      (label "missing: filt_mode" :font-size 9 :color :red :bg :transparent))))

(def digitone-op-c-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OP C" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "c_ratio" "ratio" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "c_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "c_harmonics" "harm" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "c_octave" "oct" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0))))

(def digitone-op-c-detail ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "OP C DETAIL" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "c_detune" "detune" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "algorithm" "algo" 4.7 0 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "mix_xy" "x/y" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def digitone-carrier-readout ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "CARRIER" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-3
      (label "carrier" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "mix" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent)
      (label "feedback" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent))))

(def digitone-op-a-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OP A" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "a_ratio" "ratio" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "a_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "a_index" "index" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "a_octave" "oct" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0))))

(def digitone-op-a-detail ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "OP A DETAIL" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "a_detune" "detune" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "a_harmonics" "harm" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "feedback" "feed" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))))

(def digitone-op-a-readout ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "A ENV" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (eseq.effects.custom-ui-lego/ui-lego-text-row-3
      (label "modulator A" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent)
      (label "index" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent)
      (label "env" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent))))

(def digitone-op-b-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "OP B" (eseq.effects.custom-ui-lego/ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "b_ratio" "ratio" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "b_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "b_index" "index" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "b_octave" "oct" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0))))

(def digitone-op-b-detail ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "OP B DETAIL" (eseq.effects.custom-ui-lego/ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "b_detune" "detune" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "b_harmonics" "harm" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "vel_sensitivity" "vel" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def digitone-op-b-readout ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "B ENV" (eseq.effects.custom-ui-lego/ui-accent-violet) 2
    (eseq.effects.custom-ui-lego/ui-lego-text-row-3
      (label "modulator B" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent)
      (label "index" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent)
      (label "env" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent))))

(def digitone-filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "FILTER" (eseq.effects.custom-ui-lego/ui-accent-green) 3
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 3 "filt_cutoff" "cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 3 "filt_res" "res" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 3 "filt_env_depth" "env" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "gain" "gain" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def digitone-filter-detail ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "FILTER DETAIL" (eseq.effects.custom-ui-lego/ui-accent-green) 3
    (h-stack :gap 0.32 :align :start
      (digitone-mode-dropdown 3)
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "algorithm" "algo" 4.7 0 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "mix_xy" "mix" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def digitone-global-readout ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "base_note" "note" 4.7 0 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "vel_sensitivity" "vel" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def digitone-adsr-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (if (= custom-ui-selected-section 1)
      (eseq.effects.custom-ui-lego/ui-lego-adsr-s 1 "A ENV" "a_env_attack" "a_env_decay" "a_env_sustain" false)
      (if (= custom-ui-selected-section 2)
        (eseq.effects.custom-ui-lego/ui-lego-adsr-s 2 "B ENV" "b_env_attack" "b_env_decay" "b_env_sustain" false)
        (if (= custom-ui-selected-section 3)
          (eseq.effects.custom-ui-lego/ui-lego-adsr-s 3 "FILTER ENV" "filt_attack" "filt_decay" "filt_sustain" "filt_release")
          (eseq.effects.custom-ui-lego/ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (digitone-op-c-block)
      (digitone-op-c-detail)
      (digitone-carrier-readout))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (digitone-op-a-block)
      (digitone-op-a-detail)
      (digitone-op-a-readout))
    (digitone-adsr-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (digitone-op-b-block)
      (digitone-op-b-detail)
      (digitone-op-b-readout))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (digitone-filter-block)
      (digitone-filter-detail)
      (digitone-global-readout))))
