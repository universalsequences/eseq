; Paired sources and filter/amp paths surround a contextual instrument display.
; Controls retain the host's scoped parameter, modulation and p-lock semantics.
; Existing editable envelope controls own the contour interaction.
; Use the theme's paired active-surface colors for both the display and labels.
(def heat-accent () :control-on-bg)
(def heat-ink () :control-on-fg)
(def heat-bound (name fallback)
  (eseq.effects.custom-ui-controls/ui-param-bound-value name fallback))
(def heat-knob (section name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 4.7 2.15 2.58 (heat-accent) 2 "linear" :widget-knob-track 10.5 9.5 :right))
(def heat-short-knob (section name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 4.7 2.1 2.8 (heat-accent) 2 "linear" :widget-knob-track 10.5 9.5 :right))
(def heat-log (section name title)
  (heat-log-sized section name title 4.7))
(def heat-log-sized (section name title width)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title width 2.15 2.58 (heat-accent) 1 "log" :widget-knob-track 10.5 9.5 :right))
(def heat-num (section name title)
  (heat-num-labeled section name title false))
(def heat-num-labeled (section name title labels)
  (heat-readout section name title labels 5.1 1.05 0.43 2 0.01 (heat-ink)))
(def heat-compact (section name title)
  (heat-readout section name title false 4.6 1.02 0.46 2 0.01 :dim))
(def heat-readout (section name title labels width height label-height decimals step ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white ink)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "heat-num-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "heat-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width width :height height :gap 0.06
          (label title :v-align :center :height label-height :font-size 8.6 :color ink :bg :transparent)
          (number-picker :width width :height 0.50 :noui true :decimals decimals :step step :font-size 8.0 :value-labels labels
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color ink :edit-color ink :cursor-color ink
            :plock-style :underline
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :on-change (if (number? section)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))))
(def heat-option (section name title options)
  (heat-option-sized section name title options 5.1 1.05 (heat-ink) (heat-accent)))
(def heat-source-option (section name title options width)
  (heat-option-sized section name title options width 1.05 :dim :instrument-control-bg))
(def heat-option-sized (section name title options width height ink surface)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "heat-option-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "heat-option-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (v-stack :width width :height height :gap 0.04
          (label title :v-align :center :height (- height 0.79) :font-size 8.6 :color ink :bg :transparent)
          (dropdown :width width :height 0.75 :font-size 8.6
            :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :value-index-offset (get p :min) :options options
            :text-color ink :chevron-color ink :badge-color :transparent
            :bg-color surface :border-color :transparent :border-width 0
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (lambda (v)
              (do
                (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                  (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v)))))))))))
(def heat-switch (section name title)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
      (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((on (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)) 0.5)))
      (button title :width 4.3 :height 0.75 :font-size 8 :padding 0 :corner-radius 1
        :color (if on (heat-ink) :dim)
        :background-color (if on (heat-accent) :instrument-control-bg)
        :border-color :transparent
        :on-click (lambda (x y r)
          (do
            (if (number? section)
              (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section) false)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if on 0 1))))))))
(def heat-panel (section width height body)
  (box :width width :height height :padding 0.12
    :background-color (if (= eseq.vanilla/custom-ui-selected-section section) :instrument-panel-bg :instrument-group-bg) :corner-radius 3
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    body))

