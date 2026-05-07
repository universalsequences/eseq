;; Custom Synth tab body for instruments/emulations/dx7-4op/dsp.lisp
(defstate dx7-4op-selected-section 0)
(def dx7_4op-select (section)
  (set! dx7-4op-selected-section section))
(def dx7_4op-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= dx7-4op-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def dx7_4op-cell-width 4.0)
(def dx7_4op-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "dx7_4op-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (dx7_4op-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def dx7_4op-param-cell-step-section (name title decimals step section)
  (dx7_4op-param-cell-step-section-width name title decimals step section dx7_4op-cell-width))
(def dx7_4op-param-cell-section (name title decimals section)
  (dx7_4op-param-cell-step-section name title decimals 0 section))
(def dx7_4op-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "dx7_4op-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width dx7_4op-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (dx7_4op-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def dx7_4op-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "dx7_4op-adsr-number-" name)
          (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
            (label title :font-size 10 :color :dim :bg :transparent)
            (number-picker :value (get p :value)
              :min (get p :min) :max (get p :max) :decimals decimals
              :unit unit
              :noui true :font-size 10.5
              :text-align :center
              :text-color :widget_focus_bg :edit-color :yellow
              :width 5.0 :height 0.95
              :on-change (lambda (v)
                (do
                  (dx7_4op-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :dim :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :dim :edit-color :dim
          :width 5.0 :height 0.95)))))
(def dx7_4op-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def dx7_4op-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def dx7_4op-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (dx7_4op-param-value attack 4)
    :decay (dx7_4op-param-value decay 400)
    :sustain (dx7_4op-param-value sustain 0.5)
    :release (dx7_4op-param-value release 0)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (dx7_4op-select section)
        (dx7_4op-set-param attack (get env :attack))
        (dx7_4op-set-param decay (get env :decay))
        (dx7_4op-set-param sustain (get env :sustain))
        (dx7_4op-set-param release (get env :release))))))
(def dx7_4op-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-param-number-section attack "atk" 0 "ms" section)
      (dx7_4op-param-number-section decay "dec" 0 "ms" section)
      (dx7_4op-param-number-section sustain "sus" 2 false section)
      (dx7_4op-param-number-section release "rel" 0 "ms" section))))

(def dx7_4op-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def dx7_4op-selected-adsr ()
  (if (= dx7-4op-selected-section 1)
    (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (dx7_4op-adsr-view false "mod_decay_ms" "mod_sustain" false 1)
    (dx7_4op-adsr-controls false "mod_decay_ms" "mod_sustain" false 1)
    (dx7_4op-adsr-caption "MOD ENV")))
    (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 16 :padding 0.15
  (v-stack :width :fill :gap 0.10
    (dx7_4op-adsr-view "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (dx7_4op-adsr-controls "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 0)
    (dx7_4op-adsr-caption "AMP ENV")))))
(def dx7_4op-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def dx7_4op-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (dx7_4op-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (dx7_4op-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-row-label title)
      c1)))
(def dx7_4op-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (dx7_4op-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (dx7_4op-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-row-label title)
      c1 c2)))
(def dx7_4op-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (dx7_4op-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (dx7_4op-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-row-label title)
      c1 c2 c3)))
(def dx7_4op-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (dx7_4op-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (dx7_4op-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-row-label title)
      c1 c2 c3 c4)))
(def dx7_4op-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (dx7_4op-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (dx7_4op-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-row-label title)
      c1 c2 c3 c4 c5)))
(def dx7_4op-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (dx7_4op-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (dx7_4op-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def dx7_4op-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (dx7_4op-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (dx7_4op-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def dx7_4op-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (dx7_4op-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (dx7_4op-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (dx7_4op-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (dx7_4op-panel-1 "GLOB" 0
        (dx7_4op-base-note-cell 0))
      (dx7_4op-panel-4 "RATIO" 1
        (dx7_4op-param-cell-step-section "ratio1" "r1" 2 0.25 1)
        (dx7_4op-param-cell-step-section "ratio2" "r2" 2 0.25 1)
        (dx7_4op-param-cell-step-section "ratio3" "r3" 2 0.25 1)
        (dx7_4op-param-cell-step-section "ratio4" "r4" 2 0.25 1))
      (dx7_4op-panel-5 "OP LVL" 1
        (dx7_4op-param-cell-section "level1" "l1" 2 1)
        (dx7_4op-param-cell-section "level3" "l3" 2 1)
        (dx7_4op-param-cell-section "index2" "i2" 2 1)
        (dx7_4op-param-cell-section "index3" "i3" 2 1)
        (dx7_4op-param-cell-section "index4" "i4" 2 1)))
    (v-stack :width 23.1 :gap 0.10
      (dx7_4op-selected-adsr))
    (v-stack :width 29.0 :gap 0.10
      (dx7_4op-panel-2 "ALGO" 1
        (dx7_4op-param-cell-step-section "algorithm" "algo" 0 1 1)
        (dx7_4op-param-cell-section "feedback" "feed" 2 1))
      (dx7_4op-panel-3 "DYN" 1
        (dx7_4op-param-cell-section "vel_to_index" "vel idx" 2 1)
        (dx7_4op-param-cell-section "amp_vel_amt" "vel amp" 2 1)
        (dx7_4op-param-cell-section "gain" "gain" 2 1)))))
