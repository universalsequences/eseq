; Membrane Snare Rim UI — lego-s style, extends drums/membrane-snare ui.lisp.
; Column A: STRIKE, HEADS, MIX (small). Column B: STROKE (the expression
; panel: stroke morph + press + rim hoop), WIRES, BODY. Param names match
; dsp.lisp exactly.

(def msr-strike ()
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

(def msr-heads ()
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

(def msr-mix ()
  (ui-readout-block-small-s "MIX" (ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (ui-lego-micro-num-s 0 "snares" "SNRS" 3.2 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "bottom_mix" "BTM" 3.2 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "level" "LVL" 3.2 2 false (ui-accent-orange)))))

; the expression panel: stroke 0 = ghost, 0.5 = open, 1 = rimshot; press is
; a hand laid on the head; RIM micros voice the metal hoop
(def msr-stroke ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "STROKE" 4.4 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "rim_level" "RIM" 3.6 2 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "rim_pitch" "PTCH" 3.6 0 "Hz" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "rim_decay" "RDEC" 3.6 0 "ms" (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "stroke" "STRK" 3.9 (ui-accent-orange) 2)
        (ui-lego-knob-s 0 "press" "PRSS" 3.9 (ui-accent-violet) 2)))))

(def msr-wires ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "WIRES" 4.4 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "wire_decay" "DEC" 3.6 0 "ms" (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "wire_couple" "CPL" 3.6 3 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "rim_drive" "RDRV" 3.6 3 false (ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "snare_tension" "TENS" 3.9 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "rattle" "RATL" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "wire_pitch" "PTCH" 3.9 (ui-accent-blue) 0)))))

(def msr-body ()
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
      (msr-strike)
      (msr-heads)
      (msr-mix))
    (ui-lego-column
      (msr-stroke)
      (msr-wires)
      (msr-body))))
