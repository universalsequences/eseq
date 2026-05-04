;; Custom Synth tab body for instruments/emulations/monomachine-fmplus/dsp.lisp
(defstate monomachine-fmplus-selected-section 0)
(def monomachine_fmplus-select (section)
  (set! monomachine-fmplus-selected-section section))
(def monomachine_fmplus-panel-bg (section)
  (if (= section 0)
    (rgba 0.09 0.09 0.09 1)
    (if (= monomachine-fmplus-selected-section section)
      (rgba 0.12 0.12 0.12 1)
      (rgba 0.09 0.09 0.09 1))))
(def monomachine_fmplus-cell-width 4.0)
(def monomachine_fmplus-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "monomachine_fmplus-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (monomachine_fmplus-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def monomachine_fmplus-param-cell-step-section (name title decimals step section)
  (monomachine_fmplus-param-cell-step-section-width name title decimals step section monomachine_fmplus-cell-width))
(def monomachine_fmplus-param-cell-section (name title decimals section)
  (monomachine_fmplus-param-cell-step-section name title decimals 0 section))
(def monomachine_fmplus-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "monomachine_fmplus-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width monomachine_fmplus-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (monomachine_fmplus-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def monomachine_fmplus-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "monomachine_fmplus-adsr-number-" name)
          (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
            (label title :font-size 10 :color :gray :bg :transparent)
            (number-picker :value (get p :value)
              :min (get p :min) :max (get p :max) :decimals decimals
              :unit unit
              :noui true :font-size 10.5
              :text-align :center
              :text-color :widget_focus_bg :edit-color :yellow
              :width 5.0 :height 0.95
              :on-change (lambda (v)
                (do
                  (monomachine_fmplus-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :gray :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :gray :edit-color :gray
          :width 5.0 :height 0.95)))))
(def monomachine_fmplus-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def monomachine_fmplus-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def monomachine_fmplus-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (monomachine_fmplus-param-value attack 4)
    :decay (monomachine_fmplus-param-value decay 400)
    :sustain (monomachine_fmplus-param-value sustain 0.5)
    :release (monomachine_fmplus-param-value release 0)
    :width 22.0 :height 3.55
    :background-color (rgba 0.0 0.0 0.0 1)
    :on-change (lambda (env)
      (do
        (monomachine_fmplus-select section)
        (monomachine_fmplus-set-param attack (get env :attack))
        (monomachine_fmplus-set-param decay (get env :decay))
        (monomachine_fmplus-set-param sustain (get env :sustain))
        (monomachine_fmplus-set-param release (get env :release))))))
(def monomachine_fmplus-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-param-number-section attack "atk" 0 "ms" section)
      (monomachine_fmplus-param-number-section decay "dec" 0 "ms" section)
      (monomachine_fmplus-param-number-section sustain "sus" 2 false section)
      (monomachine_fmplus-param-number-section release "rel" 0 "ms" section))))

(def monomachine_fmplus-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :gray :bg :transparent)))
(def monomachine_fmplus-selected-adsr ()
  (box :width :fill :height 6.55
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (monomachine_fmplus-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (monomachine_fmplus-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (monomachine_fmplus-adsr-caption "AMP ENV"))))
(def monomachine_fmplus-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :gray :bg :transparent)))
(def monomachine_fmplus-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (monomachine_fmplus-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_fmplus-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-row-label title)
      c1)))
(def monomachine_fmplus-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (monomachine_fmplus-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_fmplus-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-row-label title)
      c1 c2)))
(def monomachine_fmplus-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (monomachine_fmplus-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_fmplus-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-row-label title)
      c1 c2 c3)))
(def monomachine_fmplus-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (monomachine_fmplus-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_fmplus-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-row-label title)
      c1 c2 c3 c4)))
(def monomachine_fmplus-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (monomachine_fmplus-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_fmplus-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-row-label title)
      c1 c2 c3 c4 c5)))
(def monomachine_fmplus-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (monomachine_fmplus-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_fmplus-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def monomachine_fmplus-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (monomachine_fmplus-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_fmplus-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def monomachine_fmplus-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (monomachine_fmplus-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (monomachine_fmplus-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (monomachine_fmplus-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (monomachine_fmplus-panel-1 "GLOB" 0
        (monomachine_fmplus-base-note-cell 0))
      (monomachine_fmplus-panel-4 "RATIO" 0
        (monomachine_fmplus-param-cell-step-section "ratio_a" "a" 2 0.125 0)
        (monomachine_fmplus-param-cell-step-section "ratio_b" "b" 2 0.125 0)
        (monomachine_fmplus-param-cell-step-section "ratio_c" "c" 2 0.125 0)
        (monomachine_fmplus-param-cell-step-section "ratio_d" "d" 2 0.125 0))
      (monomachine_fmplus-panel-3 "IDX" 0
        (monomachine_fmplus-param-cell-section "index_a" "a" 2 0)
        (monomachine_fmplus-param-cell-section "index_b" "b" 2 0)
        (monomachine_fmplus-param-cell-section "dyn_index_amt" "dyn" 2 0)))
    (v-stack :width 23.1 :gap 0.10
      (monomachine_fmplus-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (monomachine_fmplus-panel-4 "FDBK" 0
        (monomachine_fmplus-param-cell-section "feedback_a" "a" 2 0)
        (monomachine_fmplus-param-cell-section "feedback_b" "b" 2 0)
        (monomachine_fmplus-param-cell-section "crossmod" "xmod" 2 0)
        (monomachine_fmplus-param-cell-section "self_fm" "self" 2 0))
      (monomachine_fmplus-panel-4 "CHAO" 0
        (monomachine_fmplus-param-cell-section "ratio_warp" "warp" 2 0)
        (monomachine_fmplus-param-cell-section "glitch_rate" "rate" 0 0)
        (monomachine_fmplus-param-cell-section "glitch_amt" "amt" 2 0)
        (monomachine_fmplus-param-cell-section "parallel_mix" "mix" 2 0))
      (monomachine_fmplus-panel-5 "TONE" 0
        (monomachine_fmplus-param-cell-section "tone" "tone" 0 0)
        (monomachine_fmplus-param-cell-section "resonance" "res" 2 0)
        (monomachine_fmplus-param-cell-section "keytrack" "key" 2 0)
        (monomachine_fmplus-param-cell-section "filter_drive" "drive" 2 0)
        (monomachine_fmplus-param-cell-section "gain" "gain" 2 0)))))
