(def msw-osc-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "WAVE" 3.8 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "detune_cents" "det" 4.4 0 "ct" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "pulse_width" "pw" 3.3 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "sub_level" "sub" 3.3 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "saw_mix" "saw" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "pulse_mix" "pulse" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "detune_cents" "det" 3.7 (ui-accent-orange) 0)))))

(def msw-motion-block ()
  (ui-control-panel-dense-s 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 2 "MOVE" 3.8 (ui-accent-blue))
          (ui-lego-micro-num-s 2 "motion_rate" "rate" 4.4 2 "Hz" (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "phase_smear" "phase" 3.5 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 2 "swarm" "swarm" 3.5 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 2 "motion_rate" "rate" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 2 "motion_depth" "depth" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 2 "phase_smear" "phase" 3.7 (ui-accent-violet) 2)))))

(def msw-filter-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FILT" 3.8 (ui-accent-green))
          (ui-lego-micro-num-s 1 "keytrack" "key" 4.4 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "filter_env_amt" "env" 3.5 0 false (ui-accent-blue))
          (ui-lego-micro-num-s 1 "brightness" "brt" 3.5 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filter_env_amt" "env" 3.7 (ui-accent-blue) 0)))))

(def msw-color-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "COLOR" 4.2 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "brightness" "brt" 4.0 2 false (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "noise_level" "noise" 3.5 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "drive" "drive" 3.5 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "comb_amt" "comb" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "fm_smear" "fm" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "pwm_warp" "pwm" 3.7 (ui-accent-blue) 2)))))

(def msw-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drive" "drive" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def msw-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 2 (box :width :fill :height :fill))
    (ui-detail-adsr-switch-s
      0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
      1 "FILTER" "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms")
    (msw-global-block)))

(def msw-motion-strip ()
  (ui-lego-strip-panel-s 2
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 2 "MOVE" 5.8 (ui-accent-blue))
      (ui-lego-micro-num-s 2 "motion_rate" "rate" 5.8 2 "Hz" (ui-accent-blue))
      (ui-lego-micro-num-s 2 "motion_depth" "depth" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 2 "phase_smear" "phase" 5.8 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 2 "swarm" "swarm" 5.8 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 2 "chaos" "chaos" 5.8 2 false (ui-accent-orange)))))

(def msw-color-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "COLOR" 5.8 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "comb_amt" "comb" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "comb_time" "time" 5.8 0 "smp" (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "fm_smear" "fm" 5.8 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "pwm_warp" "pwm" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "chaos" "chaos" 5.8 2 false (ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (msw-osc-block)
      (msw-motion-block)
      (msw-global-block))
    (msw-detail-column)
    (ui-lego-column
      (msw-filter-block)
      (msw-color-block)
      (ui-control-panel-small-s 0
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "SRC" 3.6 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "sub_level" "sub" 3.0 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "noise_level" "noise" 3.2 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (msw-motion-strip)
      (msw-color-strip))))
