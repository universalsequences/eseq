(def msid-filter-options ()
  '("LP" "BP" "HP"))

(def msid-osc-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OSC" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "osc2_detune" "o2det" 4.4 0 "ct" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "osc3_detune" "o3det" 3.5 0 "ct" (ui-accent-orange))
          (ui-lego-micro-num-s 0 "pulse_width" "pw" 3.5 2 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "osc1_semi" "osc1" 3.7 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "osc2_semi" "osc2" 3.7 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "osc3_semi" "osc3" 3.7 (ui-accent-cyan) 0)))))

(def msid-mix-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "MIX" 3.6 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "pulse_width" "pw" 4.4 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "pulse_mix" "pulse" 3.3 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "noise_mix" "noise" 3.3 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "tri_mix" "tri" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "saw_mix" "saw" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "pulse_mix" "pulse" 3.7 (ui-accent-blue) 2)))))

(def msid-filter-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "FILT" 3.8 (ui-accent-green))
          (ui-lego-micro-option-s 1 "filter_mode" "mode" 4.4 (msid-filter-options) (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "keytrack" "key" 3.5 2 false (ui-accent-green))
          (ui-lego-micro-num-s 1 "filter_fm" "fm" 3.5 2 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "cutoff" "cut" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 1 "resonance" "res" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 1 "drive" "drive" 3.7 (ui-accent-orange) 2)))))

(def msid-edge-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "SID" 3.6 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "sync_amt" "sync" 3.0 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "ring_amt" "ring" 3.0 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "bit_depth" "bits" 3.0 0 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "fold_amt" "fold" 3.0 2 false (ui-accent-orange)))))

(def msid-global-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "GLB" 3.6 (ui-accent-orange))
      (ui-lego-micro-base-note-s 0 3.0 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "glitch_rate" "rate" 3.2 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "glitch_amt" "amt" 3.2 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))))

(def msid-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (ui-control-panel-small-s 0 (box :width :fill :height :fill))
    (ui-detail-adsr-s 0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms")
    (msid-global-block)))

(def msid-voice-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "VOICE" 5.8 (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "osc1_level" "o1" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "osc2_level" "o2" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "osc3_level" "o3" 5.8 2 false (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "pulse_width" "pw" 5.8 2 false (ui-accent-blue)))))

(def msid-edge-strip ()
  (ui-lego-strip-panel-s 0
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-badge-s 0 "EDGE" 5.8 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "bit_depth" "bits" 5.8 0 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "fold_amt" "fold" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "buzz" "buzz" 5.8 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "gain" "gain" 5.8 2 false (ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (msid-osc-block)
      (msid-mix-block)
      (msid-edge-block))
    (msid-detail-column)
    (ui-lego-column
      (msid-filter-block)
      (ui-control-panel-small-s 0
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "LVL" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "osc1_level" "o1" 3.0 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "osc2_level" "o2" 3.0 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "osc3_level" "o3" 3.0 2 false (ui-accent-cyan))))
      (msid-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (msid-voice-strip)
      (msid-edge-strip))))
