;; Digi Drift: paired sources surround a themed envelope/cycle display.
;; UI-only layout; all parameters keep the host's scoped modulation/p-lock routes.
(def drift-accent () :control-on-bg)
(def drift-ink () :control-on-fg)
(def drift-section () (if (= eseq.vanilla/custom-ui-selected-section 1) 1 0))
(def drift-knob (name title width height size decimals taper)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s (drift-section) name title
    width height size (drift-accent) decimals taper :widget-knob-track 9.0 8.0 :right))
(def drift-panel (width height body)
  (box :width width :height height :padding 0.12 :corner-radius 2
    :background-color :instrument-group-bg body))
(def drift-label (title width)
  (box :width width :height 0.75 :background-color (drift-accent)
    (label title :width width :height 0.75 :h-align :center :font-size 8 :color (drift-ink) :bg :transparent :v-align :center)))
(def drift-switch (name title width)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((on (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)) 0.5)))
      (button title :debug-name (str "drift-switch-" name) :width width :height 0.75 :font-size 8 :padding 0 :corner-radius 1
        :color (if on (drift-ink) :dim)
        :background-color (if on (drift-accent) :instrument-control-bg)
        :on-click (lambda (x y r)
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if on 0 1)))))))
(def drift-num (name title width decimals ink)
  (drift-readout (drift-section) name title false width 1.1 0.5 decimals
    (if (= decimals 0) 1 0.01) ink))
(def drift-option (name options width ink surface)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "drift-option-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "drift-option-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (dropdown :width width :height 0.8 :font-size 8
          :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
          :value-index-offset (get p :min) :options options
          :text-color ink :chevron-color ink :badge-color :transparent
          :bg-color surface :border-color :transparent
          :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
          :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
          :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
          :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
          :on-change (lambda (v)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
              (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v)))))))))
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

(def drift-ftype-options ()
  '("I 12dB" "II 24dB"))

(def drift-lfo-mode-options ()
  '("hz" "ratio"))

(def drift-env2-mode-options ()
  '("adsr" "cyc"))

(def drift-retrig-options ()
  '("free" "trig"))


(def drift-readout (section name title labels width height label-height decimals step ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "drift-num-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "drift-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width width :height height :gap 0.06
          (label title :v-align :center :height label-height :font-size 7.6 :color ink :bg :transparent)
          (number-picker :width width :height 0.50 :noui true :decimals decimals :step step :font-size 8.0 :value-labels labels
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) ink)
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (if (number? section)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))))
;; Single-line readouts for the filter header/footer: no compressed label rows.
(def drift-inline-num (name title width decimals)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "drift-inline-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "drift-inline-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (h-stack :width width :height 0.8 :gap 0.25 :align :center
          (label title :width 2.8 :height 0.8 :font-size 7.6 :color :dim :bg :transparent :v-align :center)
          (number-picker :width (- width 3.05) :height 0.75 :noui true :decimals decimals :font-size 8
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-color (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p)
            :text-align :left
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s (drift-section) p)))))))
(def drift-preview-binding (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-binding
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
(def drift-preview-mod (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-mod-offset
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))

(def drift-source-preview ()
  (drift-waveform :width 14.0 :height 2.3
    :background-color :instrument-control-bg :wave-color (drift-accent)
    :osc1-wave (drift-preview-binding "osc1_wave")
    :osc1-shape (drift-preview-binding "osc1_shape")
    :osc1-shape-mod (drift-preview-mod "osc1_shape")
    :osc1-octave (drift-preview-binding "osc1_octave")
    :osc1-on (drift-preview-binding "osc1_on")
    :osc1-gain-db (drift-preview-binding "osc1_gain_db")
    :osc1-gain-db-mod (drift-preview-mod "osc1_gain_db")
    :osc2-wave (drift-preview-binding "osc2_wave")
    :osc2-octave (drift-preview-binding "osc2_octave")
    :osc2-detune (drift-preview-binding "osc2_detune")
    :osc2-detune-mod (drift-preview-mod "osc2_detune")
    :osc2-on (drift-preview-binding "osc2_on")
    :osc2-gain-db (drift-preview-binding "osc2_gain_db")
    :osc2-gain-db-mod (drift-preview-mod "osc2_gain_db")
    :noise-gain-db (drift-preview-binding "noise_gain_db")
    :noise-gain-db-mod (drift-preview-mod "noise_gain_db")))

(def drift-filter-curve ()
  (let ((cut-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "lp_freq"))
      (res-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "lp_res"))
      (hp-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "hp_freq"))
      (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (if (and cut-p res-p hp-p)
          (response-curve-editor
            :mode :filter
            :bands (list
              (dict :id 0 :type "lowpass"
                :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding cut-p)
                :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min cut-p)
                :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max cut-p)
                :gain 0 :gain-min -12 :gain-max 12
                :q (eseq.effects.custom-ui-runtime/custom-ui-param-binding res-p)
                :q-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min res-p)
                :q-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max res-p)
                ;; Ableton Drift-style resonance plot: gentle through the middle
                ;; (res 0.5 ~ +2 dB), sharp only near the top (res 1 ~ Q 7).
                :q-curve-offset 0.5 :q-curve-scale 6.7 :q-curve-power 3.0
                :enabled true :selected true)
              (dict :id 1 :type "highpass"
                :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding hp-p)
                :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min hp-p)
                :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max hp-p)
                :gain 0 :gain-min -12 :gain-max 12
                ;; the DSP highpass is a fixed-Q svf; draw it Butterworth-flat.
                :q 0.707 :q-min 0 :q-max 1
                :lock-y true
                :enabled true :selected false))
            :freq-min 10
            :freq-max 18000
            :gain-min -12
            :gain-max 12
            :q-min 0
            :q-max 1
            :background-color :instrument-control-bg
            :corner-radius 5
            :grid-color :border-inactive
            :stroke-color (drift-accent)
            :stroke-width 3
            :point-color (drift-accent)
            :width 29.4
            :height 2.0 :debug-name "drift-filter-response"
            :on-action (lambda (event)
              (if (or (= (get event :type) :change-band)
                  (= (get event :type) :commit-band))
                (if (= (get event :id) 1)
                    (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope hp-p (get event :freq))
                    (do
                      (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope cut-p (get event :freq))
                      (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope res-p (get event :q))))
                nil)))
          (label "missing filter params" :font-size 8 :color :red :bg :transparent))))

