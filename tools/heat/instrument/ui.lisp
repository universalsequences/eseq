; Heat's two signal paths frame a shared detail display, as on Analog.
; Controls retain the host's scoped parameter, modulation and p-lock semantics.
; Existing editable envelope controls own the contour interaction.
(def heat-accent () :yellow)
(def heat-bound (name fallback)
  (eseq.effects.custom-ui-controls/ui-param-bound-value name fallback))
(def heat-knob (section name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 4.7 2.15 2.58 (heat-accent) 2 "linear" :widget-knob-track 9.0 8.0 :right))
(def heat-log (section name title)
  (heat-log-sized section name title 4.7))
(def heat-log-sized (section name title width)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title width 2.15 2.58 (heat-accent) 1 "log" :widget-knob-track 9.0 8.0 :right))
(def heat-num (section name title)
  (heat-num-labeled section name title false))
(def heat-num-labeled (section name title labels)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "heat-num-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "heat-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width 5.7 :height 1.24 :gap 0.06
          (label title :v-align :center :height 0.68 :font-size 7.6 :color :dim :bg :transparent)
          (number-picker :width 5.7 :height 0.50 :noui true :decimals 2 :font-size 8.0 :value-labels labels
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p)
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (if (number? section)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))))
(def heat-option (section name title options)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "heat-option-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "heat-option-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (v-stack :width 6.2 :height 1.24 :gap 0.04
          (label title :v-align :center :height 0.6 :font-size 7.6 :color :dim :bg :transparent)
          (dropdown :width 6.2 :height 0.6 :font-size 7.6
            :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :value-index-offset (get p :min) :options options
            :text-color :dim :chevron-color :dim :badge-color :transparent
            :bg-color :instrument-group-bg :border-color :instrument-control-bg :border-width 0.04
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (lambda (v)
              (do
                (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                  (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v)))))))))))
(def heat-tab (section title)
  (eseq.effects.custom-ui-lego/ui-lego-mode-tab-s section title 4.3 0.75 (heat-accent)))
(def heat-switch (section name title)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((on (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)) 0.5)))
      (button title :width 4.3 :height 0.75 :font-size 8 :padding 0 :corner-radius 1
        :color (if on :black :dim)
        :background-color (if on (heat-accent) :instrument-control-bg)
        :on-click (lambda (x y r)
          (do
            (if (number? section)
              (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section) false)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if on 0 1))))))))
(def heat-panel (section width height body)
  (box :width width :height height :padding 0.12
    :background-color (if (= eseq.vanilla/custom-ui-selected-section section) :instrument-panel-bg :instrument-group-bg) :corner-radius 2
    :border-width 0.04
    :border-color (if (= eseq.vanilla/custom-ui-selected-section section) :dim :transparent)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    body))

