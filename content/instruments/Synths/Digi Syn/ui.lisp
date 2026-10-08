;; Digi Syn: paired sources surround a themed envelope/cycle display.
;; UI-only layout; all parameters keep the host's scoped modulation/p-lock routes.
(def syn-accent () :control-on-bg)
(def syn-ink () :control-on-fg)
;; Display sections: 0 ENV 1, 1 ENV 2, 2 VOICES.
(def syn-section ()
  (let ((section eseq.vanilla/custom-ui-selected-section))
    (if (or (= section 1) (= section 2)) section 0)))
(def syn-knob (name title width height size decimals taper)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s (syn-section) name title
    width height size (syn-accent) decimals taper :widget-knob-track 10.5 9.5 :right))
(def syn-panel (width height body)
  (box :width width :height height :padding 0.12 :corner-radius 2
    :background-color :instrument-group-bg body))
(def syn-label (title width)
  (box :width width :height 0.75 :background-color (syn-accent)
    (label title :width width :height 0.75 :h-align :center :font-size 8 :color (syn-ink) :bg :transparent :v-align :center)))
(def syn-switch (name title width)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((on (> (eseq.effects.custom-ui-runtime/custom-ui-param-value p) 0.5)))
      (button title :debug-name (str "syn-switch-" name) :width width :height 0.75 :font-size 8 :padding 0 :corner-radius 1
        :color (if on (syn-ink) :dim)
        :background-color (if on (syn-accent) :instrument-control-bg)
        :on-click (lambda (x y r)
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if on 0 1)))))))
(def syn-num (name title width decimals ink)
  (syn-readout (syn-section) name title false width 1.1 0.5 decimals
    (if (= decimals 0) 1 0.01) ink))
(def syn-option (name options width ink surface)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "syn-option-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "syn-option-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (dropdown :width width :height 0.8 :font-size 8
          :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
          :value-index-offset (get p :min) :options options
          :text-color ink :chevron-color ink :badge-color :transparent
          :bg-color surface :border-color :transparent
          :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
          :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
          :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
          :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
          :on-change (lambda (v)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
              (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v)))))))))