(def heat-osc-row (section prefix title)
  (heat-panel section 21 3.5
    (v-stack :gap 0.1
      (h-stack :gap 0.4 :align :start
        (heat-switch section (str prefix "_enabled") title)
        (heat-source-option section (str prefix "_wave") "Shape" '("Sine" "Saw" "Pulse" "Noise") 6.2)
        (heat-compact section (str prefix "_to_filter1") "F2 / F1"))
      (h-stack :gap 1.5
        (heat-short-knob section (str prefix "_level_db") "Level dB")
        (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section (str prefix "_semitones") "Semi"
          4.7 2.1 2.8 (heat-accent) 0 "linear" :widget-knob-track 10.5 9.5 :right)
        (heat-short-knob section (str prefix "_cents") "Detune")))))

; The response editor combines the same one/two SVF stages as heat-linear-filter.
; LP24/HP24 split Q across the stages; BP12/Notch4 use the full Q in each.
; This is the static linear response, before drive and modulation.
(def heat-filter-curve (section prefix)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (cut (eseq.effects.custom-ui-runtime/custom-ui-current-param (str prefix "_cutoff_hz")))
        (q (eseq.effects.custom-ui-runtime/custom-ui-current-param (str prefix "_q")))
        (mode-binding (heat-bound (str prefix "_mode") 0))
        (follow-binding (heat-bound "filter2_follow" 0))
        (first-cutoff (heat-bound "filter1_cutoff_hz" 1800))
        (offset (eseq.effects.custom-ui-runtime/custom-ui-current-param "filter2_offset_octaves")))
    (subtree :key (str "heat-filter-curve-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" prefix)
      (let ((mode (round (reactive-value mode-binding)))
            (follow (and (= section 6) (> (reactive-value follow-binding) 0.5))))
        (let ((frequency (if follow
                (clamp (* (reactive-value first-cutoff)
                  (pow 2 (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding offset)))) 30 22000)
                (eseq.effects.custom-ui-runtime/custom-ui-param-binding cut))))
          (response-curve-editor :width 20 :height 2.2 :mode :filter
            :debug-name (str "heat-" prefix "-response")
            :bands (map (lambda (id)
              (dict :id id :type (nth '("lowpass" "bandpass" "notch" "highpass") (floor (/ mode 2)))
                :freq frequency
                :freq-min 30 :freq-max 22000
                :q (eseq.effects.custom-ui-runtime/custom-ui-param-binding q) :q-min 0.1 :q-max 100 :q-taper :log
                :q-curve-power (if (or (= mode 1) (= mode 7)) 0.5 1)
                :enabled true :selected (= id 0)))
              (range 0 (if (= (mod mode 2) 1) 2 1)))
            :freq-min 30 :freq-max 22000 :q-min 0.1 :q-max 100
            :background-color :instrument-control-bg :grid-color :border-inactive
            :stroke-color (heat-accent) :point-color (heat-accent) :stroke-width 3.0 :corner-radius 2
            :on-action (lambda (event)
              (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
                (let ((updates (list
                        (dict :param-idx (get (if follow offset cut) :idx)
                          :value (if follow
                            (clamp (/ (log (/ (get event :freq) (reactive-value first-cutoff))) (log 2)) -8 8)
                            (get event :freq)))
                        (dict :param-idx (get q :idx) :value (get event :q))))
                      (commit (= (get event :type) :commit-band)))
                  (do
                    (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                    (if (eseq.effects.param-controls/instrument-rack-target? cut)
                      (host-command
                        (if (seq-has-selection?) "set-rack-slot-instrument-plock-batch" "set-rack-slot-instrument-param-batch")
                        (dict :track (get cut :rack-track) :slot (get cut :rack-slot)
                          :updates updates :gesture (str prefix "-response") :commit commit))
                      (host-command
                        (if (seq-has-selection?) "set-instrument-plock-batch" "set-instrument-param-batch")
                        (dict :updates updates :gesture (str prefix "-response") :commit commit)))))
                false))))))))
