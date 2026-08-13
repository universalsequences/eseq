(def dense-voice-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "DRUM" 3.6 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "voice" "drum" 5.8 '("kick" "snare" "lo tom" "hi tom" "rim" "clap" "tamb" "closed" "open" "cym") (eseq.effects.custom-ui-lego/ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "body_wave" "wave" 3.6 '("sin" "saw" "pulse" "clip") (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "filter_mode" "filt" 3.6 '("LP" "BP" "HP" "notch" "peak" "all") (eseq.effects.custom-ui-lego/ui-accent-green))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tune" "tune" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "decay" "dec" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "tone" 3.7 (eseq.effects.custom-ui-lego/ui-accent-green) 2)))))

(def dense-pitch-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "PCH" 3.6 (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "sweep_decay" "time" 3.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "sweep_curve" "crv" 2.8 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "keytrack" "key" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "wobble_rate" "wob" 3.0 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pitch_sweep" "sweep" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pitch_wobble" "warb" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pulse_width" "pw" 3.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)))))

(def dense-mix-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "MIX" 3.6 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "sub_level" "sub" 2.8 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "click_level" "clk" 2.8 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "snap" "snap" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_attack" "atk" 2.8 1 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_release" "rel" 2.8 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body_level" "body" 3.7 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "noise_level" "noise" 3.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "metal_level" "metal" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)))))

(def dense-body-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "BODY" 3.6 (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body_ratio" "ratio" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "partial_spread" "spr" 2.8 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "metal_tune" "mtun" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "metal_spread" "mspr" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "membrane_fm" "FM" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "cross_ring" "ring" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "noise_color" "color" 3.7 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)))))

(def dense-damage-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "MUT" 3.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "noise_res" "nQ" 2.8 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "resonance" "res" 2.8 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 3.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "gain" "gain" 3.0 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "drive" "drv" 3.7 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fold" "fold" 3.7 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "crush" "bits" 3.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

(def lab-readout ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "707 MUTANT LAB" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "note pitched" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "FM / ring" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent)
      (label "metal bank" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent)
      (label "fold+crush" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-blue) :bg :transparent))))

(def env-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :height (eseq.effects.custom-ui-lego/ui-lego-full-h)
      (eseq.effects.custom-ui-lego/ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "decay" "snap" "amp_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (dense-voice-block)
      (dense-pitch-block)
      (lab-readout))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (dense-mix-block)
      (dense-body-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (dense-damage-block)
      (lab-readout))
    (env-column)))