(def syn-wave1-options ()
  '("sine" "tri" "shark" "sat" "saw" "pulse" "rect"))

(def syn-wave2-options ()
  '("sine" "tri" "sat" "saw" "rect"))

(def syn-src-options ()
  '("env1" "env2" "lfo" "key" "vel" "wheel" "press" "slide"))

(def syn-lfo-wave-options ()
  '("sine" "tri" "sawU" "sawD" "sqr" "s&h" "wndr" "linear env" "exp env"))

(def syn-dest-options ()
  '("none" "o1 gain" "o1 shp" "o2 gain" "o2 det" "nz gain" "lp frq" "lp res" "hp frq" "lfo rate" "cyc rate" "vol"))

(def syn-ftype-options ()
  '("I 12dB" "II 24dB"))

(def syn-lfo-mode-options ()
  '("hz" "time" "ratio" "beat"))

(def syn-env2-mode-options ()
  '("adsr" "cyc"))

(def syn-retrig-options ()
  '("free" "trig"))


(def syn-readout (section name title labels width height label-height decimals step ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white ink)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "syn-num-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "syn-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
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
            :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
            :on-change (if (number? section)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))))
;; Single-line readouts for the filter header/footer: no compressed label rows.
(def syn-inline-num (name title width decimals)
  (syn-inline-num-ink name title width decimals false))
;; ink false keeps the dark-panel colors; the orange display passes (syn-ink).
(def syn-inline-num-ink (name title width decimals ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "syn-inline-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "syn-inline-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (h-stack :width width :height 0.8 :gap 0.25 :align :center
          (label title :width (min 5.8 (* width 0.5)) :height 0.8 :font-size 8.6 :color (if ink ink :dim) :bg :transparent :v-align :center)
          (number-picker :width (- width (+ 0.25 (min 5.8 (* width 0.5)))) :height 0.75 :noui true :decimals decimals :font-size 8
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-color (if ink ink (eseq.effects.custom-ui-runtime/custom-ui-param-knob-text-color p))
            :text-align :left
            ;; Display ink marks locks with an underline; the accent would vanish on orange.
            :plock-style (if ink :underline :fill)
            :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s (syn-section) p)))))))
(def syn-preview-binding (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-binding
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
(def syn-preview-mod (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-mod-offset
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))

(def syn-source-preview ()
  (drift-waveform :width 14.0 :height 2.3 :release 2
    :background-color :instrument-control-bg :wave-color (syn-accent)
    :osc1-wave (syn-preview-binding "osc1_wave")
    :osc1-shape (syn-preview-binding "osc1_shape")
    :osc1-shape-mod (syn-preview-mod "osc1_shape")
    :osc1-octave (syn-preview-binding "osc1_octave")
    :osc1-on (syn-preview-binding "osc1_on")
    :osc1-gain-db (syn-preview-binding "osc1_gain_db")
    :osc1-gain-db-mod (syn-preview-mod "osc1_gain_db")
    :osc2-wave (syn-preview-binding "osc2_wave")
    :osc2-octave (syn-preview-binding "osc2_octave")
    :osc2-detune (syn-preview-binding "osc2_detune")
    :osc2-detune-mod (syn-preview-mod "osc2_detune")
    :osc2-on (syn-preview-binding "osc2_on")
    :osc2-gain-db (syn-preview-binding "osc2_gain_db")
    :osc2-gain-db-mod (syn-preview-mod "osc2_gain_db")
    :noise-on (syn-preview-binding "noise_on")
    :noise-gain-db (syn-preview-binding "noise_gain_db")
    :noise-gain-db-mod (syn-preview-mod "noise_gain_db")))

(def syn-filter-curve ()
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
            :freq-max 20000
            :gain-min -12
            :gain-max 12
            :q-min 0
            :q-max 1
            :background-color :instrument-control-bg
            :corner-radius 5
            :grid-color :border-inactive
            :stroke-color (syn-accent)
            :stroke-width 3
            :point-color (syn-accent)
            :width 29.4
            :height 2.0 :debug-name "syn-filter-response"
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

(def syn-osc-row (prefix title options second second-title decimals)
  (syn-panel 25 3.5
    (v-stack :gap 0.15
      (h-stack :gap 0.4
        (syn-switch (str prefix "_on") title 4.5)
        (syn-option (str prefix "_wave") options 7 :fg :instrument-control-bg)
        (syn-switch (str prefix "_route") "To Filter" 6.5))
      (h-stack :gap 1.4
        (syn-knob (str prefix "_octave") "Octave" 6.2 2.3 3.1 0 "linear")
        (syn-knob second second-title 6.2 2.3 3.1 decimals "linear")
        (syn-knob (str prefix "_gain_db") "Gain dB" 6.2 2.3 3.1 1 "linear")))))
;; Osc1 carries its shape-mod source over the matching amount knob.
(def syn-osc1-row ()
  (syn-panel 25 3.5
    (v-stack :gap 0.15
      (h-stack :gap 0.35
        (syn-switch "osc1_on" "Osc1" 4.5)
        (syn-option "osc1_wave" (syn-wave1-options) 7 :fg :instrument-control-bg)
        (syn-option "osc1_shape_src" (syn-src-options) 5.5 :fg :instrument-control-bg)
        (syn-switch "osc1_route" "To Filter" 6.3))
      (h-stack :gap 0.35
        (syn-knob "osc1_octave" "Octave" 5.6 2.3 3.1 0 "linear")
        (syn-knob "osc1_shape" "Shape" 5.6 2.3 3.1 2 "linear")
        (syn-knob "osc1_shape_amt" "Shape mod" 5.6 2.3 3.1 2 "linear")
        (syn-knob "osc1_gain_db" "Gain dB" 5.6 2.3 3.1 1 "linear")))))
(def syn-sources ()
  (v-stack :gap 0.1
    (syn-osc1-row)
    (syn-osc-row "osc2" "Osc2" (syn-wave2-options) "osc2_detune" "Detune" 1)
    (syn-panel 25 2.6
      (h-stack :gap 0.35 :align :center
        (syn-source-preview)
        (syn-knob "noise_gain_db" "Noise dB" 5.2 2.3 2.8 0 "linear")
        (v-stack :gap 0.3
          (syn-switch "noise_on" "Noise" 4.1)
          (syn-switch "noise_route" "To Filt" 4.1))))))
(def syn-env-plot (section prefix)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :width 33.2 :height 2.4 :debug-name "syn-envelope"
      :background-color :instrument-control-bg :curve-color (syn-accent)
      :point-color (syn-accent) :grid-color :border-inactive
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
(def syn-screen-tab (section title)
  (button title :width 5.2 :height 0.75 :debug-name (str "syn-tab-" section) :font-size 8 :padding 0
    :color (if (= (syn-section) section) (syn-ink) (syn-accent))
    :background-color (if (= (syn-section) section) (syn-accent) :instrument-control-bg)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))
(def syn-matrix-row (prefix)
  (h-stack :gap 0.5 :align :center
    (syn-option (str prefix "_src") (syn-src-options) 7.0 :fg :instrument-control-bg)
    (label "→" :width 1.5 :height 0.8 :color (syn-ink) :bg :transparent)
    (syn-option (str prefix "_dest") (syn-dest-options) 13 :fg :instrument-control-bg)
    (syn-inline-num-ink (str prefix "_amt") "Amt" 9.5 2 (syn-ink))))
(def syn-env-detail (section prefix)
  (v-stack :gap 0.12
    (syn-env-plot section prefix)
    (h-stack :gap 0.8
      (syn-num (str prefix "_attack") "Attack ms" 7.6 0 (syn-ink))
      (syn-num (str prefix "_decay") "Decay ms" 7.6 0 (syn-ink))
      (syn-num (str prefix "_sustain") "Sustain" 7.6 2 (syn-ink))
      (syn-num (str prefix "_release") "Release ms" 7.6 0 (syn-ink)))
    (box :width 33.2 :height 0.03 :background-color (syn-ink))
    (h-stack :gap 0.2 :align :end
      (h-stack :width 9.8 :height 1.1 :gap 0.3 :align :center
        (label "Cycle" :width 3.3 :height 0.8 :font-size 8.6 :color (syn-ink) :bg :transparent :v-align :center)
        (syn-option "env2_mode" (syn-env2-mode-options) 6.2 :fg :instrument-control-bg))
      (syn-option "cyc_mode" (syn-lfo-mode-options) 5.0 :fg :instrument-control-bg)
      (syn-num (syn-rate-param "cyc") "Rate" 6.0 2 (syn-ink))
      (syn-num "cyc_tilt" "Tilt" 5.0 2 (syn-ink))
      (syn-num "cyc_hold" "Hold" 5.0 2 (syn-ink)))
    (box :width 33.2 :height 0.03 :background-color (syn-ink))
    (syn-matrix-row "mm1")
    (syn-matrix-row "mm2")
    (syn-matrix-row "mm3")))

;; VOICES screen: single-line LED readouts, Heat-style.
(def syn-screen-value (name title decimals step)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white (syn-ink))))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "syn-screen-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "syn-screen-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (h-stack :width 10.6 :height 0.8 :gap 0.1 :align :center
          (label title :width 6.0 :height 0.8 :v-align :center :font-size 8.6 :color ink :bg :transparent)
          (number-picker :width 4.4 :height 0.8 :noui true :font-size 8.6 :text-align :right
            :decimals decimals :step step
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-color ink :edit-color ink :cursor-color ink
            :plock-style :underline
            :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s 2 p)))))))
(def syn-screen-choice (name title options)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "syn-choice-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "syn-choice-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (h-stack :width 10.6 :height 0.8 :gap 0.1 :align :center
          (label title :width 6.0 :height 0.8 :v-align :center :font-size 8.6 :color (syn-ink) :bg :transparent)
          (dropdown :width 4.4 :height 0.8 :font-size 8.6
            :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :value-index-offset (get p :min) :options options
            :text-color (syn-ink) :chevron-color (syn-ink) :badge-color :transparent
            :bg-color (syn-accent) :border-color :transparent :border-width 0
            :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (lambda (v)
              (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v))))))))))
