; Modal Snare UI — lego-s style. Two 2-panel columns (STRIKE/HEADS,
; STROKE/WIRES) over one full-width strip (SHAPE + MIX). The strip is a local
; wide lego (both column widths + the gap) so the shaper and mix controls get
; a whole row instead of a third dense panel that overflowed the column.
; Param names match dsp.lisp; wire_drive, rim_drive, dbg are DSP-only.

(def mds-strip-w () (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-col-w)) 0.35))
(def mds-strip (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (mds-strip-w) (eseq.effects.custom-ui-lego/ui-lego-small-h)
    section :instrument-control-bg body))

(def mds-strike ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "STRIKE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "scrape" "SCRP" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bend" "BEND" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bright" "BRT" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "stick_hard" "HARD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 3)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "stick_speed" "SPD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 3)))))

(def mds-heads ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "HEADS" 4.4 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "release2" "REL2" 3.6 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "pitch2_ratio" "RAT2" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tilt" "TILT" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tune" "TUNE" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "release" "REL" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "stretch" "STRCH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(def mds-stroke ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "STROKE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rim_level" "RIM" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rim_pitch" "PTCH" 3.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rim_decay" "RDEC" 3.6 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "stroke" "STRK" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "press" "PRSS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)))))

(def mds-wires ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "WIRES" 4.4 (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "wire_decay" "DEC" 3.6 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "wire_kick" "KICK" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "contact_loss" "LOSS" 3.6 3 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "snare_tension" "TENS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rattle" "RATL" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wire_pitch" "PTCH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)))))

; full-width bottom strip: SHAPE (drive / tone / punch) + MIX
(def mds-shape-mix ()
  (mds-strip 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "SHAPE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "drive" "DRIVE" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tone" "TONE" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "punch" "PUNCH" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (box :width 1.2 :height 0.1)
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "MIX" 3.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "head_couple" "CPL" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "snares" "SNRS" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bottom_mix" "BTM" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LVL" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(defsynth-ui
  (v-stack :width :fill :gap 0.35 :align :start
    (h-stack :width :fill :gap 0.35 :align :stretch
      (eseq.effects.custom-ui-lego/ui-lego-column-2
        (mds-strike)
        (mds-heads))
      (eseq.effects.custom-ui-lego/ui-lego-column-2
        (mds-stroke)
        (mds-wires)))
    (mds-shape-mix)))
