;; Factory Drift — warm analog character expressed through semantic theme
;; colors, with source tabs, vertical gain faders, and accent-striped panels.
;; vs core/drift: the filter panel exposes the new filter_drive knob
;; (cut / res / drive); hp cutoff moved to a micro control beside keytrack.

(def drift-orange () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def drift-ice    () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def drift-pink   () :red)
(def drift-cream  () :fg)
(def drift-violet () (eseq.effects.custom-ui-lego/ui-accent-violet))

(def drift-surf-warm () :mixer-control-bg)
(def drift-surf-cool () :instrument-group-bg)
(def drift-surf-dark () :instrument-control-bg)

(def drift-bord-warm () :border-inactive)
(def drift-bord-cool () :border-inactive)
(def drift-bord-dark () :border-inactive)

(def drift-panel-dense (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) surface border stripe body))
(def drift-panel-small (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) surface border stripe body))
(def drift-panel-strip (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (* 2.0 (eseq.effects.custom-ui-lego/ui-lego-strip-w)) (eseq.effects.custom-ui-lego/ui-lego-full-h) surface border stripe body))

;; small panels: one 1.18-high control row, vertically centered in the panel
(def drift-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

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

(def drift-ftype-options ()
  '("I 12dB" "II 24dB"))

(def drift-lfo-mode-options ()
  '("hz" "ratio"))

(def drift-env2-mode-options ()
  '("adsr" "cyc"))

(def drift-retrig-options ()
  '("free" "trig"))


;; Oscillator on/off as the builtin toggle widget, with a micro-style title.
(def drift-osc-toggle (name title accent)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if p
      (let ((on (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)) 0.5)))
        (subtree :key (str "drift-osc-toggle-" name "-" (if on 1 0))
          (v-stack :width 3.4 :height 1.18 :gap 0.06 :align :start
            (label title :font-size 7.4 :width 3.4 :height 0.56 :color :dim :bg :transparent)
            (toggle
              :value on
              :color accent
              :off-color :instrument-control-bg
              :knob-color :black
              :off-knob-color :dim
              :on-change (lambda (next-on)
                (do
                  (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 0)
                  (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if next-on 1 0))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def drift-osc1-block ()
  ;(drift-panel-dense 0 (drift-surf-warm) (drift-bord-warm) (drift-orange)
  (h-stack :width :fill :height :fill :gap 0.30 :align :center
    (v-stack :width 9.6 :gap 0.18 :align :start
      (h-stack :gap 0.20 :align :end
        (box :width 1.3 :height 1.18 :v-align :end
          (eseq.effects.custom-ui-lego/ui-lego-tab-s 0 "1" 1.3 0.92 (drift-orange) :black))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc1_wave" "wave" 4.8 (drift-wave1-options) (drift-orange))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_octave" "oct" 2.4 0 false (drift-cream)))
      (h-stack :gap 0.20 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc1_route" "route" 3.4 (drift-route-options) (drift-ice))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc1_shape_src" "shp src" 4.4 (drift-src-options) (drift-cream))))
    (h-stack :gap 0.10 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc1_shape" "shape" 3.7 (drift-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc1_shape_amt" "shp A" 3.7 (drift-cream) 2))
    (eseq.effects.custom-ui-lego/ui-lego-fader-s 0 "osc1_gain_db" 2.3 1.95 (drift-orange) 1 false)))

(def drift-osc2-block ()
  ;(drift-panel-dense 0 (drift-surf-warm) (drift-bord-warm) (drift-ice)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.6 :gap 0.18 :align :start
        (h-stack :gap 0.20 :align :end
          (box :width 1.3 :height 1.18 :v-align :end
            (eseq.effects.custom-ui-lego/ui-lego-tab-s 0 "2" 1.3 0.92 (drift-ice) :black))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc2_wave" "wave" 4.4 (drift-wave2-options) (drift-ice))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc2_octave" "oct" 2.4 0 false (drift-cream)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc2_route" "route" 3.4 (drift-route-options) (drift-ice))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "voice_pan" "pan" 3.1 2 false (drift-cream))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc2_detune" "det" 3.7 (drift-ice) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "spread" "sprd" 3.7 (drift-cream) 2))
      (eseq.effects.custom-ui-lego/ui-lego-fader-s 0 "osc2_gain_db" 2.3 1.95 (drift-ice) 1 false)))

(def drift-source-block ()
  ;(drift-panel-small 0 (drift-surf-warm) (drift-bord-warm) (drift-pink)
  ;(drift-small-row
  (h-stack :gap 0.82 :align :end
    (box :width 1.3 :height 1.18 :v-align :end
      (eseq.effects.custom-ui-lego/ui-lego-tab-s 0 "N" 1.3 0.92 (drift-pink) :black))
    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "noise_gain_db" "noise" 4.5 0 "dB" (drift-pink))
    (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "noise_route" "nz route" 3.4 (drift-route-options) (drift-ice))
    (drift-osc-toggle "osc1_on" "osc1" (drift-orange))
    (drift-osc-toggle "osc2_on" "osc2" (drift-ice))))

;)
;)

(def drift-cyc-block ()
  (drift-panel-small 0 (drift-surf-cool) (drift-bord-dark) false
    (drift-small-row
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "CYC" 2.4 (drift-cream))
	(box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "env2_mode" "mode" 4.0 (drift-env2-mode-options) (drift-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "cyc_rate_hz" "rate" 4.0 1 "Hz" (drift-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "cyc_tilt" "tilt" 4.0 2 false (drift-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "cyc_hold" "hold" 4.0 2 false (drift-cream))))))

(def drift-env-detail ()
  (eseq.effects.custom-ui-lego/ui-detail-adsr-switch-s
    0 "ENV1 AMP" "env1_attack" "env1_decay" "env1_sustain" "env1_release"
    1 "ENV2 MOD" "env2_attack" "env2_decay" "env2_sustain" "env2_release"))

(def drift-global-block ()
  (drift-panel-small 0 (drift-surf-cool) (drift-bord-dark) false
    (drift-small-row
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "GLB" 2.4 (drift-cream))
	(box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 (drift-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "glide_ms" "glide" 4.0 0 "ms" (drift-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "volume_db" "vol" 5.0 1 "dB" (drift-orange))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "vel_to_vol" "vel" 4.0 2 false (drift-cream))))))

(def drift-detail-column ()
  (v-stack :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
    (drift-cyc-block)
    (drift-env-detail)
    (drift-global-block)))

(def drift-filter-block ()
  (drift-panel-dense 1 (drift-surf-cool) (drift-bord-cool) (drift-ice)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "FILTER" 4.2 (drift-ice))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 1 "filter_type" "type" 4.4 (drift-ftype-options) (drift-orange)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "keytrack" "keytrack" 3.6 2 false (drift-ice))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "hp_freq" "hp" 3.4 0 "Hz" (drift-cream))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-s 1 "lp_freq" "cut" 3.7 (drift-ice) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "lp_res" "res" 3.7 (drift-ice) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "filter_drive" "drive" 3.7 (drift-orange) 2)))))