(def heat-osc-row (section prefix title)
  (heat-panel section 31 2.3
    (h-stack :gap 0.25 :align :start
      (v-stack :gap 0
        (heat-switch section (str prefix "_enabled") title)
        (heat-num section (str prefix "_to_filter1") "F2 / F1"))
      (heat-option section (str prefix "_wave") "Shape" '("Sine" "Saw" "Pulse" "Noise"))
      (heat-knob section (str prefix "_level_db") "Level dB")
      (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section (str prefix "_semitones") "Semi"
        4.7 2.15 2.58 (heat-accent) 0 "linear" :widget-knob-track 9.0 8.0 :right)
      (heat-knob section (str prefix "_cents") "Detune"))))
(def heat-filter-row (section prefix title)
  (heat-panel section 25 2.3
    (h-stack :gap 0.25 :align :start
      (v-stack :gap 0
        (heat-switch section (str prefix "_enabled") title)
        (if (= section 2)
          (heat-num section "filter1_to_filter2" "To F2")
          (heat-option section "filter2_follow" "Follow" '("Off" "F1"))))
      (heat-option section (str prefix "_mode") "Type"
        '("LP12" "LP24" "BP6" "BP12" "N2" "N4" "HP12" "HP24"))
      (heat-log-sized section (str prefix "_cutoff_hz") "Freq Hz" 6.5)
      (heat-knob section (str prefix "_q") "Reso Q"))))
(def heat-amp-row (section prefix title)
  (heat-panel section 15 2.3
    (h-stack :gap 0.25 :align :start
      (heat-switch section (str prefix "_enabled") title)
      (heat-knob section (str prefix "_pan") "Pan")
      (heat-knob section (str prefix "_level_db") "Level dB"))))
(def heat-lfo-row (prefix title)
  (v-stack :height 4.45 :gap 0.22 :align :center
    (heat-switch 4 (str prefix "_enabled") title)
    (heat-log 4 (str prefix "_rate_hz") "Rate Hz")))

(def heat-env-plot (section prefix)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :width 22 :height 3.8 :debug-name "heat-envelope"
      :attack (heat-bound (str prefix "_attack_ms") 5)
      :decay (heat-bound (str prefix "_decay_ms") 350)
      :sustain (heat-bound (str prefix "_sustain") 0.5)
      :release (heat-bound (str prefix "_release_ms") 250)
      :attack-max 15000 :decay-max 15000 :release-max 15000
      :on-change (lambda (env)
        (do
          (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
          (eseq.effects.custom-ui-sections/custom-ui-set-active-adsr scope section (get env :active))
          (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
            (str prefix "_attack_ms") (str prefix "_decay_ms")
            (str prefix "_sustain") (str prefix "_release_ms") env))))))
(def heat-env-controls (section prefix)
  (v-stack :gap 0.12 :align :start
    (h-stack :gap 0.5
      (heat-num section (str prefix "_velocity") "Att<Vel")
      (heat-num section (str prefix "_attack_ms") "Attack ms")
      (heat-num section (str prefix "_decay_ms") "Decay ms")
      (heat-num section (str prefix "_sustain") "Sustain")
      (heat-num-labeled section (str prefix "_sustain_seconds") "S.Time s" '((-1 "inf")))
      (heat-num section (str prefix "_release_ms") "Release ms"))
    (h-stack :gap 0.5
      (heat-option section (str prefix "_exponential") "Slope" '("LIN" "EXP"))
      (heat-option section (str prefix "_legato") "Legato" '("Retrig" "Hold"))
      (heat-option section (str prefix "_free") "Free" '("Off" "On"))
      (heat-option section (str prefix "_loop") "Loop" '("ADSR" "AD-R" "ADR-R" "ADS-AR")))))
(def heat-filter-detail (section prefix)
  (h-stack :gap 0.6 :align :start
    (heat-env-plot section (str prefix "_env"))
    (v-stack :gap 0.12
      (heat-env-controls section (str prefix "_env"))
      (h-stack :gap 0.5
        (heat-option section (str prefix "_drive") "Drive" '("Off" "Sym1" "Sym2" "Sym3" "Asym1" "Asym2" "Asym3"))
        (heat-num section (str prefix "_lfo_octaves") "Freq<LFO")
        (heat-num section (str prefix "_keytrack") "Freq<Key")
        (heat-num section (str prefix "_env_octaves") "Freq<Env")
        (heat-num section (str prefix "_lfo_q") "Res<LFO")
        (heat-num section (str prefix "_env_q") "Res<Env")))))
(def heat-amp-detail (section prefix)
  (h-stack :gap 0.6 :align :start
    (heat-env-plot section (str prefix "_env"))
    (v-stack :gap 0.12
      (heat-env-controls section (str prefix "_env"))
      (h-stack :gap 0.5
        (heat-num section (str prefix "_lfo_level") "Level<LFO")
        (heat-num section (str prefix "_key_level_db") "Level<Key")
        (heat-num section (str prefix "_lfo_pan") "Pan<LFO")
        (heat-num section (str prefix "_key_pan") "Pan<Key")
        (heat-num section (str prefix "_env_pan") "Pan<Env")))))
(def heat-group (title body)
  (v-stack :gap 0.12
    (label title :v-align :center :height 0.65 :font-size 8.5 :color :dim :bg :transparent)
    body))
(def heat-pitch-editor (section prefix)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (initial (eseq.effects.custom-ui-runtime/custom-ui-current-param (str prefix "_pitch_env_initial")))
        (time (eseq.effects.custom-ui-runtime/custom-ui-current-param (str prefix "_pitch_env_time_ms"))))
    (adsr-editor :mode :decay :width 22 :height 3.8 :debug-name "heat-pitch-envelope"
      :initial (heat-bound (str prefix "_pitch_env_initial") 0)
      :time (heat-bound (str prefix "_pitch_env_time_ms") 500)
      :initial-min -48 :initial-max 48 :time-max 15000
      :on-change (lambda (env)
        (let ((updates (list
                (dict :param-idx (get initial :idx) :value (get env :initial))
                (dict :param-idx (get time :idx) :value (get env :time))))
              (gesture (str prefix "-pitch-envelope")))
          (do
            (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
            (if (eseq.effects.param-controls/instrument-rack-target? initial)
              (host-command
                (if (seq-has-selection?) "set-rack-slot-instrument-plock-batch" "set-rack-slot-instrument-param-batch")
                (dict :track (get initial :rack-track) :slot (get initial :rack-slot)
                      :updates updates :gesture gesture :commit (not (get env :active))))
              (host-command
                (if (seq-has-selection?) "set-instrument-plock-batch" "set-instrument-param-batch")
                (dict :updates updates :gesture gesture :commit (not (get env :active)))))))))))
(def heat-osc-detail (section prefix)
  (h-stack :gap 0.6 :align :start
    (heat-pitch-editor section prefix)
    (v-stack :gap 0.45
      (h-stack :gap 1
        (heat-num section (str prefix "_pitch_env_initial") "Initial st")
        (heat-num section (str prefix "_pitch_env_time_ms") "Time ms"))
      (h-stack :gap 1 :align :start
        (heat-group "Pitch Mod"
          (h-stack :gap 0.5
            (heat-num section (str prefix "_lfo_pitch_semitones") (if (= section 1) "LFO1" "LFO2"))
            (heat-num section (str prefix "_keytrack") "Key")))
        (heat-group "Pulse Width"
          (h-stack :gap 0.5
            (heat-num section (str prefix "_pulse_duty") "Width")
            (heat-num section (str prefix "_lfo_pw") (if (= section 1) "LFO1" "LFO2"))))
        (heat-group "Sub"
          (heat-num section (str prefix "_sub_level") "Level"))))))
(def heat-lfo-curve (prefix color)
  (let ((shape (heat-bound (str prefix "_shape") 0))
        (width (heat-bound (str prefix "_width") 0.5))
        (phase (heat-bound (str prefix "_phase") 0)))
    (subtree :key (str "heat-curve-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" prefix)
      (lfo-curve :width 20 :height 2 :debug-name (str "heat-" prefix "-curve")
        :shape (nth '(1 6 7 4 5) (round (reactive-value shape)))
        :pw width :phase-offset (* 360 (reactive-value phase))
        :cycles 2 :curve-color color :fill-color :transparent
        :background-color :instrument-control-bg))))
(def heat-lfo-detail-row (prefix color)
  (h-stack :height 2 :gap 0.5 :align :center
    (heat-lfo-curve prefix color)
    (heat-option 4 (str prefix "_shape") "Wave" '("Sine" "Triangle" "Pulse" "Random" "Ramp"))
    (heat-num 4 (str prefix "_width") "Width")
    (heat-option 4 (str prefix "_retrigger") "Retrig" '("Off" "On"))
    (heat-num 4 (str prefix "_phase") "Offset")
    (heat-num 4 (str prefix "_delay_ms") "Delay ms")
    (heat-num 4 (str prefix "_fade_ms") "Attack ms")))
(def heat-lfo-detail ()
  (v-stack :gap 0
    (heat-lfo-detail-row "lfo1" :cyan)
    (heat-lfo-detail-row "lfo2" :dim)))
(def heat-noise-strip ()
  (box :width 8 :height 4.4 :padding 0.12 :corner-radius 2
    :background-color :instrument-group-bg :debug-name "heat-noise-strip"
    (v-stack :gap 0.02 :align :center
      (heat-switch false "noise_enabled" "Noise")
      (heat-num false "noise_level_db" "Level dB")
      (heat-num false "noise_color_hz" "Color Hz"))))
; Quick Routing writes the same eight ordinary parameters as Analog.
(def heat-routing-values (mode)
  (nth '((1 0 1 0 1 1 1 1)
         (0.5 0.5 0.5 0 1 1 1 1)
         (1 1 1 0 1 0 1 0)
         (1 1 1 1 1 1 0 1)) mode))
(def heat-routing-config (mode)
  (let ((names '("osc1_to_filter1" "osc2_to_filter1" "noise_to_filter1"
                 "filter1_to_filter2" "filter1_enabled" "filter2_enabled"
                 "amp1_enabled" "amp2_enabled"))
        (values (heat-routing-values mode)))
    (map (lambda (i) (dict :name (nth names i) :value (nth values i))) (range 0 8))))
(def heat-routing-selected (bindings)
  (= 0 (len
    (filter (lambda (pair)
      (> (abs (- (reactive-value (get pair :binding)) (get pair :value))) 0.0001)) bindings))))
(def heat-routing-callback (mode)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (config (heat-routing-config mode)))
    (lambda (x y r)
      (let ((first (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope "osc1_to_filter1"))
            (updates (map (lambda (pair)
              (dict :param-idx (get (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope (get pair :name)) :idx)
                    :value (get pair :value))) config)))
        (do
          (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 0)
          (if (eseq.effects.param-controls/instrument-rack-target? first)
            (host-command "set-rack-slot-instrument-param-batch"
              (dict :track (get first :rack-track) :slot (get first :rack-slot)
                    :updates updates :commit true :gesture "heat-routing" :label "Heat quick routing"))
            (host-command "set-instrument-param-batch"
              (dict :updates updates :commit true :gesture "heat-routing" :label "Heat quick routing"))))))))
(defmacro heat-route-node (xx yy color)
  `(sdf/stroke (sdf/translate ,xx ,yy (sdf/rect (* width 0.10) 0.27)) 0.035 ,color))
(defwidget heat-routing-diagram
  :width 8.6 :height 1.3
  :state (mode selected) :bindable (selected)
  :shader
  (let ((sx (* width -0.68)) (ax (* width 0.68))
        (orange (rgba 1 0.55 0.02 1)) (green (rgba 0.1 0.9 0.2 1))
        (cyan (rgba 0 0.85 0.73 1)) (muted (rgba 0.35 0.35 0.35 1)))
    (sdf/layer
      (sdf/fill (sdf/rect width height)
        (if (> selected 0.5) (rgba 0.34 0.30 0.12 1)
          (if hit/hover (rgba 0.2 0.2 0.2 1) (rgba 0.12 0.12 0.12 1))))
      (if (> selected 0.5)
        (sdf/stroke (sdf/rect (- width 0.06) (- height 0.06)) 0.06 (rgba 1 0.82 0.05 1))
        (rgba 0 0 0 0))
      (sdf/stroke (sdf/line (+ sx (* width 0.1)) -0.48 (* width -0.1) -0.48) 0.035 orange)
      (sdf/stroke (sdf/line (+ sx (* width 0.1)) 0.48 (* width -0.1) (if (> mode 1.5) -0.48 0.48)) 0.035 orange)
      (if (= mode 1)
        (sdf/stroke (min
          (sdf/line (+ sx (* width 0.1)) -0.48 (* width -0.1) 0.48)
          (sdf/line (+ sx (* width 0.1)) 0.48 (* width -0.1) -0.48)) 0.035 orange)
        (rgba 0 0 0 0))
      (if (= mode 3)
        (sdf/stroke (sdf/line 0 -0.21 0 0.21) 0.035 green)
        (sdf/stroke (sdf/line (* width 0.1) -0.48 (- ax (* width 0.1)) -0.48) 0.035 green))
      (if (= mode 2) (rgba 0 0 0 0)
        (sdf/stroke (sdf/line (* width 0.1) 0.48 (- ax (* width 0.1)) 0.48) 0.035 green))
      (heat-route-node sx -0.48 orange)
      (heat-route-node sx 0.48 orange)
      (heat-route-node 0 -0.48 green)
      (heat-route-node 0 0.48 (if (= mode 2) muted green))
      (heat-route-node ax -0.48 (if (= mode 3) muted cyan))
      (heat-route-node ax 0.48 (if (= mode 2) muted cyan)))))
(def heat-route-button (mode)
  (let ((bindings (map (lambda (pair)
            (dict :binding (heat-bound (get pair :name) 0) :value (get pair :value)))
          (heat-routing-config mode)))
        (callback (heat-routing-callback mode)))
    (subtree :key (str "heat-routing-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" mode)
      (heat-routing-diagram :mode mode :selected (if (heat-routing-selected bindings) 1 0)
        :debug-name (str "heat-route-" mode) :on-click callback))))
(def heat-quick-routing ()
  (v-stack :width 18 :height 3.8 :gap 0.22
    (label "Quick Routing" :v-align :center :height 0.65 :font-size 9 :color :dim :bg :transparent)
    (h-stack :gap 0.35 (heat-route-button 0) (heat-route-button 1))
    (h-stack :gap 0.35 (heat-route-button 2) (heat-route-button 3))))

(def heat-global-detail ()
  (h-stack :gap 2 :align :start
    (heat-quick-routing)
    (v-stack :gap 0.7
      (heat-num 0 "tune_semitones" "Tune st")
      (heat-num 0 "filter2_offset_octaves" "F2 Offset"))
    (heat-num 0 "noise_to_filter1" "Noise F1")
    (v-stack :gap 0.7
      (label "Pressure" :v-align :center :font-size 9 :color :dim :bg :transparent)
      (h-stack :gap 1
        (heat-num 0 "pressure_pitch_semitones" "Pitch st")
        (heat-num 0 "pressure_filter_octaves" "Filter oct")
        (heat-num 0 "pressure_amp_db" "Level dB")))))
(def heat-detail ()
  (let ((section eseq.vanilla/custom-ui-selected-section))
    (box :width 63 :height 4.4 :padding 0.2 :corner-radius 2
      :background-color :instrument-control-bg
      (if (= section 1) (heat-osc-detail 1 "osc1")
        (if (= section 2) (heat-filter-detail 2 "filter1")
          (if (= section 3) (heat-amp-detail 3 "amp1")
            (if (= section 4) (heat-lfo-detail)
              (if (= section 5) (heat-osc-detail 5 "osc2")
                (if (= section 6) (heat-filter-detail 6 "filter2")
                  (if (= section 7) (heat-amp-detail 7 "amp2")
                    (heat-global-detail)))))))))))

(defsynth-ui
  (h-stack :gap 0.15 :align :start
    (v-stack :gap 0.15
      (h-stack :gap 0.1
        (heat-osc-row 1 "osc1" "Osc1")
        (heat-filter-row 2 "filter1" "Fil1")
        (heat-amp-row 3 "amp1" "Amp1"))
      (h-stack :gap 0.2 (heat-noise-strip) (heat-detail))
      (h-stack :gap 0.1
        (heat-osc-row 5 "osc2" "Osc2")
        (heat-filter-row 6 "filter2" "Fil2")
        (heat-amp-row 7 "amp2" "Amp2")))
    (heat-panel 4 6.2 9.2
      (v-stack :gap 0.05
        (heat-lfo-row "lfo1" "LFO1")
        (heat-lfo-row "lfo2" "LFO2")))
    (heat-panel 0 6.5 9.2
      (v-stack :gap 0.8 :align :center :padding 1
        (heat-knob 0 "volume_db" "Volume")))))
