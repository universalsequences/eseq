;; monomachine/melt — FM+ DYNAMIC-spirit machine (FM where the ratios move).
;; Dense layout: OP1/OP2/AMP | GULP/LOFI/GLB | FM+PERF strips.

(def melt-snap-options ()
  '("free" "snap"))

(def melt-op1-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "OP1" 3.6 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "op1_attack_ms" "atk" 4.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "op1_decay_ms" "dec" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "op1_sustain" "sus" 3.1 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ratio1" "rato" 3.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "idx1" "idx" 3.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sweep1" "swp" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)))))

(def melt-op2-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "OP2" 3.6 (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "op2_attack_ms" "atk" 4.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "op2_decay_ms" "dec" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "op2_sustain" "sus" 3.1 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ratio2" "rato" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "idx2" "idx" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sweep2" "swp" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)))))

(def melt-amp-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "AMP" 3.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_attack_ms" "atk" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_hold_ms" "hld" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_decay_ms" "dec" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_release_ms" "rel" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def melt-gulp-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 1 "GULP" 3.6 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "flt_res_lo" "rLo" 3.1 1 false (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "flt_res_hi" "rHi" 3.1 1 false (eseq.effects.custom-ui-lego/ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "fenv_attack_ms" "atk" 3.1 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "fenv_decay_ms" "dec" 3.1 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "env_to_width" "toW" 3.1 1 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "flt_base" "base" 3.7 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "flt_width" "wdth" 3.7 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "env_to_base" "toB" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

(def melt-lofi-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 1 "LOFI" 3.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "am_rate" "am rate" 4.4 1 "Hz" (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "eq_freq" "eq frq" 3.7 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "eq_q" "eq Q" 3.1 1 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "am_depth" "AM" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "srr" "srr" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "eq_gain_db" "EQ" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 1)))))

(def melt-global-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-small-s 1
    (h-stack :gap 0.18 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 1 "GLB" 3.6 (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 1 3.0 (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "gain" "gain" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "keytrack" "key" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def melt-fm-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 2 "FM" 3.6 (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ratio_snap" "ratio" 4.4 (melt-snap-options) (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "glide_ms" "glide" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "pan_width" "wid" 3.1 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "feedback" "fdbk" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "stack" "stak" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "drive" "drv" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (melt-op1-block)
      (melt-op2-block)
      (melt-amp-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (melt-gulp-block)
      (melt-lofi-block)
      (melt-global-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (melt-fm-block))))
