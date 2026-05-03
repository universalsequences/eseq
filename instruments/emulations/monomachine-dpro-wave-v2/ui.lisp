;; Custom Synth tab body for instruments/emulations/monomachine-dpro-wave-v2/dsp.lisp
(defstate monomachine-dpro-wave-v2-selected-section 0)
(def mdp2-select (section)
  (set! monomachine-dpro-wave-v2-selected-section section))
(def mdp2-panel-bg (section)
  (if (= monomachine-dpro-wave-v2-selected-section section)
    (rgba 0.12 0.12 0.12 1)
    (rgba 0.075 0.075 0.075 1)))
(def mdp2-cell-width 4.0)
(def mdp2-param-cell-step-section-width (name title decimals step section width)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mdp2-cell-" name)
        (knob-number :label title
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals decimals
          :step step
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width width :height 2.05
          :on-change (lambda (v)
            (do
              (mdp2-select section)
              (fx-set-instrument-value p v)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mdp2-param-cell-step-section (name title decimals step section)
  (mdp2-param-cell-step-section-width name title decimals step section mdp2-cell-width))
(def mdp2-param-cell-section (name title decimals section)
  (mdp2-param-cell-step-section name title decimals 0 section))
(def mdp2-base-note-cell (section)
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key "mdp2-base-note-cell"
        (knob-number :label "note"
          :value (get p :value)
          :min (get p :min) :max (get p :max) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :gray :label-color :gray
          :width mdp2-cell-width :height 2.05
          :on-change (lambda (v)
            (do
              (mdp2-select section)
              (fx-set-instrument-value p v)))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
(def mdp2-param-value (name fallback)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (get p :value) fallback))
    fallback))
(def mdp2-set-param (name value)
  (if name
    (let ((p (inst-param synth-ui-current-inst name)))
      (if p (fx-set-instrument-value p value) false))
    false))
(def mdp2-param-number-section (name title decimals unit section)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (subtree :key (str "mdp2-number-" name)
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
                (mdp2-select section)
                (fx-set-instrument-value p v))))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
(def mdp2-adsr-view (attack decay sustain release section)
  (adsr-editor
    :attack (mdp2-param-value attack 2)
    :decay (mdp2-param-value decay 120)
    :sustain (mdp2-param-value sustain 0.78)
    :release (mdp2-param-value release 90)
    :width 22.0 :height 4.0
    :background-color (rgba 0.0 0.0 0.0 1)
    :on-change (lambda (env)
      (do
        (mdp2-select section)
        (mdp2-set-param attack (get env :attack))
        (mdp2-set-param decay (get env :decay))
        (mdp2-set-param sustain (get env :sustain))
        (mdp2-set-param release (get env :release))))))
(def mdp2-adsr-controls (attack decay sustain release section)
  (box :width :fill :height 1.95 :padding 0.25
    (h-stack :width :fill :gap 0.20 :align :start
      (mdp2-param-number-section attack "atk" 0 "ms" section)
      (mdp2-param-number-section decay "dec" 0 "ms" section)
      (mdp2-param-number-section sustain "sus" 2 false section)
      (mdp2-param-number-section release "rel" 0 "ms" section))))
(def mdp2-adsr-panel-for (attack decay sustain release section)
  (box :width :fill :height 6.35
       :background-color (rgba 0.0 0.0 0.0 1)
       :border-width 1 :corner-radius 8 :padding 0.15
    (v-stack :width :fill :gap 0.10
      (mdp2-adsr-view attack decay sustain release section)
      (mdp2-adsr-controls attack decay sustain release section))))
(def mdp2-adsr-panel ()
  (if (= monomachine-dpro-wave-v2-selected-section 2)
    (mdp2-adsr-panel-for "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms" 2)
    (mdp2-adsr-panel-for "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms" 1)))
(def mdp2-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :gray :bg :transparent)))
(def mdp2-panel-1 (title section c1)
  (box :width :fill :height 2.35
       :background-color (mdp2-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdp2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdp2-row-label title)
      c1)))
(def mdp2-panel-2 (title section c1 c2)
  (box :width :fill :height 2.35
       :background-color (mdp2-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdp2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdp2-row-label title)
      c1 c2)))
(def mdp2-panel-3 (title section c1 c2 c3)
  (box :width :fill :height 2.35
       :background-color (mdp2-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdp2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdp2-row-label title)
      c1 c2 c3)))
(def mdp2-panel-4 (title section c1 c2 c3 c4)
  (box :width :fill :height 2.35
       :background-color (mdp2-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdp2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdp2-row-label title)
      c1 c2 c3 c4)))
(def mdp2-panel-5 (title section c1 c2 c3 c4 c5)
  (box :width :fill :height 2.35
       :background-color (mdp2-panel-bg section)
       :border-width 1 :corner-radius 8 :padding 0.1
       :on-click (lambda (info) (mdp2-select section))
    (h-stack :width :fill :gap 0.20 :align :start
      (mdp2-row-label title)
      c1 c2 c3 c4 c5)))
(defsynth-ui
  (h-stack :width :fill :gap 0.45 :align :start
    (v-stack :width 27.2 :gap 0.10
      (mdp2-panel-1 "GLOB" 0
        (mdp2-base-note-cell 0))
      (mdp2-panel-3 "DPRO" 0
        (mdp2-param-cell-step-section "wave" "wave" 0 1 0)
        (mdp2-param-cell-step-section "wp" "wp" 0 1 0)
        (mdp2-param-cell-section "tune_cents" "tune" 0 0))
      (mdp2-panel-2 "SYNC" 0
        (mdp2-param-cell-step-section "sync_mode" "mode" 0 1 0)
        (mdp2-param-cell-section "sfrq" "sfrq" 0 0)))
    (v-stack :width 23.1 :gap 0.10
      (mdp2-adsr-panel))
    (v-stack :width 27.2 :gap 0.10
      (mdp2-panel-4 "FILT" 2
        (mdp2-param-cell-section "cutoff" "cut" 0 2)
        (mdp2-param-cell-section "resonance" "res" 2 2)
        (mdp2-param-cell-section "keytrack" "key" 2 2)
        (mdp2-param-cell-section "filter_env_amt" "env" 0 2))
      (mdp2-panel-2 "OUT" 0
        (mdp2-param-cell-section "drive" "drv" 2 0)
        (mdp2-param-cell-section "gain" "gain" 2 0)))))
