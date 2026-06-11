;; Drift — warm analog character: orange/ice/pink palette, source tabs with
;; vertical gain faders, chip toggles instead of tiny dropdowns, accent-striped
;; tinted panels (built on the ui-lego-*-x / tab / fader / chip pieces).

(def drift-orange () (rgba 1.00 0.55 0.18 1.0))
(def drift-ice    () (rgba 0.45 0.78 1.00 1.0))
(def drift-pink   () (rgba 1.00 0.36 0.46 1.0))
(def drift-cream  () (rgba 0.93 0.88 0.78 1.0))
(def drift-violet () (rgba 0.72 0.55 1.00 1.0))

(def drift-surf-warm () (rgba 0.096 0.082 0.070 1.0))
(def drift-surf-cool () (rgba 0.066 0.080 0.094 1.0))
(def drift-surf-dark () (rgba 0.055 0.058 0.064 1.0))

(def drift-bord-warm () (rgba 0.42 0.26 0.10 0.60))
(def drift-bord-cool () (rgba 0.14 0.30 0.44 0.60))
(def drift-bord-dark () (rgba 0.22 0.22 0.26 0.60))

(def drift-panel-dense (section surface border stripe body)
  (ui-lego-panel-x-s section (ui-lego-col-w) (ui-lego-dense-h) surface border stripe body))
(def drift-panel-small (section surface border stripe body)
  (ui-lego-panel-x-s section (ui-lego-col-w) (ui-lego-small-h) surface border stripe body))
(def drift-panel-strip (section surface border stripe body)
  (ui-lego-panel-x-s section (ui-lego-strip-w) (ui-lego-full-h) surface border stripe body))

