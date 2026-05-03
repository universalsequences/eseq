;; Custom Synth tab body for instruments/emulations/monomachine-sid/dsp.lisp
(defstate monomachine-sid-selected-section 0)
(def monomachine_sid-select (section)
  (set! monomachine-sid-selected-section section))
(def monomachine_sid-panel-bg (section)
  (if (= monomachine-sid-selected-section section)
    (rgba 0.12 0.12 0.12 1)
    (rgba 0.09 0.09 0.09 1)))
(def monomachine_sid-cell-width 4.0)
(def monomachine_sid-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "monomachine_sid-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (monomachine_sid-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def monomachine_sid-param-cell-step-section (name title decimals step section)
  (monomachine_sid-param-cell-step-section-width name title decimals step section monomachine_sid-cell-width))
(def monomachine_sid-param-cell-section (name title decimals section)
  (monomachine_sid-param-cell-step-section name title decimals 0 section))
(def monomachine_sid-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "monomachine_sid-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width monomachine_sid-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (monomachine_sid-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def monomachine_sid-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "monomachine_sid-adsr-number-" name)
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
                  (monomachine_sid-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 4.35 :height 1.75
      (v-stack :width 4.35 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :gray :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :gray :edit-color :gray
          :width 4.2 :height 0.95)))))
(def monomachine_sid-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def monomachine_sid-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def monomachine_sid-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (monomachine_sid-param-value attack 4)
    :decay (monomachine_sid-param-value decay 400)
    :sustain (monomachine_sid-param-value sustain 0.5)
    :release (monomachine_sid-param-value release 0)
    :width 18.5 :height 4.0
    :background-color (rgba 0.0 0.0 0.0 1)
    :on-change (lambda (env)
      (do
        (monomachine_sid-select section)
        (monomachine_sid-set-param attack (get env :attack))
        (monomachine_sid-set-param decay (get env :decay))
        (monomachine_sid-set-param sustain (get env :sustain))
        (monomachine_sid-set-param release (get env :release))))))
(def monomachine_sid-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.95 :padding 0.25
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-param-number-section attack "atk" 0 "ms" section)
      (monomachine_sid-param-number-section decay "dec" 0 "ms" section)
      (monomachine_sid-param-number-section sustain "sus" 2 false section)
      (monomachine_sid-param-number-section release "rel" 0 "ms" section))))
(def monomachine_sid-selected-adsr ()
  (box :width :fill :height 6.35
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (monomachine_sid-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (monomachine_sid-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0))))
(def monomachine_sid-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :gray :bg :transparent)))
(def monomachine_sid-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (monomachine_sid-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_sid-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-row-label title)
      c1)))
(def monomachine_sid-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (monomachine_sid-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_sid-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-row-label title)
      c1 c2)))
(def monomachine_sid-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (monomachine_sid-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_sid-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-row-label title)
      c1 c2 c3)))
(def monomachine_sid-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (monomachine_sid-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_sid-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-row-label title)
      c1 c2 c3 c4)))
(def monomachine_sid-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (monomachine_sid-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_sid-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-row-label title)
      c1 c2 c3 c4 c5)))
(def monomachine_sid-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (monomachine_sid-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_sid-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def monomachine_sid-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (monomachine_sid-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_sid-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def monomachine_sid-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (monomachine_sid-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_sid-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_sid-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (monomachine_sid-panel-1 "GLOB" 0
        (monomachine_sid-base-note-cell 0))
      (monomachine_sid-panel-5 "TUNE" 0
        (monomachine_sid-param-cell-step-section "osc1_semi" "o1 st" 0 1 0)
        (monomachine_sid-param-cell-step-section "osc2_semi" "o2 st" 0 1 0)
        (monomachine_sid-param-cell-step-section "osc3_semi" "o3 st" 0 1 0)
        (monomachine_sid-param-cell-section "osc2_detune" "o2 det" 0 0)
        (monomachine_sid-param-cell-section "osc3_detune" "o3 det" 0 0))
      (monomachine_sid-panel-3 "LVL" 0
        (monomachine_sid-param-cell-section "osc1_level" "o1" 2 0)
        (monomachine_sid-param-cell-section "osc2_level" "o2" 2 0)
        (monomachine_sid-param-cell-section "osc3_level" "o3" 2 0)))
    (v-stack :width 19.6 :gap 0.10
      (monomachine_sid-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (monomachine_sid-panel-4 "WAV" 0
        (monomachine_sid-param-cell-section "tri_mix" "tri" 2 0)
        (monomachine_sid-param-cell-section "saw_mix" "saw" 2 0)
        (monomachine_sid-param-cell-section "pulse_mix" "pulse" 2 0)
        (monomachine_sid-param-cell-section "noise_mix" "noise" 2 0))
      (monomachine_sid-panel-3 "P/S" 0
        (monomachine_sid-param-cell-section "pulse_width" "pw" 2 0)
        (monomachine_sid-param-cell-section "sync_amt" "sync" 2 0)
        (monomachine_sid-param-cell-section "ring_amt" "ring" 2 0))
      (monomachine_sid-panel-5 "FILT" 0
        (monomachine_sid-param-cell-section "cutoff" "cut" 0 0)
        (monomachine_sid-param-cell-section "resonance" "res" 2 0)
        (monomachine_sid-param-cell-section "filter_mode" "mode" 2 0)
        (monomachine_sid-param-cell-section "keytrack" "key" 2 0)
        (monomachine_sid-param-cell-section "filter_fm" "fm" 0 0))
      (monomachine_sid-panel-7 "GRIT" 0
        (monomachine_sid-param-cell-step-section "bit_depth" "bits" 0 1 0)
        (monomachine_sid-param-cell-section "drive" "drive" 2 0)
        (monomachine_sid-param-cell-section "glitch_rate" "rate" 0 0)
        (monomachine_sid-param-cell-section "glitch_amt" "amt" 2 0)
        (monomachine_sid-param-cell-section "fold_amt" "fold" 2 0)
        (monomachine_sid-param-cell-section "buzz" "buzz" 2 0)
        (monomachine_sid-param-cell-section "gain" "gain" 2 0)))))