(def drift-osc-row (prefix title options second second-title decimals)
  (drift-panel 25 3.5
    (v-stack :gap 0.15
      (h-stack :gap 0.4
        (drift-switch (str prefix "_on") title 4.5)
        (drift-option (str prefix "_wave") options 7 :fg :instrument-control-bg)
        (drift-switch (str prefix "_route") "To Filter" 6.5))
      (h-stack :gap 1.4
        (drift-knob (str prefix "_octave") "Octave" 6.2 2.3 3.1 0 "linear")
        (drift-knob second second-title 6.2 2.3 3.1 decimals "linear")
        (drift-knob (str prefix "_gain_db") "Gain dB" 6.2 2.3 3.1 1 "linear")))))
(def drift-sources ()
  (v-stack :gap 0.1
    (drift-osc-row "osc1" "Osc1" (drift-wave1-options) "osc1_shape" "Shape" 2)
    (drift-osc-row "osc2" "Osc2" (drift-wave2-options) "osc2_detune" "Detune" 1)
    (drift-panel 25 2.6
      (h-stack :gap 0.35 :align :center
        (drift-source-preview)
        (drift-knob "noise_gain_db" "Noise dB" 5.2 2.3 2.8 0 "linear")
        (drift-switch "noise_route" "To Filt" 4.1)))))