(def drift-pitch-mod-block ()
  (drift-panel-dense 1 (drift-surf-cool) (drift-bord-warm) (drift-violet)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "PITCH" 3.6 (drift-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 1 "pitch_mod1_src" "src1" 4.4 (drift-src-options) (drift-orange)))
        (h-stack :gap 0.22 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 1 "pitch_mod2_src" "src2" 4.4 (drift-src-options) (drift-orange))
          (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "DRIFT" 3.6 (drift-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "pitch_mod1_amt" "amt1" 3.7 (drift-orange) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "pitch_mod2_amt" "amt2" 3.7 (drift-orange) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "drift" "drift" 3.7 (drift-violet) 2)))))

(def drift-filter-mod-block ()
  (drift-panel-small 1 (drift-surf-cool) (drift-bord-cool) false
    (drift-small-row
      (h-stack :gap 0.52 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "FMOD" 2.8 (drift-ice))
	(box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 1 "lp_mod1_src" "src1" 4.2 (drift-src-options) (drift-ice))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "lp_mod1_amt" "amt1" 2.7 1 false (drift-ice))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 1 "lp_mod2_src" "src2" 3.4 (drift-src-options) (drift-ice))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "lp_mod2_amt" "amt2" 2.7 1 false (drift-ice))))))

(def drift-lfo-strip ()
  (drift-panel-strip 2 (drift-surf-cool) (drift-bord-cool) (drift-violet)
    (v-stack :width :fill :gap 0.08 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "LFO" 5.6 (drift-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo_wave" "wave" 5.6 (drift-lfo-wave-options) (drift-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo_mode" "mode" 5.6 (drift-lfo-mode-options) (drift-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo_retrig" "retrig" 5.6 (drift-retrig-options) (drift-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_rate_hz" "rate" 5.6 2 "Hz" (drift-violet))
      (h-stack
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_ratio" "ratio" 5.6 2 false (drift-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_amount" "amt" 5.6 2 false (drift-violet)))
      )))

(def drift-matrix-strip ()
  (drift-panel-strip 2 (drift-surf-cool) (drift-bord-warm) (drift-orange)
    (v-stack :width :fill :gap 0.08 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "MOD" 5.6 (drift-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "mm1_src" "src1" 5.6 (drift-src-options) (drift-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "mm1_dest" "dst1" 5.6 (drift-dest-options) (drift-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "mm1_amt" "amt1" 5.6 2 false (drift-orange))
      (h-stack
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "mm2_src" "src2" 5.6 (drift-src-options) (drift-cream))
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "mm2_dest" "dst2" 5.6 (drift-dest-options) (drift-cream)))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "mm2_amt" "amt2" 5.6 2 false (drift-cream)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    ;(eseq.effects.custom-ui-lego/ui-lego-column
    (box :padding 0.1 :corner-radius 11 :background-color  :instrument-group-bg
      (v-stack :gap 0.1 :width :fill
        (box :padding 0.2
          (drift-osc1-block)
          )
        (box :width 26 :height 0.05 :background-color :mixer-strip-selected-bg)
        (box :padding 0.2
          (drift-osc2-block)
          )
        (box :width 26 :height 0.05 :background-color :mixer-strip-selected-bg)
        (box :padding 0.2
          (drift-source-block)
          )
        )
      )
    ; )
    (drift-detail-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (drift-filter-block)
      (drift-pitch-mod-block)
      (drift-filter-mod-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (drift-lfo-strip)
      (drift-matrix-strip))))
