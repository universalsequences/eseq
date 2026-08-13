; Membrane Hat UI — lego-s style, family layout of membrane-snare-rim.
; Column A: PEDAL (the expression panel: open + choke + decay/tilt),
; STICK, MIX (small). Column B: PLATES, RATTLE, BODY. Param names match
; dsp.lisp exactly.

; the expression panel: open 0 = pedal tight, 1 = fully open; choke is how
; hard gate-off closes the pedal; decay/tilt voice the ring
(def mh-pedal ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "PEDAL" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "decay" "DEC" 3.6 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tilt" "TILT" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "open" "OPEN" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "choke" "CHOK" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)))))

(def mh-stick ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "STICK" 4.4 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "scrape" "SCRP" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "stick_hard" "HARD" 3.6 3 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "stick_speed" "SPD" 3.6 3 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "strike" "POS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)))))

(def mh-mix ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "MIX" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "sizzle" "SIZL" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bottom_mix" "BTM" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LVL" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def mh-plates ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "PLATES" 4.4 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "plate_ratio" "RAT2" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "wash" "WASH" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "contact_loss" "LOSS" 3.6 3 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tune" "TUNE" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rattle" "RATL" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 1)))))

(def mh-body ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "BODY" 4.4 (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body1_gain" "1 GN" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body2_gain" "2 GN" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body1_freq" "1 HZ" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body2_freq" "2 HZ" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (mh-pedal)
      (mh-stick)
      (mh-mix))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (mh-plates)
      (mh-body))))
