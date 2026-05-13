(def mddrw-wave-block ()
  (ui-control-block-medium-s "DDRW" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "wav1" "wav1" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "mix" "mix" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "wav2" "wav2" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "tune_cents" "tune" 4.8 (ui-accent-orange) 0))))

(def mddrw-global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "drive" "drive" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "mix" "mix" 4.2 2 false (ui-accent-blue)))))

(def mddrw-source-block ()
  (ui-readout-block-small-s "SOURCE" (ui-accent-cyan) 0
    (ui-lego-text-row-4
      (label "wav1" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "<->" :font-size 9.0 :color :dim :bg :transparent)
      (label "wav2" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "draw" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(def mddrw-draw-block ()
  (ui-control-block-medium-s "DRAW" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "time" "time" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "br1" "br1" 4.8 (ui-accent-orange) 0)
      (ui-lego-knob-s 0 "wid" "width" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "br2" "br2" 4.8 (ui-accent-orange) 0))))

(def mddrw-out-block ()
  (ui-readout-block-small-s "OUT" (ui-accent-orange) 0
    (ui-lego-text-row-3
      (label "draw" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
      (label "into" :font-size 9.0 :color :dim :bg :transparent)
      (label "filter" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def mddrw-filter-mod-block ()
  (ui-readout-block-small-s "FILTER MOD" (ui-accent-green) 2
    (ui-lego-text-row-3
      (label "filter env" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "+ key" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
      (label "tracked" :font-size 9.0 :color :dim :bg :transparent))))

(def mddrw-filter-block ()
  (ui-control-block-medium-s "FILTER" (ui-accent-green) 2
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 2 "cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 2 "resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 2 "keytrack" "key" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 2 "filter_env_amt" "env" 4.8 (ui-accent-blue) 0))))

(def mddrw-filter-readout-block ()
  (ui-readout-block-small-s "TOPOLOGY" (ui-accent-green) 2
    (ui-lego-text-row-3
      (label "DDRW" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "through" :font-size 9.0 :color :dim :bg :transparent)
      (label "filter" :font-size 9.0 :color (ui-accent-green) :bg :transparent))))

(def mddrw-adsr-column ()
  (ui-lego-column-full
    (if (= custom-ui-selected-section 2)
      (ui-lego-adsr-s 2 "FILTER ENV" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms")
      (ui-lego-adsr-s 0 "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (mddrw-wave-block)
      (mddrw-global-block)
      (mddrw-source-block))
    (ui-lego-column
      (mddrw-draw-block)
      (mddrw-out-block)
      (mddrw-filter-mod-block))
    (mddrw-adsr-column)
    (ui-lego-column
      (mddrw-filter-block)
      (mddrw-filter-readout-block)
      (ui-readout-block-small-s "OUTPUT" (ui-accent-orange) 0
        (ui-lego-text-row-3
          (label "drive" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
          (label "+" :font-size 9.0 :color :dim :bg :transparent)
          (label "gain" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))))
