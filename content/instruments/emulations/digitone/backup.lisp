
(defsynth-ui
  (tabs :items (list "global" "ops c/a" "op b/env" "filter")
        :bind digitone-section-tab
        :compact true
        :gap 0.75
        :tab-padding 0.5
        :header-height 1.2

    (h-stack :gap 1.2
      (digitone-section "GLOBAL"
        (eseq.effects.custom-ui-runtime/base-note))
      (digitone-section "AMP"
        (params amp_attack amp_decay amp_sustain amp_release))
      (digitone-section "MIX"
        (params algorithm mix_xy feedback vel_sensitivity gain)))
    (h-stack :gap 1.2
      (digitone-section "OP C"
        (params c_ratio c_detune c_level c_harmonics c_octave))
      (digitone-section "OP A"
        (params a_ratio a_detune a_level a_index a_harmonics a_octave)))
    (h-stack :gap 1.2
      (digitone-section "OP B"
        (params b_ratio b_detune b_level b_index b_harmonics b_octave))
      (digitone-section "OP ENVELOPES"
        (params a_env_attack a_env_decay a_env_sustain b_env_attack b_env_decay b_env_sustain)))
    (h-stack :gap 1.2
      (digitone-section "FILTER"
        (params filt_mode filt_cutoff filt_res filt_env_depth))
      (digitone-section "FILTER ENV"
        (params filt_attack filt_decay filt_sustain filt_release)))))
