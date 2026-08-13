(def vco1-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "VCO 1" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-option-s 0 "vco1_wave" "wave" 5.2 '("saw" "pulse") (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco1_pw" "pw" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco1_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(def vco2-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "VCO 2" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-option-s 0 "vco2_wave" "wave" 5.2 '("saw" "tri") (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco2_pitch" "pitch" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 1)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco2_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(def mixer-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "MIXER / UTILITY" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ring_level" "ring" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "noise_level" "noise" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco_cross_mod" "x-mod" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))

(def mod-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "MODULATION" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vco2_to_cutoff" "vco2->cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "lfo_rate" "lfo rate" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "lfo_to_lpf" "lfo->lpf" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0))))

(def hpf-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "HIGH PASS FILTER" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "hpf_cutoff" "cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "hpf_resonance" "res" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "eg1_to_hpf" "eg1" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0))))

(def lpf-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "LOW PASS FILTER" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "lpf_cutoff" "cut" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "lpf_resonance" "res" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "eg1_to_lpf" "eg1" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0))))

(def envelope-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :height (eseq.effects.custom-ui-lego/ui-lego-full-h)
      (eseq.effects.custom-ui-lego/ui-adsr-switch
        0 "AMP ENV (EG2)" "eg2_attack" "eg2_decay" "eg2_sustain" "eg2_release"
        1 "FILTER ENV (EG1)" "eg1_attack" "eg1_decay" "eg1_sustain" "eg1_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (vco1-block)
      (vco2-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (mixer-block)
      (mod-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (hpf-block)
      (lpf-block))
    (envelope-column)))