(def drift-wave1-options ()
  '("sine" "tri" "shark" "sat" "saw" "pulse" "rect"))

(def drift-wave2-options ()
  '("sine" "tri" "sat" "saw" "rect"))

(def drift-src-options ()
  '("env1" "env2" "lfo" "key" "vel"))

(def drift-lfo-wave-options ()
  '("sine" "tri" "sawU" "sawD" "sqr" "s&h" "wndr"))

(def drift-dest-options ()
  '("o1 gain" "o1 shp" "o2 gain" "o2 det" "nz gain" "lp frq" "lp res" "hp frq" "vol"))

(def drift-route-options ()
  '("dry" "filt"))

(def drift-onoff-options ()
  '("off" "on"))

(def drift-ftype-options ()
  '("I 12dB" "II 24dB"))

(def drift-lfo-mode-options ()
  '("hz" "ratio"))

(def drift-env2-mode-options ()
  '("adsr" "cyc"))

(def drift-retrig-options ()
  '("free" "trig"))

(def drift-osc1-block ()
  (drift-panel-dense 0 (drift-surf-warm) (drift-bord-warm) (drift-orange)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.6 :gap 0.18 :align :start
        (h-stack :gap 0.20 :align :end
          (box :width 1.3 :height 1.18 :v-align :end
            (ui-lego-tab-s 0 "1" 1.3 0.92 (drift-orange) :black))
          (ui-lego-micro-option-s 0 "osc1_wave" "wave" 4.4 (drift-wave1-options) (drift-orange))
          (ui-lego-micro-num-s 0 "osc1_octave" "oct" 2.4 0 false (drift-cream)))
        (h-stack :gap 0.20 :align :start
          (ui-lego-micro-option-s 0 "osc1_route" "route" 3.4 (drift-route-options) (drift-ice))
          (ui-lego-micro-option-s 0 "osc1_shape_src" "shp src" 4.4 (drift-src-options) (drift-cream))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "osc1_shape" "shape" 3.7 (drift-orange) 2)
        (ui-lego-knob-s 0 "osc1_shape_amt" "shp A" 3.7 (drift-cream) 2))
      (ui-lego-fader-s 0 "osc1_gain_db" 2.3 1.95 (drift-orange) 1 false))))

(def drift-osc2-block ()
  (drift-panel-dense 0 (drift-surf-warm) (drift-bord-warm) (drift-ice)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.6 :gap 0.18 :align :start
        (h-stack :gap 0.20 :align :end
          (box :width 1.3 :height 1.18 :v-align :end
            (ui-lego-tab-s 0 "2" 1.3 0.92 (drift-ice) :black))
          (ui-lego-micro-option-s 0 "osc2_wave" "wave" 4.4 (drift-wave2-options) (drift-ice))
          (ui-lego-micro-num-s 0 "osc2_octave" "oct" 2.4 0 false (drift-cream)))
        (h-stack :gap 0.20 :align :start
          (ui-lego-micro-option-s 0 "osc2_route" "route" 3.4 (drift-route-options) (drift-ice))
          (ui-lego-micro-num-s 0 "voice_pan" "pan" 3.1 2 false (drift-cream))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "osc2_detune" "det" 3.7 (drift-ice) 1)
        (ui-lego-knob-s 0 "spread" "sprd" 3.7 (drift-cream) 2))
      (ui-lego-fader-s 0 "osc2_gain_db" 2.3 1.95 (drift-ice) 1 false))))

(def drift-source-block ()
  (drift-panel-small 0 (drift-surf-warm) (drift-bord-warm) (drift-pink)
    (h-stack :gap 0.24 :align :end
      (box :width 1.3 :height 1.18 :v-align :end
        (ui-lego-tab-s 0 "N" 1.3 0.92 (drift-pink) :black))
      (ui-lego-micro-num-s 0 "noise_gain_db" "noise" 3.2 0 "dB" (drift-pink))
      (ui-lego-micro-option-s 0 "noise_route" "nz route" 3.4 (drift-route-options) (drift-ice))
      (ui-lego-micro-option-s 0 "osc1_on" "osc1" 3.4 (drift-onoff-options) (drift-orange))
      (ui-lego-micro-option-s 0 "osc2_on" "osc2" 3.4 (drift-onoff-options) (drift-ice)))))

(def drift-cyc-block ()
  (drift-panel-small 0 (drift-surf-dark) (drift-bord-dark) false
    (h-stack :gap 0.22 :align :end
      (ui-lego-header-s 0 "CYC" 2.4 (drift-cream))
      (ui-lego-micro-option-s 0 "env2_mode" "mode" 3.6 (drift-env2-mode-options) (drift-cream))
      (ui-lego-micro-num-s 0 "cyc_rate_hz" "rate" 3.4 1 "Hz" (drift-cream))
      (ui-lego-micro-num-s 0 "cyc_tilt" "tilt" 2.9 2 false (drift-cream))
      (ui-lego-micro-num-s 0 "cyc_hold" "hold" 2.9 2 false (drift-cream)))))

(def drift-env-detail ()
  (ui-detail-adsr-switch-s
    0 "ENV1 AMP" "env1_attack" "env1_decay" "env1_sustain" "env1_release"
    1 "ENV2 MOD" "env2_attack" "env2_decay" "env2_sustain" "env2_release"))

(def drift-global-block ()
  (drift-panel-small 0 (drift-surf-dark) (drift-bord-dark) false
    (h-stack :gap 0.22 :align :end
      (ui-lego-header-s 0 "GLB" 2.4 (drift-cream))
      (ui-lego-micro-base-note-s 0 2.8 (drift-cream))
      (ui-lego-micro-num-s 0 "glide_ms" "glide" 2.9 0 "ms" (drift-cream))
      (ui-lego-micro-num-s 0 "volume_db" "vol" 3.4 1 "dB" (drift-orange))
      (ui-lego-micro-num-s 0 "vel_to_vol" "vel" 2.7 2 false (drift-cream)))))

(def drift-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (drift-cyc-block)
    (drift-env-detail)
    (drift-global-block)))

(def drift-filter-block ()
  (drift-panel-dense 1 (drift-surf-cool) (drift-bord-cool) (drift-ice)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (ui-lego-header-s 1 "FILTER" 4.2 (drift-ice))
          (ui-lego-micro-option-s 1 "filter_type" "type" 4.4 (drift-ftype-options) (drift-orange)))
        (h-stack :gap 0.20 :align :start
          (ui-lego-micro-num-s 1 "keytrack" "keytrack" 3.6 2 false (drift-ice))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 1 "lp_freq" "cut" 3.7 (drift-ice) 0)
        (ui-lego-knob-s 1 "lp_res" "res" 3.7 (drift-ice) 2)
        (ui-lego-knob-s 1 "hp_freq" "hp" 3.7 (drift-cream) 0)))))

(def drift-pitch-mod-block ()
  (drift-panel-dense 1 (drift-surf-warm) (drift-bord-warm) (drift-violet)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (ui-lego-header-s 1 "PITCH" 3.6 (drift-orange))
          (ui-lego-micro-option-s 1 "pitch_mod1_src" "src1" 4.4 (drift-src-options) (drift-orange)))
        (h-stack :gap 0.22 :align :end
          (ui-lego-micro-option-s 1 "pitch_mod2_src" "src2" 4.4 (drift-src-options) (drift-orange))
          (ui-lego-header-s 1 "DRIFT" 3.6 (drift-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 1 "pitch_mod1_amt" "amt1" 3.7 (drift-orange) 1)
        (ui-lego-knob-s 1 "pitch_mod2_amt" "amt2" 3.7 (drift-orange) 1)
        (ui-lego-knob-s 1 "drift" "drift" 3.7 (drift-violet) 2)))))

(def drift-filter-mod-block ()
  (drift-panel-small 1 (drift-surf-cool) (drift-bord-cool) false
    (h-stack :gap 0.22 :align :end
      (ui-lego-header-s 1 "FMOD" 2.8 (drift-ice))
      (ui-lego-micro-option-s 1 "lp_mod1_src" "src1" 3.4 (drift-src-options) (drift-ice))
      (ui-lego-micro-num-s 1 "lp_mod1_amt" "amt1" 2.7 1 false (drift-ice))
      (ui-lego-micro-option-s 1 "lp_mod2_src" "src2" 3.4 (drift-src-options) (drift-ice))
      (ui-lego-micro-num-s 1 "lp_mod2_amt" "amt2" 2.7 1 false (drift-ice)))))

(def drift-lfo-strip ()
  (drift-panel-strip 2 (drift-surf-cool) (drift-bord-cool) (drift-violet)
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-header-s 2 "LFO" 5.6 (drift-violet))
      (ui-lego-micro-option-s 2 "lfo_wave" "wave" 5.6 (drift-lfo-wave-options) (drift-violet))
      (ui-lego-micro-option-s 2 "lfo_mode" "mode" 5.6 (drift-lfo-mode-options) (drift-violet))
      (ui-lego-micro-option-s 2 "lfo_retrig" "retrig" 5.6 (drift-retrig-options) (drift-orange))
      (ui-lego-micro-num-s 2 "lfo_rate_hz" "rate" 5.6 2 "Hz" (drift-violet))
      (ui-lego-micro-num-s 2 "lfo_ratio" "ratio" 5.6 2 false (drift-violet))
      (ui-lego-micro-num-s 2 "lfo_amount" "amt" 5.6 2 false (drift-violet)))))

(def drift-matrix-strip ()
  (drift-panel-strip 2 (drift-surf-warm) (drift-bord-warm) (drift-orange)
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-header-s 2 "MOD" 5.6 (drift-orange))
      (ui-lego-micro-option-s 2 "mm1_src" "src1" 5.6 (drift-src-options) (drift-orange))
      (ui-lego-micro-option-s 2 "mm1_dest" "dst1" 5.6 (drift-dest-options) (drift-orange))
      (ui-lego-micro-num-s 2 "mm1_amt" "amt1" 5.6 2 false (drift-orange))
      (ui-lego-micro-option-s 2 "mm2_src" "src2" 5.6 (drift-src-options) (drift-cream))
      (ui-lego-micro-option-s 2 "mm2_dest" "dst2" 5.6 (drift-dest-options) (drift-cream))
      (ui-lego-micro-num-s 2 "mm2_amt" "amt2" 5.6 2 false (drift-cream)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (drift-osc1-block)
      (drift-osc2-block)
      (drift-source-block))
    (drift-detail-column)
    (ui-lego-column
      (drift-filter-block)
      (drift-pitch-mod-block)
      (drift-filter-mod-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (drift-lfo-strip)
      (drift-matrix-strip))))
