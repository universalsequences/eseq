(def vco1-block ()
  (ui-control-block-medium-s "VCO 1" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-option-s 0 "vco1_wave" "wave" 5.2 '("saw" "pulse") (ui-accent-cyan))
      (ui-lego-knob-s 0 "vco1_pw" "pw" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "vco1_level" "level" 4.8 (ui-accent-cyan) 2))))

(def vco2-block ()
  (ui-control-block-medium-s "VCO 2" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-option-s 0 "vco2_wave" "wave" 5.2 '("saw" "tri") (ui-accent-cyan))
      (ui-lego-knob-s 0 "vco2_pitch" "pitch" 4.8 (ui-accent-cyan) 1)
      (ui-lego-knob-s 0 "vco2_level" "level" 4.8 (ui-accent-cyan) 2))))

(def mixer-block ()
  (ui-control-block-medium-s "MIXER / UTILITY" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "ring_level" "ring" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "noise_level" "noise" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "vco_cross_mod" "x-mod" 4.8 (ui-accent-orange) 2))))

(def mod-block ()
  (ui-control-block-medium-s "MODULATION" (ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "vco2_to_cutoff" "vco2->cut" 4.8 (ui-accent-blue) 0)
      (ui-lego-knob-s 0 "lfo_rate" "lfo rate" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "lfo_to_lpf" "lfo->lpf" 4.8 (ui-accent-blue) 0))))

(def hpf-block ()
  (ui-control-block-medium-s "HIGH PASS FILTER" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "hpf_cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "hpf_resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "eg1_to_hpf" "eg1" 4.8 (ui-accent-green) 0))))

(def lpf-block ()
  (ui-control-block-medium-s "LOW PASS FILTER" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 1 "lpf_cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "lpf_resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "eg1_to_lpf" "eg1" 4.8 (ui-accent-green) 0))))

(def envelope-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-adsr-switch
        0 "AMP ENV (EG2)" "eg2_attack" "eg2_decay" "eg2_sustain" "eg2_release"
        1 "FILTER ENV (EG1)" "eg1_attack" "eg1_decay" "eg1_sustain" "eg1_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column-2
      (vco1-block)
      (vco2-block))
    (ui-lego-column-2
      (mixer-block)
      (mod-block))
    (ui-lego-column-2
      (hpf-block)
      (lpf-block))
    (envelope-column)))
