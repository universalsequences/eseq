(def wave-options ()
  '("pulse" "saw" "tri" "sine"))

(def noise-options ()
  '("bright" "dark" "thin"))

(def arp-options ()
  '("off" "oct" "min7" "maj7" "jump"))

(def filter-options ()
  '("LP" "BP" "HP"))

(def osc-block ()
  (ui-control-block-medium-s "OSC" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-option-s 0 "osc1_wave" "w1" 4.8 (wave-options) (ui-accent-cyan))
      (ui-lego-knob-s 0 "osc1_level" "1 lvl" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "pulse_width1" "pw1" 4.8 (ui-accent-blue) 2)
      (ui-lego-option-s 0 "osc2_wave" "w2" 4.8 (wave-options) (ui-accent-violet))
      (ui-lego-knob-s 0 "osc2_level" "2 lvl" 4.8 (ui-accent-violet) 2))))

(def tune-block ()
  (ui-control-block-small-s "TUNE" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 0 "osc1_octave" "o1" 4.2 0 "oct" (ui-accent-cyan))
      (ui-lego-num-s 0 "osc1_semitones" "s1" 4.2 0 "st" (ui-accent-cyan))
      (ui-lego-num-s 0 "osc2_octave" "o2" 4.2 0 "oct" (ui-accent-violet))
      (ui-lego-num-s 0 "osc2_semitones" "s2" 4.2 0 "st" (ui-accent-violet)))))

(def bass-noise-block ()
  (ui-readout-block-small-s "BASS+NOISE" (ui-accent-blue) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-knob-s 0 "tri_level" "tri" 4.6 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "noise_level" "noise" 4.6 (ui-accent-orange) 2)
      (ui-lego-option-s 0 "noise_tone" "tone" 5.2 (noise-options) (ui-accent-orange)))))

(def arp-block ()
  (ui-control-block-medium-s "ARP/BLIP" (ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (ui-lego-option-s 2 "arp_pattern" "pat" 5.2 (arp-options) (ui-accent-violet))
      (ui-lego-num-s 2 "arp_steps" "steps" 4.2 0 false (ui-accent-violet))
      (ui-lego-num-s 2 "arp_rate" "rate" 4.6 1 "Hz" (ui-accent-violet))
      (ui-lego-knob-s 2 "pitch_env_amt" "blip" 4.8 (ui-accent-orange) 0))))

(def motion-block ()
  (ui-control-block-small-s "MOTION" (ui-accent-blue) 2
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 2 "vibrato_rate" "vib r" 4.4 1 "Hz" (ui-accent-blue))
      (ui-lego-num-s 2 "vibrato_depth" "vib" 4.4 2 "st" (ui-accent-blue))
      (ui-lego-num-s 2 "pwm_rate" "pwm r" 4.4 1 "Hz" (ui-accent-cyan))
      (ui-lego-num-s 2 "pwm_amount" "pwm" 4.4 2 false (ui-accent-cyan)))))

(def chip-readout-block ()
  (ui-readout-block-small-s "CHIP" (ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (ui-lego-base-note 4.2 (ui-accent-orange))
      (ui-lego-num-s 0 "bit_depth" "bits" 4.2 0 false (ui-accent-orange))
      (ui-lego-num-s 0 "noise_rate" "nrate" 4.6 0 "Hz" (ui-accent-orange)))))

(def filter-block ()
  (ui-control-block-medium-s "FILTER" (ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (ui-lego-option-s 1 "filter_mode" "mode" 4.8 (filter-options) (ui-accent-green))
      (ui-lego-knob-s 1 "cutoff" "cut" 4.8 (ui-accent-green) 0)
      (ui-lego-knob-s 1 "resonance" "res" 4.8 (ui-accent-green) 2)
      (ui-lego-knob-s 1 "filter_env_amt" "env" 4.8 (ui-accent-blue) 0))))

(def color-block ()
  (ui-control-block-small-s "COLOR" (ui-accent-orange) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-knob-s 1 "drive" "drive" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 1 "dry_chip" "raw" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 1 "output_gain" "out" 4.8 (ui-accent-orange) 2))))

(def status-block ()
  (ui-readout-block-small-s "MODE" (ui-accent-blue) 0
    (ui-lego-text-row-3
      (label "8-bit" :font-size 9.0 :color (ui-accent-cyan) :bg :transparent)
      (label "pulse" :font-size 9.0 :color (ui-accent-violet) :bg :transparent)
      (label "blips" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(def env-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-lego-adsr-s 0 "AMP ENV" "amp_attack" "amp_decay" "amp_sustain" "amp_release"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (osc-block)
      (tune-block)
      (bass-noise-block))
    (ui-lego-column
      (arp-block)
      (motion-block)
      (chip-readout-block))
    (env-column)
    (ui-lego-column
      (filter-block)
      (color-block)
      (status-block))))
