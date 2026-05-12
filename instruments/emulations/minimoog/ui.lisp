;; Custom Synth tab body for instruments/emulations/minimoog/dsp.lisp
(defstate minimoog-selected-section 0)
(def minimoog-select (section)
  (set! minimoog-selected-section section))
(def minimoog-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= minimoog-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def minimoog-cell-width 4.0)
(def minimoog-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "minimoog-cell-" name)
        (knob-number :label title
          :value (fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (minimoog-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def minimoog-param-cell-step-section (name title decimals step section)
  (minimoog-param-cell-step-section-width name title decimals step section minimoog-cell-width))
(def minimoog-param-cell-section (name title decimals section)
  (minimoog-param-cell-step-section name title decimals 0 section))
(def minimoog-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "minimoog-base-note-cell")
        (knob-number :label "note"
          :value (fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width minimoog-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (minimoog-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def minimoog-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "minimoog-adsr-number-" name)
          (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
            (label title :font-size 10 :color :dim :bg :transparent)
            (number-picker :value (fx-param-value p)
              :min (get p :min) :max (get p :max) :decimals decimals
              :unit unit
              :noui true :font-size 10.5
              :text-align :center
              :text-color :widget_focus_bg :edit-color :yellow
              :width 5.0 :height 0.95
              :on-change (lambda (v)
                (do
                  (minimoog-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :dim :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :dim :edit-color :dim
          :width 5.0 :height 0.95)))))
(def minimoog-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-param-value p) fallback))
    fallback))
(def minimoog-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def minimoog-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (minimoog-param-value attack 4)
    :decay (minimoog-param-value decay 400)
    :sustain (minimoog-param-value sustain 0.5)
    :release (minimoog-param-value release 0)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (minimoog-select section)
        (minimoog-set-param attack (get env :attack))
        (minimoog-set-param decay (get env :decay))
        (minimoog-set-param sustain (get env :sustain))
        (minimoog-set-param release (get env :release))))))
(def minimoog-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-param-number-section attack "atk" 0 "ms" section)
      (minimoog-param-number-section decay "dec" 0 "ms" section)
      (minimoog-param-number-section sustain "sus" 2 false section)
      (minimoog-param-number-section release "rel" 0 "ms" section))))

(def minimoog-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def minimoog-selected-adsr ()
  (if (= minimoog-selected-section 1)
    (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (minimoog-adsr-view "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (minimoog-adsr-controls "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms" 1)
    (minimoog-adsr-caption "FILTER ENV")))
    (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (minimoog-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (minimoog-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (minimoog-adsr-caption "AMP ENV")))))
(def minimoog-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def minimoog-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (minimoog-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (minimoog-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-row-label title)
      c1)))
(def minimoog-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (minimoog-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (minimoog-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-row-label title)
      c1 c2)))
(def minimoog-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (minimoog-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (minimoog-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-row-label title)
      c1 c2 c3)))
(def minimoog-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (minimoog-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (minimoog-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-row-label title)
      c1 c2 c3 c4)))
(def minimoog-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (minimoog-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (minimoog-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-row-label title)
      c1 c2 c3 c4 c5)))
(def minimoog-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (minimoog-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (minimoog-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def minimoog-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (minimoog-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (minimoog-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def minimoog-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (minimoog-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (minimoog-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (minimoog-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (minimoog-panel-1 "GLOB" 0
        (minimoog-base-note-cell 0))
      (minimoog-panel-5 "TUNE" 0
        (minimoog-param-cell-step-section "osc1_semi" "o1 st" 0 1 0)
        (minimoog-param-cell-step-section "osc2_semi" "o2 st" 0 1 0)
        (minimoog-param-cell-step-section "osc3_semi" "o3 st" 0 1 0)
        (minimoog-param-cell-section "osc2_detune" "o2 det" 0 0)
        (minimoog-param-cell-section "osc3_detune" "o3 det" 0 0))
      (minimoog-panel-4 "LVL" 0
        (minimoog-param-cell-section "osc1_level" "o1" 2 0)
        (minimoog-param-cell-section "osc2_level" "o2" 2 0)
        (minimoog-param-cell-section "osc3_level" "o3" 2 0)
        (minimoog-param-cell-section "noise_level" "noise" 2 0)))
    (v-stack :width 23.1 :gap 0.10
      (minimoog-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (minimoog-panel-4 "W1/2" 0
        (minimoog-param-cell-section "osc1_saw" "o1 saw" 2 0)
        (minimoog-param-cell-section "osc1_pulse" "o1 pls" 2 0)
        (minimoog-param-cell-section "osc2_saw" "o2 saw" 2 0)
        (minimoog-param-cell-section "osc2_pulse" "o2 pls" 2 0))
      (minimoog-panel-4 "W3" 0
        (minimoog-param-cell-section "osc3_saw" "saw" 2 0)
        (minimoog-param-cell-section "osc3_tri" "tri" 2 0)
        (minimoog-param-cell-section "pulse_width" "pw" 2 0)
        (minimoog-param-cell-section "sync_1_to_3" "sync" 2 0))
      (minimoog-panel-6 "FILT" 1
        (minimoog-param-cell-section "cutoff" "cut" 0 1)
        (minimoog-param-cell-section "resonance" "res" 2 1)
        (minimoog-param-cell-section "filter_env_amt" "env" 0 1)
        (minimoog-param-cell-section "keytrack" "key" 2 1)
        (minimoog-param-cell-section "filter_vel_amt" "vel" 2 1)
        (minimoog-param-cell-section "filter_drive" "drive" 2 1))
      (minimoog-panel-4 "PERF" 0
        (minimoog-param-cell-section "osc3_fm_amt" "fm" 2 0)
        (minimoog-param-cell-section "glide_time" "glide" 0 0)
        (minimoog-param-cell-section "amp_vel_amt" "vel" 2 0)
        (minimoog-param-cell-section "gain" "gain" 2 0)))))
