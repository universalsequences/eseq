(def hat-main-block ()
  (ui-control-block-medium-s "909 OPEN HAT" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "decay_ms" "decay" 4.8 (ui-accent-cyan) 0)
      (ui-lego-knob-s 0 "tune" "tune" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "metal_mix" "metal" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "swoosh" "swoosh" 4.8 (ui-accent-orange) 2))))

(def hat-filter-block ()
  (ui-control-block-medium-s "SWEEP FILTER" (ui-accent-green) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "cutoff" "tone" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 0 "resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 0 "air" "air" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "drive" "drive" 4.8 (ui-accent-orange) 2))))

(def hat-mix-block ()
  (ui-readout-block-small-s "SOURCE MIX" (ui-accent-violet) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "noise_mix" "noise" 4.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 0 "body_level" "body" 4.2 2 false (ui-accent-violet))
      (ui-lego-num-s 0 "attack_ms" "atk" 4.2 1 "ms" (ui-accent-blue))
      (ui-lego-num-s 0 "release_ms" "rel" 4.2 0 "ms" (ui-accent-blue)))))

(def hat-global-block ()
  (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "gain" "gain" 4.2 2 false (ui-accent-orange))
      (ui-lego-num-s 0 "decay_ms" "decay" 4.2 0 "ms" (ui-accent-cyan))
      (ui-lego-num-s 0 "cutoff" "tone" 4.2 0 "Hz" (ui-accent-green)))))

(def hat-info-block ()
  (ui-readout-block-small-s "CHARACTER" (ui-accent-blue) 0
    (ui-lego-text-row-4
      (label "metal bank" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "swept BP" :font-size 9.0 :color (ui-accent-green) :bg :transparent)
      (label "airy tail" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "909 wash" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (hat-main-block)
      (hat-mix-block))
    (ui-lego-column-2
      (hat-filter-block)
      (hat-info-block))
    (ui-lego-column-full
      (hat-global-block))))