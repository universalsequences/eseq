; Membrane Kick UI — lego-s style, modelled on drums/md-hat (inline-badge dense
; panels + a small numberpicker readout, which fits where title-header blocks do
; not). Column A: EXCITER, PRIMARY membrane, MIX (small). Column B: BODY,
; SECONDARY membrane. PRIMARY and SECONDARY sit side by side (same dense size).
; Param names match dsp.lisp exactly.

(def mk-exciter ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "EXCITER" 4.4 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "impulse_decay" "IMP" 3.6 0 "ms" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "feedback" "FBK" 3.6 2 false (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "freq" "FREQ" 3.9 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "shape2" "FM" 3.9 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "shape" "HIT" 3.9 (ui-accent-violet) 2)))))

(def mk-primary ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "PRIMARY" 4.4 (ui-accent-green))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "tune" "TUNE" 3.9 (ui-accent-green) 1)
        (ui-lego-knob-s 0 "release" "REL" 3.9 (ui-accent-green) 0)
        (ui-lego-knob-s 0 "coupling" "CPL" 3.9 (ui-accent-orange) 2)))))

(def mk-mix ()
  (ui-readout-block-small-s "MIX" (ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (ui-lego-micro-num-s 0 "mixer" "MIX" 3.2 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "multi" "MULT" 3.2 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "level" "LVL" 3.2 2 false (ui-accent-orange)))))

(def mk-body ()
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

(def mk-secondary ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "SECONDARY" 4.4 (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "pitch2_ratio" "RAT" 3.9 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "release2" "REL" 3.9 (ui-accent-violet) 0)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (mk-exciter)
      (mk-primary)
      (mk-mix))
    (ui-lego-column-2
      (mk-body)
      (mk-secondary))))