(def drift-env-plot (section prefix)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :width 33.2 :height 2.4 :debug-name "drift-envelope"
      :background-color :instrument-control-bg :curve-color (drift-accent)
      :point-color (drift-accent) :grid-color :border-inactive
      :attack (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_attack") 4)
      :decay (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_decay") 350)
      :sustain (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_sustain") 0.75)
      :release (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_release") 250)
      :on-change (lambda (env)
        (do
          (eseq.effects.custom-ui-sections/custom-ui-set-active-adsr scope section (get env :active))
          (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
            (str prefix "_attack") (str prefix "_decay")
            (str prefix "_sustain") (str prefix "_release") env))))))
(def drift-screen-tab (section title)
  (button title :width 5.2 :height 0.75 :font-size 8 :padding 0
    :color (if (= (drift-section) section) (drift-ink) (drift-accent))
    :background-color (if (= (drift-section) section) (drift-accent) :instrument-control-bg)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))
(def drift-matrix-row (prefix)
  (h-stack :gap 0.5 :align :center
    (drift-option (str prefix "_src") (drift-src-options) 7.0 :fg :instrument-control-bg)
    (label "→" :width 1.5 :height 0.8 :color (drift-ink) :bg :transparent)
    (drift-option (str prefix "_dest") (drift-dest-options) 13 :fg :instrument-control-bg)
    (drift-num (str prefix "_amt") "Amount" 9.5 2 (drift-ink))))
(def drift-display ()
  (let ((section (drift-section))
        (prefix (if (= (drift-section) 1) "env2" "env1")))
    (box :width 34 :height 9.8 :padding 0.35 :corner-radius 2
      :debug-name "drift-detail-display" :background-color (drift-accent)
      (v-stack :gap 0.12
        (h-stack :gap 0.15
          (box :width 22.5 :height 0.75 :background-color :instrument-control-bg
            (label "DIGI DRIFT / ENVELOPE" :height 0.75 :font-size 8 :color (drift-accent) :bg :transparent :v-align :center))
          (drift-screen-tab 0 "ENV 1") (drift-screen-tab 1 "ENV 2"))
        (drift-env-plot section prefix)
        (h-stack :gap 0.8
          (drift-num (str prefix "_attack") "Attack ms" 7.6 0 (drift-ink))
          (drift-num (str prefix "_decay") "Decay ms" 7.6 0 (drift-ink))
          (drift-num (str prefix "_sustain") "Sustain" 7.6 2 (drift-ink))
          (drift-num (str prefix "_release") "Release ms" 7.6 0 (drift-ink)))
        (box :width 33.2 :height 0.03 :background-color (drift-ink))
        (h-stack :gap 0.8 :align :end
          (h-stack :width 9.8 :height 1.1 :gap 0.3 :align :center
            (label "Cycle" :width 3.3 :height 0.8 :font-size 7.6 :color (drift-ink) :bg :transparent :v-align :center)
            (drift-option "env2_mode" (drift-env2-mode-options) 6.2 :fg :instrument-control-bg))
          (drift-num "cyc_rate_hz" "Rate Hz" 7.0 2 (drift-ink))
          (drift-num "cyc_tilt" "Tilt" 7.0 2 (drift-ink))
          (drift-num "cyc_hold" "Hold" 7.0 2 (drift-ink)))
        (box :width 33.2 :height 0.03 :background-color (drift-ink))
        (drift-matrix-row "mm1")
        (drift-matrix-row "mm2")))))
(def drift-filter-panel ()
  (drift-panel 29.8 5.4
    (v-stack :gap 0.1
      (h-stack :gap 0.5 :align :center
        (drift-label "FILTER" 5.4)
        (drift-option "filter_type" (drift-ftype-options) 8 :fg :instrument-control-bg)
        (drift-inline-num "keytrack" "Key" 6 2))
      (h-stack :gap 1.8
        (drift-knob "lp_freq" "Cutoff Hz" 8.3 2.15 3.0 0 "log")
        (drift-knob "lp_res" "Resonance" 8.3 2.15 3.0 2 "linear")
        (drift-knob "hp_freq" "HP Hz" 8.3 2.15 3.0 0 "log"))
      (drift-filter-curve))))
(def drift-lfo-preview ()
  (let ((wave (drift-preview-binding "lfo_wave")))
    (subtree :key (str "drift-lfo-preview-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
      (lfo-curve :width 8.2 :height 2.0 :debug-name "drift-lfo-preview"
        :shape (nth '(1 0 3 8 2 4 5) (round (reactive-value wave)))
        :cycles 1.5 :curve-color (drift-accent) :fill-color :transparent
        :background-color :instrument-control-bg))))
(def drift-lfo-panel ()
  (drift-panel 29.8 3.15
    (v-stack :gap 0.08
      (h-stack :gap 0.6
        (drift-label "LFO" 4.6)
        (drift-option "lfo_wave" (drift-lfo-wave-options) 7 :fg :instrument-control-bg)
        (drift-option "lfo_mode" (drift-lfo-mode-options) 7 :fg :instrument-control-bg)
        (drift-option "lfo_retrig" (drift-retrig-options) 7 :fg :instrument-control-bg))
      (h-stack :gap 0.6
        (drift-lfo-preview)
        (drift-knob "lfo_rate_hz" "Rate Hz" 6.3 1.7 2.2 2 "log")
        (drift-knob "lfo_ratio" "Ratio" 6.3 1.7 2.2 2 "linear")
        (drift-knob "lfo_amount" "Amount" 6.3 1.7 2.2 2 "linear")))))
(def drift-filter-mod ()
  (drift-panel 29.8 1.05
    (h-stack :gap 0.4 :align :center
      (drift-option "lp_mod1_src" (drift-src-options) 6.8 :fg :instrument-control-bg)
      (drift-inline-num "lp_mod1_amt" "Amt1" 6.8 1)
      (drift-option "lp_mod2_src" (drift-src-options) 6.8 :fg :instrument-control-bg)
      (drift-inline-num "lp_mod2_amt" "Amt2" 6.8 1))))
(def drift-pitch-panel ()
  (drift-panel 12.4 9.8
    (v-stack :gap 0.18 :align :center
      (drift-label "PITCH" 5.2)
      (h-stack :gap 0.3
        (drift-option "pitch_mod1_src" (drift-src-options) 5.8 :fg :instrument-control-bg)
        (drift-option "pitch_mod2_src" (drift-src-options) 5.8 :fg :instrument-control-bg))
      (h-stack :gap 0.3
        (drift-knob "pitch_mod1_amt" "Amount 1" 5.8 2.0 2.8 1 "linear")
        (drift-knob "pitch_mod2_amt" "Amount 2" 5.8 2.0 2.8 1 "linear"))
      (h-stack :gap 0.3
        (drift-knob "drift" "Drift" 5.8 2.2 3.0 2 "linear")
        (drift-knob "volume_db" "Vol dB" 5.8 2.2 3.0 1 "linear"))
      (h-stack :gap 0.3
        (drift-num "glide_ms" "Glide ms" 5.8 0 :dim)
        (drift-num "vel_to_vol" "Velocity" 5.8 2 :dim))
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s (drift-section) 5.8 :fg))))
(defsynth-ui
  (h-stack :gap 0.15 :align :start
    (drift-sources)
    (drift-display)
    (v-stack :gap 0.1
      (drift-filter-panel)
      (drift-lfo-panel)
      (drift-filter-mod))
    (drift-pitch-panel)))