(def syn-screen-group (title body)
  (v-stack :gap 0.1
    (label title :v-align :center :height 0.55 :font-size 8.6 :color (syn-ink) :bg :transparent)
    (box :width 10.6 :height 0.03 :background-color (syn-ink))
    body))
;; Mode is the voice allocator's main switch: one wide segment per mode.
(def syn-mode-button (mode title)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "voice_mode"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((on (= (round (eseq.effects.custom-ui-runtime/custom-ui-param-value p)) mode)))
      (button title :debug-name (str "syn-voice-mode-" mode) :width 8.1 :height 1.1 :font-size 9 :padding 0 :corner-radius 0
        :color (if on (syn-accent) (syn-ink))
        :background-color (if on (syn-ink) (syn-accent))
        :border-color (syn-ink)
        :on-click (lambda (x y r)
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p mode))))))
(def syn-voices-detail ()
  (v-stack :gap 0.45
    (h-stack :gap 0.27
      (syn-mode-button 0 "POLY") (syn-mode-button 1 "MONO")
      (syn-mode-button 2 "STEREO") (syn-mode-button 3 "UNISON"))
    (h-stack :gap 0.75 :align :start
      (syn-screen-group "Voice"
        (v-stack :gap 0.18
          (syn-screen-choice "osc_retrig" "Retrigger" '("Off" "On"))
          (syn-screen-choice "note_pitch_bend_on" "Pitch bend" '("Off" "On"))))
      (syn-screen-group "Width"
        (v-stack :gap 0.18
          (syn-screen-value "mono_thickness" "Thickness" 2 0.01)
          (syn-screen-value "stereo_spread" "Stereo" 2 0.01)
          (syn-screen-value "unison_strength" "Unison" 2 0.01)
          (syn-screen-value "spread" "Drift pan" 2 0.01)))
      (syn-screen-group "Tune / Pan"
        (v-stack :gap 0.18
          (syn-screen-value "transpose" "Transpose" 0 1)
          (syn-screen-value "pitch_bend_range" "Bend range" 0 1)
          (syn-screen-value "voice_pan" "Pan" 2 0.01))))))

(def syn-display ()
  (let ((section (syn-section))
        (prefix (if (= (syn-section) 1) "env2" "env1")))
    (box :width 34 :height 9.8 :padding 0.35 :corner-radius 2
      :debug-name "syn-detail-display" :background-color (syn-accent)
      (v-stack :gap 0.12
        (h-stack :gap 0.15
          (box :width 17.15 :height 0.75 :background-color :instrument-control-bg
            (label (if (= section 2) "DIGI SYN / VOICES" "DIGI SYN / ENVELOPE")
              :height 0.75 :font-size 8 :color (syn-accent) :bg :transparent :v-align :center))
          (syn-screen-tab 0 "ENV 1") (syn-screen-tab 1 "ENV 2") (syn-screen-tab 2 "VOICES"))
        (if (= section 2)
          (syn-voices-detail)
          (syn-env-detail section prefix))))))
(def syn-filter-panel ()
  (syn-panel 29.8 5.4
    (v-stack :gap 0.1
      (h-stack :gap 0.5 :align :center
        (syn-label "FILTER" 5.4)
        (syn-option "filter_type" (syn-ftype-options) 8 :fg :instrument-control-bg)
        (syn-inline-num "keytrack" "Key" 6 2))
      (h-stack :gap 1.8
        (syn-knob "lp_freq" "Cutoff Hz" 8.3 2.15 3.0 0 "log")
        (syn-knob "lp_res" "Resonance" 8.3 2.15 3.0 2 "linear")
        (syn-knob "hp_freq" "HP Hz" 8.3 2.15 3.0 0 "log"))
      (syn-filter-curve))))
(def syn-lfo-panel ()
  (syn-panel 29.8 3.0
    (v-stack :gap 0.1
      (h-stack :gap 0.6
        (syn-label "LFO" 4.6)
        (syn-option "lfo_wave" (syn-lfo-wave-options) 7 :fg :instrument-control-bg)
        (syn-option "lfo_mode" (syn-lfo-mode-options) 7 :fg :instrument-control-bg)
        (syn-option "lfo_retrig" (syn-retrig-options) 7 :fg :instrument-control-bg))
      (h-stack :gap 0.6 :align :center
        (syn-option "lfo_mod_src" (syn-src-options) 6.3 :fg :instrument-control-bg)
        (syn-knob (syn-rate-param "lfo") "Rate" 6.3 1.7 2.2 2 "log")
        (syn-knob "lfo_amount" "Amount" 6.3 1.7 2.2 2 "linear")
        (syn-knob "lfo_mod_amt" "Mod amt" 6.3 1.7 2.2 2 "linear")))))
(def syn-filter-mod ()
  (syn-panel 29.8 1.05
    (h-stack :gap 0.4 :align :center
      (syn-option "lp_mod1_src" (syn-src-options) 6.8 :fg :instrument-control-bg)
      (syn-inline-num "lp_mod1_amt" "Amt1" 6.8 1)
      (syn-option "lp_mod2_src" (syn-src-options) 6.8 :fg :instrument-control-bg)
      (syn-inline-num "lp_mod2_amt" "Amt2" 6.8 1))))
(def syn-pitch-panel ()
  (syn-panel 12.4 9.8
    (v-stack :gap 0.18 :align :center
      (syn-label "PITCH" 5.2)
      (h-stack :gap 0.3
        (syn-option "pitch_mod1_src" (syn-src-options) 5.8 :fg :instrument-control-bg)
        (syn-option "pitch_mod2_src" (syn-src-options) 5.8 :fg :instrument-control-bg))
      (h-stack :gap 0.3
        (syn-knob "pitch_mod1_amt" "Amount 1" 5.8 2.0 2.8 1 "linear")
        (syn-knob "pitch_mod2_amt" "Amount 2" 5.8 2.0 2.8 1 "linear"))
      (h-stack :gap 0.3
        (syn-knob "drift" "Drift" 5.8 2.2 3.0 2 "linear")
        (syn-knob "volume_db" "Vol dB" 5.8 2.2 3.0 1 "linear"))
      (h-stack :gap 0.3
        (syn-num "glide_ms" "Glide ms" 5.8 0 :dim)
        (syn-num "vel_to_vol" "Velocity" 5.8 2 :dim))
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s (syn-section) 5.8 :fg))))
(def syn-rate-param (prefix)
  (let ((mode (round (eseq.effects.custom-ui-controls/ui-param-value (str prefix "_mode") 0))))
    (str prefix (nth '("_rate_hz" "_time_ms" "_ratio" "_beats") mode))))

(defsynth-ui
  (h-stack :gap 0.15 :align :start
    (syn-sources)
    (syn-display)
    (v-stack :gap 0.1
      (syn-filter-panel)
      (syn-lfo-panel)
      (syn-filter-mod))
    (syn-pitch-panel)))
