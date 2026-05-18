(def dense-voice-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "DRUM" 3.6 (ui-accent-cyan))
          (ui-lego-micro-option-s 0 "voice" "drum" 6.0 '("kick" "snare" "lo tom" "mid tom" "hi tom" "rim" "clap" "closed" "open" "ride" "crash") (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-option-s 0 "body_wave" "wave" 3.6 '("sin" "saw" "pulse" "tri") (ui-accent-blue))
          (ui-lego-micro-option-s 0 "filter_mode" "filt" 3.6 '("LP" "BP" "HP" "notch" "peak" "all") (ui-accent-green))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "tune" "tune" 3.7 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "decay" "dec" 3.7 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "tone" "tone" 3.7 (ui-accent-green) 2)))))

(def dense-pitch-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "PCH" 3.6 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "sweep_decay" "time" 3.0 0 "ms" (ui-accent-blue))
          (ui-lego-micro-num-s 0 "sweep_curve" "crv" 2.8 2 false (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "keytrack" "key" 3.0 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "punch" "punch" 3.0 2 false (ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "pitch_sweep" "sweep" 3.7 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "membrane_fm" "FM" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "pulse_width" "pw" 3.7 (ui-accent-cyan) 2)))))

(def dense-mix-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "MIX" 3.6 (ui-accent-green))
          (ui-lego-micro-num-s 0 "sub_level" "sub" 2.8 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "click_level" "clk" 2.8 2 false (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "snap" "snap" 3.0 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "amp_attack" "atk" 2.8 1 "ms" (ui-accent-violet))
          (ui-lego-micro-num-s 0 "amp_release" "rel" 2.8 0 "ms" (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "body_level" "body" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "noise_level" "noise" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "metal_level" "metal" 3.7 (ui-accent-violet) 2)))))

(def dense-metal-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "MTL" 3.6 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "metal_tune" "mtun" 3.0 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "metal_spread" "mspr" 3.0 2 false (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "hats_decay" "hat" 3.0 0 "ms" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "noise_res" "nQ" 3.0 2 false (ui-accent-green))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "body_ratio" "ratio" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "partial_spread" "spr" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "noise_color" "color" 3.7 (ui-accent-cyan) 2)))))

(def dense-damage-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "OUT" 3.6 (ui-accent-orange))
          (ui-lego-micro-base-note-s 0 3.1 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "resonance" "res" 3.0 2 false (ui-accent-green))
          (ui-lego-micro-num-s 0 "cross_ring" "ring" 3.0 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "drive" "drv" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 0 "fold" "fold" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "crush" "bits" 3.7 (ui-accent-blue) 2)))))

(def lab-readout ()
  (ui-readout-block-small-s "909 DROPDOWN" (ui-accent-cyan) 0
    (ui-lego-text-row-4
      (label "FM everywhere" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "ratio clap" :font-size 9.0 :color (ui-accent-blue) :bg :transparent)
      (label "metal hats" :font-size 9.0 :color (ui-accent-orange) :bg :transparent)
      (label "one voice" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent))))

(def env-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "decay" "snap" "amp_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (dense-voice-block)
      (dense-pitch-block)
      (lab-readout))
    (ui-lego-column-2
      (dense-mix-block)
      (dense-metal-block))
    (ui-lego-column-2
      (dense-damage-block)
      (lab-readout))
    (env-column)))