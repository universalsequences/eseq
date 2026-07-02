; Membrane Snare UI — lego-s style, modelled on drums/membrane-kick ui.lisp
; (inline-badge dense panels + micro numberpickers). Column A: STRIKE, HEADS,
; MIX (small). Column B: WIRES, BODY. Param names match dsp.lisp exactly.

(def ms-strike ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (ui-lego-matrix-s 0 "strike_mask" "MASK" 4.8 3.05 (ui-accent-cyan))
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "STRIKE" 4.4 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "scrape" "SCRP" 3.6 2 false (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "bend" "BEND" 3.6 2 false (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "stick_hard" "HARD" 3.9 (ui-accent-cyan) 3)
        (ui-lego-knob-s 0 "stick_speed" "SPD" 3.9 (ui-accent-blue) 3)))))

(def ms-heads ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "HEADS" 4.4 (ui-accent-green))
          (ui-lego-micro-num-s 0 "release2" "REL2" 3.6 0 "ms" (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "pitch2_ratio" "RAT2" 3.6 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "tone_damp" "DAMP" 3.6 3 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "tune" "TUNE" 3.9 (ui-accent-green) 1)
        (ui-lego-knob-s 0 "release" "REL" 3.9 (ui-accent-green) 0)
        (ui-lego-knob-s 0 "head_couple" "CPL" 3.9 (ui-accent-orange) 2)))))

(def ms-mix ()
  (ui-readout-block-small-s "MIX" (ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (ui-lego-micro-num-s 0 "snares" "SNRS" 3.2 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "bottom_mix" "BTM" 3.2 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "level" "LVL" 3.2 2 false (ui-accent-orange)))))

(def ms-wires ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "WIRES" 4.4 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "wire_decay" "DEC" 3.6 0 "ms" (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "wire_couple" "CPL" 3.6 3 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "snare_tension" "TENS" 3.9 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "rattle" "RATL" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "wire_pitch" "PTCH" 3.9 (ui-accent-blue) 0)))))

(def ms-body ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "BODY" 4.4 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "body1_gain" "1 GN" 3.4 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "body2_gain" "2 GN" 3.4 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "body3_gain" "3 GN" 3.4 2 false (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "body1_freq" "1 HZ" 3.9 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "body2_freq" "2 HZ" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "body3_freq" "3 HZ" 3.9 (ui-accent-violet) 0)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (ms-strike)
      (ms-heads)
      (ms-mix))
    (ui-lego-column-2
      (ms-wires)
      (ms-body))))
