(def mdens-dens-block ()
  (ui-control-block-medium-s "DENS" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "wave" "wave" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "pch2" "pch2" 4.8 (ui-accent-orange) 0)
      (ui-lego-knob-s 0 "pch3" "pch3" 4.8 (ui-accent-orange) 0)
      (ui-lego-knob-s 0 "pch4" "pch4" 4.8 (ui-accent-orange) 0))))

(def mdens-global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "drive" "drive" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "tune_cents" "tune" 4.2 0 false (ui-accent-cyan)))))

(def mdens-source-block ()
  (ui-readout-block-small-s "SOURCE" (ui-accent-cyan) 0
    (ui-lego-text-row-4
      (label "dpro" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "wave" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "+ detune" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
      (label "density" :font-size 9.0 :color (ui-accent-violet) :bg :transparent))))

(def mdens-chor-block ()
  (ui-control-block-medium-s "CHORUS" (ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "chrl" "level" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "chrw" "width" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "drive" "drive" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "gain" "gain" 4.8 (ui-accent-orange) 2))))

(def mdens-out-block ()
  (ui-readout-block-small-s "OUT" (ui-accent-orange) 0
    (ui-lego-text-row-3
      (label "chorus" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "into" :font-size 9.0 :color :dim :bg :transparent)
      (label "filter" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def mdens-filter-block ()
  (ui-control-block-medium-s "FILTER" (ui-accent-green) 2
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 2 "cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 2 "resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 2 "keytrack" "key" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 2 "filter_env_amt" "env" 4.8 (ui-accent-blue) 0))))

(def mdens-filter-readout-block ()
  (ui-readout-block-small-s "FILTER MOD" (ui-accent-green) 2
    (ui-lego-text-row-3
      (label "filter env" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "+ key" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
      (label "tracked" :font-size 9.0 :color :dim :bg :transparent))))

(def mdens-adsr-column ()
  (ui-lego-column-full
    (if (= custom-ui-selected-section 2)
      (ui-lego-adsr-s 2 "FILTER ENV" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms")
      (ui-lego-adsr-s 0 "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (mdens-dens-block)
      (mdens-global-block)
      (mdens-source-block))
    (ui-lego-column
      (mdens-chor-block)
      (mdens-out-block)
      (mdens-filter-readout-block))
    (mdens-adsr-column)
    (ui-lego-column
      (mdens-filter-block)
      (ui-readout-block-small-s "TOPOLOGY" (ui-accent-green) 2
        (ui-lego-text-row-3
          (label "DPRO" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
          (label "through" :font-size 9.0 :color :dim :bg :transparent)
          (label "filter" :font-size 9.0 :color (ui-accent-green) :bg :transparent)))
      (ui-readout-block-small-s "OUTPUT" (ui-accent-orange) 0
        (ui-lego-text-row-3
          (label "drive" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
          (label "+" :font-size 9.0 :color :dim :bg :transparent)
          (label "gain" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))))