(def heat-filter-row (section prefix title)
  (heat-panel section 20.4 4.85
    (v-stack :gap 0.1
      (h-stack :gap 0.2 :align :start
        (v-stack :gap 0
          (heat-switch section (str prefix "_enabled") title)
          (if (= section 2)
            (heat-compact section "filter1_to_filter2" "To F2")
            (heat-source-option section "filter2_follow" "Follow" '("Off" "F1") 4.6)))
        (heat-source-option section (str prefix "_mode") "Type"
          '("LP12" "LP24" "BP6" "BP12" "N2" "N4" "HP12" "HP24") 4.5)
        (heat-log-sized section (str prefix "_cutoff_hz") "Freq Hz" 5.6)
        (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section (str prefix "_q") "Reso Q" 4.7 2.15 2.58 (heat-accent) 2 "log" :widget-knob-track 10.5 9.5 :right))
      (heat-filter-curve section prefix))))
(def heat-amp-row (section prefix title)
  (heat-panel section 10.8 4.85
    (v-stack :gap 0.25 :align :center
      (heat-switch section (str prefix "_enabled") title)
      (h-stack :gap 0.3
        (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section (str prefix "_pan") "Pan" 5.1 2.9 3.5 (heat-accent) 2 "linear" :widget-knob-track 10.5 9.5 :center)
        (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section (str prefix "_level_db") "Level dB" 5.1 2.9 3.5 (heat-accent) 2 "linear" :widget-knob-track 10.5 9.5 :center)))))
(def heat-lfo-row (prefix title)
  (v-stack :height 2.95 :gap 0.12 :align :center
    (heat-switch 4 (str prefix "_enabled") title)
    (heat-log 4 (str prefix "_rate_hz") "Rate Hz")))

(def heat-env-plot (section prefix)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :width 32.6 :height 2.0 :debug-name "heat-envelope"
      :curve-color (heat-ink) :point-color (heat-ink) :grid-color (heat-ink) :background-color (heat-accent)
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
  (v-stack :gap 0.15
    (h-stack :gap 0.3
      (heat-num section (str prefix "_velocity") "Att<Vel")
      (heat-num section (str prefix "_attack_ms") "Attack ms")
      (heat-num section (str prefix "_decay_ms") "Decay ms")
      (heat-num section (str prefix "_sustain") "Sustain")
      (heat-num-labeled section (str prefix "_sustain_seconds") "S.Time s" '((-1 "inf")))
      (heat-num section (str prefix "_release_ms") "Release ms"))
    (h-stack :gap 0.3
      (heat-option section (str prefix "_exponential") "Slope" '("LIN" "EXP"))
      (heat-option section (str prefix "_legato") "Legato" '("Retrig" "Hold"))
      (heat-option section (str prefix "_free") "Free" '("Off" "On"))
      (heat-option section (str prefix "_loop") "Loop" '("ADSR" "AD-R" "ADR-R" "ADS-AR")))))
(def heat-filter-detail (section prefix)
  (v-stack :gap 0.25
    (heat-env-plot section (str prefix "_env"))
    (heat-env-controls section (str prefix "_env"))
    (h-stack :gap 0.3
      (heat-option section (str prefix "_drive") "Drive" '("Off" "Sym1" "Sym2" "Sym3" "Asym1" "Asym2" "Asym3"))
      (heat-num section (str prefix "_lfo_octaves") "Freq<LFO")
      (heat-num section (str prefix "_keytrack") "Freq<Key")
      (heat-num section (str prefix "_env_octaves") "Freq<Env"))
    (h-stack :gap 0.3
      (heat-num section (str prefix "_lfo_q") "Res<LFO")
      (heat-num section (str prefix "_env_q") "Res<Env")
      (if (= section 6) (heat-num section "filter2_offset_octaves" "F2 Offset") false))))
(def heat-amp-detail (section prefix)
  (v-stack :gap 0.25
    (heat-env-plot section (str prefix "_env"))
    (heat-env-controls section (str prefix "_env"))
    (h-stack :gap 0.3
      (heat-num section (str prefix "_lfo_level") "Level<LFO")
      (heat-num section (str prefix "_key_level_db") "Level<Key")
      (heat-num section (str prefix "_lfo_pan") "Pan<LFO")
      (heat-num section (str prefix "_key_pan") "Pan<Key")
      (heat-num section (str prefix "_env_pan") "Pan<Env"))))
(def heat-group (title body)
  (v-stack :gap 0.08
    (label title :v-align :center :height 0.5 :font-size 8.5 :color (heat-ink) :bg :transparent)
    body))
(def heat-pitch-editor (section prefix)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (initial (eseq.effects.custom-ui-runtime/custom-ui-current-param (str prefix "_pitch_env_initial")))
        (time (eseq.effects.custom-ui-runtime/custom-ui-current-param (str prefix "_pitch_env_time_ms"))))
    (adsr-editor :mode :decay :width 32.6 :height 2.0 :debug-name "heat-pitch-envelope"
      :curve-color (heat-ink) :point-color (heat-ink) :grid-color (heat-ink) :background-color (heat-accent)
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
  (v-stack :gap 0.2
    (heat-pitch-editor section prefix)
    (h-stack :gap 0.5
      (heat-num section (str prefix "_pitch_env_initial") "Initial st")
      (heat-num section (str prefix "_pitch_env_time_ms") "Time ms")
      (heat-num section (str prefix "_lfo_pitch_semitones") "Pitch<LFO")
      (heat-num section (str prefix "_keytrack") "Pitch<Key"))
    (heat-group "Pulse Width"
      (h-stack :gap 0.5
        (heat-num section (str prefix "_pulse_duty") "Width")
        (heat-num section (str prefix "_lfo_pw") "Width<LFO")))
    (heat-group "Sub / Sync"
      (h-stack :gap 0.5
        (heat-option section (str prefix "_sub_sync") "Mode" '("Sub" "Sync"))
        (if (> (reactive-value (heat-bound (str prefix "_sub_sync") 0)) 0.5)
          (heat-num section (str prefix "_sync_semitones") "Ratio st")
          (heat-num section (str prefix "_sub_level") "Level"))))))
(def heat-lfo-curve (prefix)
  (let ((shape (heat-bound (str prefix "_shape") 0))
        (width (heat-bound (str prefix "_width") 0.5))
        (phase (heat-bound (str prefix "_phase") 0)))
    (subtree :key (str "heat-curve-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" prefix)
      (lfo-curve :width 32.6 :height 2 :debug-name (str "heat-" prefix "-curve")
        :shape (nth '(1 6 7 4 5) (round (reactive-value shape)))
        :pw width :phase-offset (* 360 (reactive-value phase))
        :cycles 2 :curve-color (heat-ink) :fill-color :transparent
        :background-color (heat-accent)))))
(def heat-lfo-detail-row (prefix)
  (v-stack :gap 0.12
    (heat-lfo-curve prefix)
    (h-stack :gap 0.3
      (heat-option 4 (str prefix "_shape") "Wave" '("Sine" "Triangle" "Pulse" "Random" "Ramp"))
      (heat-num 4 (str prefix "_width") "Width")
      (heat-option 4 (str prefix "_retrigger") "Retrig" '("Off" "On"))
      (heat-num 4 (str prefix "_phase") "Offset")
      (heat-num 4 (str prefix "_delay_ms") "Delay ms")
      (heat-num 4 (str prefix "_fade_ms") "Attack ms"))))
(def heat-lfo-detail ()
  (v-stack :gap 0.6
    (heat-lfo-detail-row "lfo1")
    (heat-lfo-detail-row "lfo2")))
(def heat-noise-strip ()
  (box :width 21 :height 2.56 :padding 0.12 :corner-radius 3
    :background-color :instrument-group-bg :debug-name "heat-noise-strip"
    (v-stack :gap 0.3
      (heat-switch false "noise_enabled" "Noise")
      (h-stack :gap 1.5
        (heat-compact false "noise_level_db" "Level dB")
        (heat-compact false "noise_color_hz" "Color Hz")
        (heat-compact false "noise_to_filter1" "F2 / F1")))))
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
  :width 7.8 :height 1.15
  :state (mode selected) :bindable (selected)
  :shader
  (let ((sx (* width -0.68)) (ax (* width 0.68))
        (ink (if (> selected 0.5) :control-on-bg :control-on-fg))
        (muted (mix :control-on-bg :control-on-fg 0.25)))
    (sdf/layer
      (sdf/fill (sdf/rect width height)
        (if (> selected 0.5) :control-on-fg :control-on-bg))
      (if (> selected 0.5)
        (sdf/stroke (sdf/rect (- width 0.06) (- height 0.06)) 0.06 :control-on-fg)
        (rgba 0 0 0 0))
      (sdf/stroke (sdf/line (+ sx (* width 0.1)) -0.48 (* width -0.1) -0.48) 0.035 ink)
      (sdf/stroke (sdf/line (+ sx (* width 0.1)) 0.48 (* width -0.1) (if (> mode 1.5) -0.48 0.48)) 0.035 ink)
      (if (= mode 1)
        (sdf/stroke (min
          (sdf/line (+ sx (* width 0.1)) -0.48 (* width -0.1) 0.48)
          (sdf/line (+ sx (* width 0.1)) 0.48 (* width -0.1) -0.48)) 0.035 ink)
        (rgba 0 0 0 0))
      (if (= mode 3)
        (sdf/stroke (sdf/line 0 -0.21 0 0.21) 0.035 ink)
        (sdf/stroke (sdf/line (* width 0.1) -0.48 (- ax (* width 0.1)) -0.48) 0.035 ink))
      (if (= mode 2) (rgba 0 0 0 0)
        (sdf/stroke (sdf/line (* width 0.1) 0.48 (- ax (* width 0.1)) 0.48) 0.035 ink))
      (heat-route-node sx -0.48 ink)
      (heat-route-node sx 0.48 ink)
      (heat-route-node 0 -0.48 ink)
      (heat-route-node 0 0.48 (if (= mode 2) muted ink))
      (heat-route-node ax -0.48 (if (= mode 3) muted ink))
      (heat-route-node ax 0.48 (if (= mode 2) muted ink)))))
(def heat-route-button (mode)
  (let ((bindings (map (lambda (pair)
            (dict :binding (heat-bound (get pair :name) 0) :value (get pair :value)))
          (heat-routing-config mode)))
        (callback (heat-routing-callback mode)))
    (subtree :key (str "heat-routing-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" mode)
      (heat-routing-diagram :mode mode :selected (if (heat-routing-selected bindings) 1 0)
        :debug-name (str "heat-route-" mode) :on-click callback))))
(def heat-quick-routing ()
  (h-stack :gap 0.4 :height 1.15
    (heat-route-button 0) (heat-route-button 1)
    (heat-route-button 2) (heat-route-button 3)))

; Single-line readouts leave room for every global control in the display.
(def heat-screen-value (name title decimals step labels)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white (heat-ink))))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "heat-screen-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "heat-screen-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (h-stack :width 10.5 :height 0.5 :gap 0.1 :align :center
          (label title :width 6.2 :height 0.5 :v-align :center :font-size 8.6 :color ink :bg :transparent)
          (number-picker :width 4.1 :height 0.5 :noui true :font-size 8 :text-align :right
            :decimals decimals :step step :value-labels labels
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-color ink :edit-color ink :cursor-color ink
            :plock-style :underline
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s 0 p)))))))
(def heat-screen-num (name title)
  (heat-screen-value name title 2 0.01 false))
(def heat-screen-integer (name title)
  (heat-screen-value name title 0 1 false))
(def heat-screen-choice (name title options)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "heat-choice-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "heat-choice-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (h-stack :width 10.5 :height 0.65 :gap 0.1 :align :center
          (label title :width 5.0 :height 0.5 :v-align :center :font-size 8.6 :color (heat-ink) :bg :transparent)
          (dropdown :width 5.4 :height 0.65 :font-size 8.6
            :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :value-index-offset (get p :min) :options options
            :text-color (heat-ink) :chevron-color (heat-ink) :badge-color :transparent
            :bg-color (heat-accent) :border-color :transparent :border-width 0
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (lambda (v)
              (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v))))))))))
(def heat-global-detail ()
  (v-stack :gap 0.2
    (heat-quick-routing)
    (h-stack :gap 0.45 :align :start
      (heat-group "Vibrato"
        (v-stack :gap 0.04
          (heat-screen-num "vibrato_rate_hz" "Rate Hz")
          (heat-screen-num "vibrato_amount_cents" "Amount ct")
          (heat-screen-num "vibrato_delay_ms" "Delay ms")
          (heat-screen-num "vibrato_attack_ms" "Attack ms")
          (heat-screen-num "vibrato_wheel_cents" "Amt<MW")))
      (heat-group "Keyboard"
        (v-stack :gap 0.04
          (heat-screen-integer "octave" "Octave")
          (heat-screen-integer "tune_semitones" "Semi")
          (heat-screen-num "detune_cents" "Detune ct")))
      (heat-group "Unison"
        (v-stack :gap 0.04
          (heat-screen-choice "unison_voices" "Voices" '("Off" "2" "3" "4"))
          (heat-screen-num "unison_detune_cents" "Detune ct")
          (heat-screen-num "unison_delay_ms" "Delay ms")
          (heat-screen-num "unison_spread" "Spread"))))
    (h-stack :gap 0.45 :align :start
      (heat-group "Glide"
        (v-stack :gap 0.04
          (heat-screen-choice "glide_mode" "Apply" '("Off" "Always" "Legato"))
          (heat-screen-choice "glide_rate_mode" "Mode" '("Const" "Prop"))
          (heat-screen-num "glide_time_ms" "Time ms")))
      (heat-group "Pressure"
        (v-stack :gap 0.04
          (heat-screen-num "pressure_pitch_semitones" "Pitch st")
          (heat-screen-num "pressure_filter_octaves" "Filter oct")
          (heat-screen-num "pressure_amp_db" "Level dB")))
      (heat-group "Tuning"
        (v-stack :gap 0.04
          (heat-screen-num "stretch_cents" "Stretch")
          (heat-screen-num "tuning_error_cents" "Error ct")
          (heat-screen-num "bend_range_semitones" "PB Range"))))))
(def heat-detail ()
  (let ((section eseq.vanilla/custom-ui-selected-section))
    (box :width 34 :height 9.8 :padding 0.35 :corner-radius 2
      :background-color (heat-accent) :debug-name "heat-detail-display"
      (v-stack :gap 0.35
        (h-stack :width 33.3 :height 0.7 :gap 0.2
          (button (str "HEAT / " (nth '("OVERVIEW" "OSC 1" "FILTER 1" "AMP 1" "LFO" "OSC 2" "FILTER 2" "AMP 2") section))
            :width 27.5 :height 0.7 :font-size 8 :padding 0 :corner-radius 0
            :color (heat-accent) :background-color (heat-ink) :border-color :transparent
            :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback 0))
          (button "Overview" :width 5.5 :height 0.7 :font-size 8.6 :padding 0 :corner-radius 0
            :color (heat-ink) :background-color (heat-accent) :border-color (heat-ink)
            :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback 0)))
        (if (= section 1) (heat-osc-detail 1 "osc1")
          (if (= section 2) (heat-filter-detail 2 "filter1")
            (if (= section 3) (heat-amp-detail 3 "amp1")
              (if (= section 4) (heat-lfo-detail)
                (if (= section 5) (heat-osc-detail 5 "osc2")
                  (if (= section 6) (heat-filter-detail 6 "filter2")
                    (if (= section 7) (heat-amp-detail 7 "amp2")
                      (heat-global-detail))))))))))))

(defsynth-ui
  (h-stack :gap 0.15 :align :start
    (v-stack :gap 0.12
      (heat-osc-row 1 "osc1" "Osc1")
      (heat-osc-row 5 "osc2" "Osc2")
      (heat-noise-strip))
    (heat-detail)
    (v-stack :gap 0.1
      (h-stack :gap 0.12
        (heat-filter-row 2 "filter1" "Fil1") (heat-amp-row 3 "amp1" "Amp1"))
      (h-stack :gap 0.12
        (heat-filter-row 6 "filter2" "Fil2") (heat-amp-row 7 "amp2" "Amp2")))
    (heat-panel 4 6.2 9.8
      (v-stack :gap 0.15 :align :center
        (heat-lfo-row "lfo1" "LFO1")
        (heat-lfo-row "lfo2" "LFO2")
        (heat-knob 0 "volume_db" "Volume")))))
