;; Custom Synth tab body for instruments/emulations/rhodes-additive-v2/dsp.lisp
(defstate rhodes-additive-v2-selected-section 0)
(def rhodes_additive_v2-select (section)
  (set! rhodes-additive-v2-selected-section section))
(def rhodes_additive_v2-panel-bg (section)
  (if (= rhodes-additive-v2-selected-section section)
    (rgba 0.12 0.12 0.12 1)
    (rgba 0.09 0.09 0.09 1)))
(def rhodes_additive_v2-cell-width 4.0)
(def rhodes_additive_v2-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "rhodes_additive_v2-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (rhodes_additive_v2-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def rhodes_additive_v2-param-cell-step-section (name title decimals step section)
  (rhodes_additive_v2-param-cell-step-section-width name title decimals step section rhodes_additive_v2-cell-width))
(def rhodes_additive_v2-param-cell-section (name title decimals section)
  (rhodes_additive_v2-param-cell-step-section name title decimals 0 section))
(def rhodes_additive_v2-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "rhodes_additive_v2-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width rhodes_additive_v2-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (rhodes_additive_v2-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def rhodes_additive_v2-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "rhodes_additive_v2-adsr-number-" name)
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
                  (rhodes_additive_v2-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 4.35 :height 1.75
      (v-stack :width 4.35 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :gray :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :gray :edit-color :gray
          :width 4.2 :height 0.95)))))
(def rhodes_additive_v2-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :gray :bg :transparent)))
(def rhodes_additive_v2-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (rhodes_additive_v2-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (rhodes_additive_v2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (rhodes_additive_v2-row-label title)
      c1)))
(def rhodes_additive_v2-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (rhodes_additive_v2-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (rhodes_additive_v2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (rhodes_additive_v2-row-label title)
      c1 c2)))
(def rhodes_additive_v2-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (rhodes_additive_v2-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (rhodes_additive_v2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (rhodes_additive_v2-row-label title)
      c1 c2 c3)))
(def rhodes_additive_v2-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (rhodes_additive_v2-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (rhodes_additive_v2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (rhodes_additive_v2-row-label title)
      c1 c2 c3 c4)))
(def rhodes_additive_v2-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (rhodes_additive_v2-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (rhodes_additive_v2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (rhodes_additive_v2-row-label title)
      c1 c2 c3 c4 c5)))
(def rhodes_additive_v2-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (rhodes_additive_v2-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (rhodes_additive_v2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (rhodes_additive_v2-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def rhodes_additive_v2-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (rhodes_additive_v2-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (rhodes_additive_v2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (rhodes_additive_v2-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def rhodes_additive_v2-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (rhodes_additive_v2-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (rhodes_additive_v2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (rhodes_additive_v2-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 31.0 :gap 0.10
      (rhodes_additive_v2-panel-1 "GLOB" 0
        (rhodes_additive_v2-base-note-cell 0))
      (rhodes_additive_v2-panel-3 "BAR" 0
        (rhodes_additive_v2-param-cell-section "decay" "decay" 0 0)
        (rhodes_additive_v2-param-cell-section "harmonic_2" "h2" 2 0)
        (rhodes_additive_v2-param-cell-section "harmonic_4" "h4" 2 0))
      (rhodes_additive_v2-panel-3 "TINE" 0
        (rhodes_additive_v2-param-cell-section "tine_vol" "tine" 2 0)
        (rhodes_additive_v2-param-cell-section "bark_amt" "bark" 2 0)
        (rhodes_additive_v2-param-cell-section "detune" "det" 2 0)))
    (v-stack :width 31.0 :gap 0.10
      (rhodes_additive_v2-panel-2 "VIB" 1
        (rhodes_additive_v2-param-cell-section "vib_speed" "speed" 2 1)
        (rhodes_additive_v2-param-cell-section "vib_depth" "depth" 2 1))
      (rhodes_additive_v2-panel-1 "OUT" 1
        (rhodes_additive_v2-param-cell-section "gain" "gain" 2 1)))))
