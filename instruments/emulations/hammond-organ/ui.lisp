;; Custom Synth tab body for instruments/emulations/hammond-organ/dsp.lisp
(defstate hammond-organ-selected-section 0)
(def hammond_organ-select (section)
  (set! hammond-organ-selected-section section))
(def hammond_organ-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= hammond-organ-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def hammond_organ-cell-width 4.0)
(def hammond_organ-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "hammond_organ-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (hammond_organ-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def hammond_organ-param-cell-step-section (name title decimals step section)
  (hammond_organ-param-cell-step-section-width name title decimals step section hammond_organ-cell-width))
(def hammond_organ-param-cell-section (name title decimals section)
  (hammond_organ-param-cell-step-section name title decimals 0 section))
(def hammond_organ-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "hammond_organ-base-note-cell")
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width hammond_organ-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (hammond_organ-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def hammond_organ-param-number-section (name title decimals unit section)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p
        (subtree :key (str "hammond_organ-adsr-number-" name)
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
                  (hammond_organ-select section)
                  (fx-set-instrument-value p v))))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :dim :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit :noui true :font-size 10.5
          :text-align :center :text-color :dim :edit-color :dim
          :width 5.0 :height 0.95)))))
(def hammond_organ-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def hammond_organ-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (hammond_organ-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (hammond_organ-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (hammond_organ-row-label title)
      c1)))
(def hammond_organ-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (hammond_organ-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (hammond_organ-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (hammond_organ-row-label title)
      c1 c2)))
(def hammond_organ-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (hammond_organ-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (hammond_organ-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (hammond_organ-row-label title)
      c1 c2 c3)))
(def hammond_organ-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (hammond_organ-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (hammond_organ-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (hammond_organ-row-label title)
      c1 c2 c3 c4)))
(def hammond_organ-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (hammond_organ-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (hammond_organ-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (hammond_organ-row-label title)
      c1 c2 c3 c4 c5)))
(def hammond_organ-panel-6 (title section c1 c2 c3 c4 c5 c6)
  (box :width :fill :height 2.35
       :background-color (hammond_organ-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (hammond_organ-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (hammond_organ-row-label title)
      c1 c2 c3 c4 c5 c6)))
(def hammond_organ-panel-7 (title section c1 c2 c3 c4 c5 c6 c7)
  (box :width :fill :height 2.35
       :background-color (hammond_organ-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (hammond_organ-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (hammond_organ-row-label title)
      c1 c2 c3 c4 c5 c6 c7)))
(def hammond_organ-panel-8 (title section c1 c2 c3 c4 c5 c6 c7 c8)
  (box :width :fill :height 2.35
       :background-color (hammond_organ-panel-bg section)
       :border-width 1 :corner-radius 16 :padding 0.1
       :on-click (lambda (info) (hammond_organ-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (hammond_organ-row-label title)
      c1 c2 c3 c4 c5 c6 c7 c8)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 31.0 :gap 0.10
      (hammond_organ-panel-1 "GLOB" 0
        (hammond_organ-base-note-cell 0))
      (hammond_organ-panel-5 "16-4" 0
        (hammond_organ-param-cell-section "draw1" "16" 2 0)
        (hammond_organ-param-cell-section "draw2" "8" 2 0)
        (hammond_organ-param-cell-section "draw3" "5 1/3" 2 0)
        (hammond_organ-param-cell-section "draw4" "4" 2 0)
        (hammond_organ-param-cell-section "draw5" "2 2/3" 2 0))
      (hammond_organ-panel-4 "2-1" 0
        (hammond_organ-param-cell-section "draw6" "2" 2 0)
        (hammond_organ-param-cell-section "draw7" "1 3/5" 2 0)
        (hammond_organ-param-cell-section "draw8" "1 1/3" 2 0)
        (hammond_organ-param-cell-section "draw9" "1" 2 0)))
    (v-stack :width 31.0 :gap 0.10
      (hammond_organ-panel-2 "CLICK" 1
        (hammond_organ-param-cell-section "click_amt" "amt" 2 1)
        (hammond_organ-param-cell-section "click_decay" "dec" 0 1))
      (hammond_organ-panel-2 "PERC" 1
        (hammond_organ-param-cell-section "perc_level" "level" 2 1)
        (hammond_organ-param-cell-section "perc_decay" "dec" 0 1))
      (hammond_organ-panel-3 "ROT" 2
        (hammond_organ-param-cell-section "rotary_speed" "speed" 2 2)
        (hammond_organ-param-cell-section "rotary_depth" "depth" 2 2)
        (hammond_organ-param-cell-section "rotary_doppler" "dop" 2 2))
      (hammond_organ-panel-2 "OUT" 2
        (hammond_organ-param-cell-section "drive" "drive" 2 2)
        (hammond_organ-param-cell-section "gain" "gain" 2 2)))))
