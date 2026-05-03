;; Custom Synth tab body for instruments/emulations/prophet-6-inspired/dsp.lisp
(defstate prophet-6-inspired-selected-section 0)
(def p6-select (section)
  (set! prophet-6-inspired-selected-section section))
(def p6-panel-bg (section)
  (if (= prophet-6-inspired-selected-section section)
    (rgba 0.12 0.12 0.12 1)
    (rgba 0.09 0.09 0.09 1)))
(def p6-cell-width 4.0)
(def p6-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "p6-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (p6-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def p6-param-cell-step-section (name title decimals step section)
  (p6-param-cell-step-section-width name title decimals step section p6-cell-width))
(def p6-param-cell-section (name title decimals section)
  (p6-param-cell-step-section name title decimals 0 section))
(def p6-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "p6-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width p6-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (p6-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def p6-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "p6-adsr-number-" name)
          (v-stack :width 4.35 :height 1.75 :gap 0.0 :align :center
            (label title :font-size 10 :color :gray :bg :transparent)
            (number-picker :value (get p :value)
              :min (get p :min) :max (get p :max) :decimals decimals
              :unit unit
              :noui true :font-size 10.5
              :text-align :center
              :text-color :widget_focus_bg :edit-color :yellow
              :width 4.2 :height 0.95
              :on-change (lambda (v)
                (do
                  (p6-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 4.35 :height 1.75
      (v-stack :width 4.35 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :gray :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :gray :edit-color :gray
          :width 4.2 :height 0.95)))))
(def p6-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def p6-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def p6-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (p6-param-value attack 4)
    :decay (p6-param-value decay 400)
    :sustain (p6-param-value sustain 0.5)
    :release (p6-param-value release 0)
    :width 18.5 :height 4.0
    :background-color (rgba 0.0 0.0 0.0 1)
    :on-change (lambda (env)
      (do
        (p6-select section)
        (p6-set-param attack (get env :attack))
        (p6-set-param decay (get env :decay))
        (p6-set-param sustain (get env :sustain))
        (p6-set-param release (get env :release))))))
(def p6-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.95 :padding 0.25
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-param-number-section attack "atk" 0 "ms" section)
      (p6-param-number-section decay "dec" 0 "ms" section)
      (p6-param-number-section sustain "sus" 2 false section)
      (p6-param-number-section release "rel" 0 "ms" section))))
(def p6-selected-adsr ()
  (if (= prophet-6-inspired-selected-section 1)
    (box :width :fill :height 6.35
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (p6-adsr-view "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (p6-adsr-controls "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)))
    (box :width :fill :height 6.35
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (p6-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (p6-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)))))
(def p6-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :gray :bg :transparent)))
(def p6-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (p6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (p6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-row-label title)
      c1)))
(def p6-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (p6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (p6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-row-label title)
      c1 c2)))
(def p6-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (p6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (p6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-row-label title)
      c1 c2 c3)))
(def p6-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (p6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (p6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-row-label title)
      c1 c2 c3 c4)))
(def p6-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (p6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (p6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-row-label title)
      c1 c2 c3 c4 c5)))
(def p6-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (p6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (p6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def p6-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (p6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (p6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def p6-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (p6-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (p6-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (p6-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (p6-panel-1 "GLOB" 0
        (p6-base-note-cell 0))
      (p6-panel-4 "OSC1" 0
        (p6-param-cell-section "osc1_shape" "shape" 2 0)
        (p6-param-cell-step-section "osc1_semitones" "semi" 0 1 0)
        (p6-param-cell-section "pulse_width" "pw" 2 0)
        (p6-param-cell-section "brass" "brass" 2 0))
      (p6-panel-4 "OSC2" 0
        (p6-param-cell-section "osc2_shape" "shape" 2 0)
        (p6-param-cell-step-section "osc2_semitones" "semi" 0 1 0)
        (p6-param-cell-section "osc_detune_cents" "det" 0 0)
        (p6-param-cell-section "pulse_width" "pw" 2 0))
      (p6-panel-5 "MIX" 0
        (p6-param-cell-section "osc_mix" "mix" 2 0)
        (p6-param-cell-section "sub_level" "sub" 2 0)
        (p6-param-cell-section "noise_level" "noise" 2 0)
        (p6-param-cell-section "osc_slop" "slop" 2 0)
        (p6-param-cell-section "shape_drift" "drift" 2 0)))
    (v-stack :width 19.6 :gap 0.10
      (p6-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (p6-panel-4 "FILT" 1
        (p6-param-cell-section "cutoff" "cut" 0 1)
        (p6-param-cell-section "resonance" "res" 2 1)
        (p6-param-cell-section "filter_env_amt" "env" 0 1)
        (p6-param-cell-section "keytrack" "key" 2 1))
      (p6-panel-4 "COL" 1
        (p6-param-cell-section "vel_to_filter" "vel" 2 1)
        (p6-param-cell-section "filter_drive" "drive" 2 1)
        (p6-param-cell-section "filter_tone" "tone" 2 1)
        (p6-param-cell-section "cutoff_skew" "skew" 2 1))
      (p6-panel-5 "MOD" 1
        (p6-param-cell-section "lfo_rate_hz" "rate" 2 1)
        (p6-param-cell-section "lfo_to_pw" "pw" 2 1)
        (p6-param-cell-section "lfo_to_cutoff" "cut" 0 1)
        (p6-param-cell-section "env_to_pitch" "env p" 2 1)
        (p6-param-cell-section "vibrato" "vib" 2 1))
      (p6-panel-2 "OUT" 0
        (p6-param-cell-section "stereo_spread" "spread" 2 0)
        (p6-param-cell-section "gain" "gain" 2 0)))))
