;; monomachine/wave3 — DPRO-spirit wavetable machine (dpro-wave-v2 modernized).
;; Dense layout: WAVE/STACK/AMP | GULP/LOFI/GLB | SYNC.

(def wave3-sync-options ()
  '("off" "fixed" "key"))

(def wave3-wave-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "WAVE" 3.6 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bits" "bits" 3.1 0 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "glide_ms" "glide" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wave" "wave" 3.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "wp" "scan" 3.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "phase_morph" "mrph" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)))))

(def wave3-stack-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "STAK" 3.6 (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "stack_detune" "det" 3.4 0 "ct" (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "pan_width" "wid" 3.1 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "stack_semi" "semi" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "stack_level" "lvl" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

(def wave3-amp-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "AMP" 3.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_attack_ms" "atk" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_hold_ms" "hld" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_decay_ms" "dec" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_release_ms" "rel" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def wave3-gulp-block ()
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

(def wave3-lofi-block ()
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

(def wave3-global-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-small-s 1
    (h-stack :gap 0.18 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-badge-s 1 "GLB" 3.6 (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 1 3.0 (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "gain" "gain" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "keytrack" "key" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def wave3-sync-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 2 "SYNC" 3.6 (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "sync_mode" "mode" 4.4 (wave3-sync-options) (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "sfrq" "sfrq" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "drive" "drv" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (wave3-wave-block)
      (wave3-stack-block)
      (wave3-amp-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (wave3-gulp-block)
      (wave3-lofi-block)
      (wave3-global-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-full
      (wave3-sync-block))))
