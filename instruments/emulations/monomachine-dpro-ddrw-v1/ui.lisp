;; Custom Synth tab body for instruments/emulations/monomachine-dpro-ddrw-v1/dsp.lisp
(defstate monomachine-dpro-ddrw-v1-selected-section 0)
(def mddrw-select (section)
  (set! monomachine-dpro-ddrw-v1-selected-section section))
(def mddrw-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= monomachine-dpro-ddrw-v1-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))
(def mddrw-cell-width 4.0)
(def mddrw-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mddrw-cell-" name)
        (knob-number :label title
          :value (fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (mddrw-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mddrw-param-cell-step-section (name title decimals step section)
  (mddrw-param-cell-step-section-width name title decimals step section mddrw-cell-width))
(def mddrw-param-cell-section (name title decimals section)
  (mddrw-param-cell-step-section name title decimals 0 section))
(def mddrw-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key "mddrw-base-note-cell"
        (knob-number :label "note"
          :value (fx-param-value p)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width mddrw-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (mddrw-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def mddrw-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-param-value p) fallback))
    fallback))
(def mddrw-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def mddrw-param-number-section (name title decimals unit section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mddrw-number-" name)
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
                (mddrw-select section)
                (fx-set-instrument-value p v))))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mddrw-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (mddrw-param-value attack 2)
    :decay (mddrw-param-value decay 120)
    :sustain (mddrw-param-value sustain 0.78)
    :release (mddrw-param-value release 90)
    :width 22.0 :height 3.55
    :background-color :instrument-control-bg
    :on-change (lambda (env)
      (do
        (mddrw-select section)
        (mddrw-set-param attack (get env :attack))
        (mddrw-set-param decay (get env :decay))
        (mddrw-set-param sustain (get env :sustain))
        (mddrw-set-param release (get env :release))))))
(def mddrw-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.75 :padding 0.15
    (h-stack :width :fill :gap 0.20 :align :start
      (mddrw-param-number-section attack "atk" 0 "ms" section)
      (mddrw-param-number-section decay "dec" 0 "ms" section)
      (mddrw-param-number-section sustain "sus" 2 false section)
      (mddrw-param-number-section release "rel" 0 "ms" section))))

(def mddrw-adsr-caption (title)
  (box :width :fill :height 0.35 :h-align :center :v-align :center
    (label title :font-size 8.5 :color :dim :bg :transparent)))
(def mddrw-adsr-panel-for (title attack decay sustain release section)
  (box :width :fill :height 6.55
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 8 :padding 0.15
    (v-stack :width :fill :gap 0.10
      (mddrw-adsr-view attack decay sustain release section)
      (mddrw-adsr-controls attack decay sustain release section)
      (mddrw-adsr-caption title))))
(def mddrw-adsr-panel ()
  (if (= monomachine-dpro-ddrw-v1-selected-section 2)
    (mddrw-adsr-panel-for "FILTER ENV" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms" 2)
    (mddrw-adsr-panel-for "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 1)))
(def mddrw-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))
(def mddrw-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (mddrw-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mddrw-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mddrw-row-label title)
      c1)))
(def mddrw-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (mddrw-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mddrw-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mddrw-row-label title)
      c1 c2)))
(def mddrw-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (mddrw-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mddrw-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mddrw-row-label title)
      c1 c2 c3)))
(def mddrw-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (mddrw-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mddrw-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mddrw-row-label title)
      c1 c2 c3 c4)))
(def mddrw-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (mddrw-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mddrw-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mddrw-row-label title)
      c1 c2 c3 c4 c5)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (mddrw-panel-1 "GLOB" 0
        (mddrw-base-note-cell 0))
      (mddrw-panel-3 "DDRW" 0
        (mddrw-param-cell-step-section "wav1" "wav1" 0 1 0)
        (mddrw-param-cell-section "mix" "mix" 2 0)
        (mddrw-param-cell-step-section "wav2" "wav2" 0 1 0))
      (mddrw-panel-5 "DRAW" 0
        (mddrw-param-cell-step-section "time" "time" 0 1 0)
        (mddrw-param-cell-step-section "br1" "br1" 0 1 0)
        (mddrw-param-cell-section "wid" "wid" 0 0)
        (mddrw-param-cell-step-section "br2" "br2" 0 1 0)
        (mddrw-param-cell-section "tune_cents" "tune" 0 0))
      )
    (v-stack :width 23.1 :gap 0.10
      (mddrw-adsr-panel))
    (v-stack :width 27.2 :gap 0.10
      (mddrw-panel-4 "FILT" 2
        (mddrw-param-cell-section "cutoff" "cut" 0 2)
        (mddrw-param-cell-section "resonance" "res" 2 2)
        (mddrw-param-cell-section "keytrack" "key" 2 2)
        (mddrw-param-cell-section "filter_env_amt" "env" 0 2))
      (mddrw-panel-2 "OUT" 0
        (mddrw-param-cell-section "drive" "drv" 2 0)
        (mddrw-param-cell-section "gain" "gain" 2 0)))))
