;; Custom Synth tab body for instruments/emulations/monomachine-digipro/dsp.lisp
(defstate monomachine-digipro-selected-section 0)
(def monomachine_digipro-select (section)
  (set! monomachine-digipro-selected-section section))
(def monomachine_digipro-panel-bg (section)
  (if (= monomachine-digipro-selected-section section)
    (rgba 0.12 0.12 0.12 1)
    (rgba 0.09 0.09 0.09 1)))
(def monomachine_digipro-cell-width 4.0)
(def monomachine_digipro-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "monomachine_digipro-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (monomachine_digipro-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def monomachine_digipro-param-cell-step-section (name title decimals step section)
  (monomachine_digipro-param-cell-step-section-width name title decimals step section monomachine_digipro-cell-width))
(def monomachine_digipro-param-cell-section (name title decimals section)
  (monomachine_digipro-param-cell-step-section name title decimals 0 section))
(def monomachine_digipro-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "monomachine_digipro-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width monomachine_digipro-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (monomachine_digipro-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def monomachine_digipro-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "monomachine_digipro-adsr-number-" name)
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
                  (monomachine_digipro-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 4.35 :height 1.75
      (v-stack :width 4.35 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :gray :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :gray :edit-color :gray
          :width 4.2 :height 0.95)))))
(def monomachine_digipro-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def monomachine_digipro-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def monomachine_digipro-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (monomachine_digipro-param-value attack 4)
    :decay (monomachine_digipro-param-value decay 400)
    :sustain (monomachine_digipro-param-value sustain 0.5)
    :release (monomachine_digipro-param-value release 0)
    :width 18.5 :height 4.0
    :background-color (rgba 0.0 0.0 0.0 1)
    :on-change (lambda (env)
      (do
        (monomachine_digipro-select section)
        (monomachine_digipro-set-param attack (get env :attack))
        (monomachine_digipro-set-param decay (get env :decay))
        (monomachine_digipro-set-param sustain (get env :sustain))
        (monomachine_digipro-set-param release (get env :release))))))
(def monomachine_digipro-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.95 :padding 0.25
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-param-number-section attack "atk" 0 "ms" section)
      (monomachine_digipro-param-number-section decay "dec" 0 "ms" section)
      (monomachine_digipro-param-number-section sustain "sus" 2 false section)
      (monomachine_digipro-param-number-section release "rel" 0 "ms" section))))
(def monomachine_digipro-selected-adsr ()
  (box :width :fill :height 6.35
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (monomachine_digipro-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (monomachine_digipro-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0))))
(def monomachine_digipro-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :gray :bg :transparent)))
(def monomachine_digipro-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (monomachine_digipro-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_digipro-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-row-label title)
      c1)))
(def monomachine_digipro-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (monomachine_digipro-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_digipro-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-row-label title)
      c1 c2)))
(def monomachine_digipro-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (monomachine_digipro-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_digipro-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-row-label title)
      c1 c2 c3)))
(def monomachine_digipro-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (monomachine_digipro-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_digipro-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-row-label title)
      c1 c2 c3 c4)))
(def monomachine_digipro-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (monomachine_digipro-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_digipro-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-row-label title)
      c1 c2 c3 c4 c5)))
(def monomachine_digipro-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (monomachine_digipro-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_digipro-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def monomachine_digipro-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (monomachine_digipro-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_digipro-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def monomachine_digipro-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (monomachine_digipro-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_digipro-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_digipro-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (monomachine_digipro-panel-1 "GLOB" 0
        (monomachine_digipro-base-note-cell 0))
      (monomachine_digipro-panel-5 "WAVE" 0
        (monomachine_digipro-param-cell-section "morph" "morph" 2 0)
        (monomachine_digipro-param-cell-section "shape" "shape" 2 0)
        (monomachine_digipro-param-cell-section "formant" "form" 2 0)
        (monomachine_digipro-param-cell-section "detune_cents" "det" 0 0)
        (monomachine_digipro-param-cell-section "unison" "uni" 2 0))
      (monomachine_digipro-panel-4 "DIGI" 0
        (monomachine_digipro-param-cell-section "sync_amt" "sync" 2 0)
        (monomachine_digipro-param-cell-section "alias" "alias" 2 0)
        (monomachine_digipro-param-cell-section "noise_level" "noise" 2 0)
        (monomachine_digipro-param-cell-section "table_jump" "jump" 2 0)))
    (v-stack :width 19.6 :gap 0.10
      (monomachine_digipro-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (monomachine_digipro-panel-4 "TEX" 0
        (monomachine_digipro-param-cell-section "phase_distort" "phase" 2 0)
        (monomachine_digipro-param-cell-section "comb_amt" "comb" 2 0)
        (monomachine_digipro-param-cell-section "comb_time" "time" 2 0)
        (monomachine_digipro-param-cell-section "vowel_amt" "vowel" 2 0))
      (monomachine_digipro-panel-5 "FILT" 0
        (monomachine_digipro-param-cell-section "cutoff" "cut" 0 0)
        (monomachine_digipro-param-cell-section "resonance" "res" 2 0)
        (monomachine_digipro-param-cell-section "filter_env_amt" "env" 0 0)
        (monomachine_digipro-param-cell-section "keytrack" "key" 2 0)
        (monomachine_digipro-param-cell-section "drive" "drive" 2 0))
      (monomachine_digipro-panel-1 "OUT" 0
        (monomachine_digipro-param-cell-section "gain" "gain" 2 0)))))
