(def mfpar-op-block (section title freq env wave mix accent)
  (ui-control-panel-dense-s section
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s section title 3.6 accent)
          (ui-lego-micro-num-s section wave "wave" 4.4 2 false accent))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s section env "env" 3.3 2 false (ui-accent-blue))
          (ui-lego-micro-num-s section mix "mix" 3.3 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s section freq "frq" 3.7 accent 2)
        (ui-lego-knob-s section env "env" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s section mix "mix" 3.7 (ui-accent-violet) 2)))))

(def mfpar-carrier-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "CAR" 3.6 (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "car_wave" "wave" 3.2 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "car_mix" "mix" 3.2 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "tone" "tone" 3.2 2 false (ui-accent-green))
      (ui-lego-micro-num-s 0 "tune_cents" "tune" 3.2 0 "ct" (ui-accent-orange)))))

(def mfpar-filter-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FILT" 3.8 (ui-accent-green))
          (ui-lego-micro-num-s 1 "keytrack" "key" 4.4 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "filter_env_amt" "env" 3.5 0 false (ui-accent-blue))
          (ui-lego-micro-num-s 1 "drive" "drive" 3.5 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "filter_env_amt" "env" 3.7 (ui-accent-blue) 0)))))

(def mfpar-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "drive" "drive" 3.0 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def mfpar-env-detail ()
  (if (= custom-ui-selected-section 2)
    (ui-detail-adsr-s 2 "OP2" "op2_attack_ms" "op2_decay_ms" "op2_sustain" "op2_release_ms")
    (if (= custom-ui-selected-section 3)
      (ui-detail-adsr-s 3 "OP3" "op3_attack_ms" "op3_decay_ms" "op3_sustain" "op3_release_ms")
      (if (= custom-ui-selected-section 1)
        (ui-detail-adsr-s 1 "FILTER" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms")
        (ui-detail-adsr-s 0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms")))))

(def mfpar-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 0 (box :width :fill :height :fill))
    (mfpar-env-detail)
    (mfpar-global-block)))

(def mfpar-op-strip (section title freq env wave mix accent)
  (ui-lego-strip-panel-s section
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s section title 5.8 accent)
      (ui-lego-micro-num-s section freq "frq" 5.8 2 false accent)
      (ui-lego-micro-num-s section env "env" 5.8 2 false (ui-accent-blue))
      (ui-lego-micro-num-s section wave "wave" 5.8 2 false accent)
      (ui-lego-micro-num-s section mix "mix" 5.8 2 false (ui-accent-violet)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (mfpar-op-block 0 "OP1" "op1_frq" "op1_env" "op1_wave" "op1_mix" (ui-accent-cyan))
      (mfpar-op-block 2 "OP2" "op2_frq" "op2_env" "op2_wave" "op2_mix" (ui-accent-blue))
      (mfpar-carrier-block))
    (mfpar-detail-column)
    (ui-lego-column
      (mfpar-op-block 3 "OP3" "op3_frq" "op3_env" "op3_wave" "op3_mix" (ui-accent-violet))
      (mfpar-filter-block)
      (mfpar-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (mfpar-op-strip 0 "OP1" "op1_frq" "op1_env" "op1_wave" "op1_mix" (ui-accent-cyan))
      (mfpar-op-strip 2 "OP2" "op2_frq" "op2_env" "op2_wave" "op2_mix" (ui-accent-blue)))))
