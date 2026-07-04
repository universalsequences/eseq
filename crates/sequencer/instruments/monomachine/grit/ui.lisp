;; monomachine/grit — SID-spirit gritty machine.
;; Dense 3-column layout: OSC/MODE/AMP | GULP filter/FENV/GLB | LOFI.

(def grit-wave-options ()
  '("tri" "saw" "pulse" "mixed" "noise"))

(def grit-mode-options ()
  '("off" "sync" "ring" "r+s"))

(def grit-osc-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OSC" 3.6 (ui-accent-cyan))
          (ui-lego-micro-option-s 0 "osc_wave" "wave" 4.4 (grit-wave-options) (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "fine_cents" "fin" 3.1 0 "ct" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "bits" "bit" 3.1 0 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "pw" "pw" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "tune_semi" "tune" 3.7 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "osc2_level" "osc2" 3.7 (ui-accent-blue) 2)))))

(def grit-mode-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "MODE" 3.6 (ui-accent-violet))
          (ui-lego-micro-option-s 0 "osc_mode" "mode" 4.4 (grit-mode-options) (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "interlace_hz" "il hz" 3.4 1 "Hz" (ui-accent-violet))
          (ui-lego-micro-num-s 0 "glide_ms" "gli" 3.1 0 "ms" (ui-accent-cyan))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "interlace" "ilace" 3.7 (ui-accent-violet) 2)))))

(def grit-amp-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "AMP" 3.6 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp_attack_ms" "atk" 3.0 0 "ms" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp_hold_ms" "hld" 3.0 0 "ms" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp_decay_ms" "dec" 3.0 0 "ms" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp_release_ms" "rel" 3.0 0 "ms" (ui-accent-orange)))))

(def grit-gulp-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "GULP" 3.6 (ui-accent-green))
          (ui-lego-micro-num-s 1 "keytrack" "key" 3.1 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "flt_res_lo" "rLo" 3.1 1 false (ui-accent-green))
          (ui-lego-micro-num-s 1 "flt_res_hi" "rHi" 3.1 1 false (ui-accent-green))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "flt_base" "base" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "flt_width" "wdth" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "drive" "drv" 3.7 (ui-accent-orange) 2)))))

(def grit-fenv-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FENV" 3.6 (ui-accent-blue))
          (ui-lego-micro-num-s 1 "fenv_attack_ms" "atk" 4.4 0 "ms" (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "fenv_decay_ms" "dec" 4.4 0 "ms" (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "env_to_base" "toB" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 1 "env_to_width" "toW" 3.7 (ui-accent-blue) 2)))))

(def grit-global-block ()
  (ui-control-panel-small-s 1
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 1 "GLB" 3.6 (ui-accent-green))
      (ui-lego-micro-base-note-s 1 3.0 (ui-accent-green))
      (ui-lego-micro-num-s 1 "gain" "gain" 3.0 2 false (ui-accent-green))
      (ui-lego-micro-num-s 1 "pan_width" "wid" 3.0 2 false (ui-accent-violet)))))

(def grit-lofi-block ()
  (ui-control-panel-dense-s 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 2 "LOFI" 3.6 (ui-accent-orange))
          (ui-lego-micro-num-s 2 "am_rate" "am rate" 4.4 1 "Hz" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "eq_freq" "eq frq" 3.7 0 "Hz" (ui-accent-blue))
          (ui-lego-micro-num-s 2 "eq_q" "eq Q" 3.1 1 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 2 "am_depth" "AM" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 2 "srr" "srr" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 2 "eq_gain_db" "EQ" 3.7 (ui-accent-blue) 1)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (grit-osc-block)
      (grit-mode-block)
      (grit-amp-block))
    (ui-lego-column
      (grit-gulp-block)
      (grit-fenv-block)
      (grit-global-block))
    (ui-lego-column-full
      (grit-lofi-block))))
