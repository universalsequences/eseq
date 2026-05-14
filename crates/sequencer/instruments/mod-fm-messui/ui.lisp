(def fm-filter-options ()
  '("LP" "BP" "HP" "notch" "peak" "AP"))

(def fm-route-options ()
  '("1>2" "2>1" "par" "F1" "F2"))

(def fm-ops-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OPS" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "op1_ratio" "r1" 2.7 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "op2_ratio" "r2" 2.7 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "op1_detune" "d1" 3.1 0 "ct" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "op2_detune" "d2" 3.1 0 "ct" (ui-accent-blue))
          (ui-lego-micro-num-s 0 "op3_detune" "d3" 3.1 0 "ct" (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "op1_level" "op1" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "op2_level" "op2" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "op3_level" "op3" 3.7 (ui-accent-violet) 2)))))

(def fm-patch-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "FM" 3.6 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "op3_ratio" "r3" 2.8 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "mod_env_to_op2" "e2" 2.8 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "mod_env_to_op3" "e3" 3.1 2 false (ui-accent-green))
          (ui-lego-micro-num-s 0 "lfo_rate" "rate" 3.1 2 "Hz" (ui-accent-green))
          (ui-lego-micro-num-s 0 "lfo_to_pitch" "pit" 3.1 0 "ct" (ui-accent-green))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "op2_to_op1" "2>1" 3.7 (ui-accent-violet) 0)
        (ui-lego-knob-s 0 "op3_to_op2" "3>2" 3.7 (ui-accent-violet) 0)
        (ui-lego-knob-s 0 "lfo_to_index" "idx" 3.7 (ui-accent-green) 2)))))

(def fm-drive-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "OUT" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 2.8 (ui-accent-blue))
      (ui-lego-micro-num-s 0 "op3_to_op1" "3>1" 2.9 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "fold" "fold" 2.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 2.8 2 false (ui-accent-orange)))))

(def fm-filter1-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FIL1" 3.8 (ui-accent-green))
          (ui-lego-micro-option-s 1 "f1_mode" "mode" 4.4 (fm-filter-options) (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "f1_env_amt" "env" 3.1 0 "Hz" (ui-accent-blue))
          (ui-lego-micro-num-s 1 "f1_lfo_amt" "lfo" 3.1 0 "Hz" (ui-accent-green))
          (ui-lego-micro-num-s 1 "f1_drive" "drv" 3.1 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "f1_cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "f1_resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filter_blend" "mix" 3.7 (ui-accent-cyan) 2)))))

(def fm-filter2-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FIL2" 3.8 (ui-accent-cyan))
          (ui-lego-micro-option-s 1 "f2_mode" "mode" 4.4 (fm-filter-options) (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "f2_env_amt" "env" 3.1 0 "Hz" (ui-accent-blue))
          (ui-lego-micro-num-s 1 "f2_lfo_amt" "lfo" 3.1 0 "Hz" (ui-accent-green))
          (ui-lego-micro-num-s 1 "f2_drive" "drv" 3.1 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "f2_cutoff" "cut" 3.7 (ui-accent-cyan) 0)
        (ui-lego-knob-s 1 "f2_resonance" "res" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 1 "drive" "pre" 3.7 (ui-accent-orange) 2)))))

(def fm-route-block ()
  (ui-control-panel-small-s 1
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 1 "ROU" 3.6 (ui-accent-violet))
      (ui-lego-micro-option-s 1 "filter_route" "route" 4.2 (fm-route-options) (ui-accent-violet))
      (ui-lego-micro-num-s 1 "f1_lfo_amt" "F1L" 3.0 0 "Hz" (ui-accent-green))
      (ui-lego-micro-num-s 1 "f2_lfo_amt" "F2L" 3.0 0 "Hz" (ui-accent-green))
      (ui-lego-micro-num-s 1 "filter_blend" "mix" 2.8 2 false (ui-accent-cyan)))))

(def fm-env-detail ()
  (ui-detail-adsr-switch-s
    0 "AMP" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
    1 "MOD" "mod_attack" "mod_decay" "mod_sustain" "mod_release"))

(def fm-mod-readout ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "MOD" 3.6 (ui-accent-green))
      (ui-lego-micro-num-s 0 "mod_attack" "atk" 3.0 0 "ms" (ui-accent-green))
      (ui-lego-micro-num-s 0 "mod_decay" "dec" 3.0 0 "ms" (ui-accent-green))
      (ui-lego-micro-num-s 0 "mod_sustain" "sus" 3.0 2 false (ui-accent-green))
      (ui-lego-micro-num-s 0 "mod_release" "rel" 3.0 0 "ms" (ui-accent-green)))))

(def fm-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (box :width :fill :height (ui-lego-small-h))
    (fm-env-detail)
    (fm-mod-readout)))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (fm-ops-block)
      (fm-patch-block)
      (fm-drive-block))
    (fm-detail-column)
    (ui-lego-column
      (fm-filter1-block)
      (fm-filter2-block)
      (fm-route-block))))
