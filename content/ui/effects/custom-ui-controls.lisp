;; Standard generated-UI controls.
(module eseq.effects.custom-ui-controls)

(import eseq.effects.custom-ui-runtime :as rt)
(import eseq.effects.custom-ui-sections :as sec)
(import eseq.effects.param-controls :as pc)

(export ui-param-knob
        ui-param-matrix
        ui-param-knob-c
        base-note-c
        ui-panel-c
        ui-param-value
        ui-param-bound-value
        ui-set-param)

;; Identity aliases: these names are the generated-instrument-UI vocabulary.
;; Bare callers: unconverted custom-ui-lego.lisp, generated
;; instruments/**/ui.lisp files (evaluated headerless), the ui_validate.rs
;; whitelist/stubs, and headerless evals in state_values/tests.rs.

(def ui-param-knob (name title)
  (let ((p (rt/custom-ui-current-param name)))
    (if p
      (rt/custom-ui-param-mod-wrapper p (str "custom-ui-knob-mod-" (rt/custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-knob-" (rt/custom-ui-scope-name) (rt/custom-ui-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (rt/custom-ui-param-binding p)
            :min (rt/custom-ui-param-control-min p) :max (rt/custom-ui-param-control-max p) :decimals 2
            :base-value (rt/custom-ui-param-base-value-prop p)
            :modulated-value (rt/custom-ui-param-modulated-value p)
            :base-min (rt/custom-ui-param-base-min-prop p) :base-max (rt/custom-ui-param-base-max-prop p)
            :mod-range-0-slot (rt/custom-ui-param-knob-mod-slot-prop p 0) :mod-range-0-depth (rt/custom-ui-param-knob-mod-depth-prop p 0)
            :mod-range-1-slot (rt/custom-ui-param-knob-mod-slot-prop p 1) :mod-range-1-depth (rt/custom-ui-param-knob-mod-depth-prop p 1)
            :mod-range-2-slot (rt/custom-ui-param-knob-mod-slot-prop p 2) :mod-range-2-depth (rt/custom-ui-param-knob-mod-depth-prop p 2)
            :mod-range-3-slot (rt/custom-ui-param-knob-mod-slot-prop p 3) :mod-range-3-depth (rt/custom-ui-param-knob-mod-depth-prop p 3)
            :mod-range-4-slot (rt/custom-ui-param-knob-mod-slot-prop p 4) :mod-range-4-depth (rt/custom-ui-param-knob-mod-depth-prop p 4)
            :mod-range-5-slot (rt/custom-ui-param-knob-mod-slot-prop p 5) :mod-range-5-depth (rt/custom-ui-param-knob-mod-depth-prop p 5)
            :mod-range-6-slot (rt/custom-ui-param-knob-mod-slot-prop p 6) :mod-range-6-depth (rt/custom-ui-param-knob-mod-depth-prop p 6)
            :mod-range-7-slot (rt/custom-ui-param-knob-mod-slot-prop p 7) :mod-range-7-depth (rt/custom-ui-param-knob-mod-depth-prop p 7)
            :mod-range-8-slot (rt/custom-ui-param-knob-mod-slot-prop p 8) :mod-range-8-depth (rt/custom-ui-param-knob-mod-depth-prop p 8)
            :mod-range-9-slot (rt/custom-ui-param-knob-mod-slot-prop p 9) :mod-range-9-depth (rt/custom-ui-param-knob-mod-depth-prop p 9)
            :selected-mod-slot (rt/custom-ui-selected-mod-slot-prop p)
            :font-size 10.5 :label-font-size 10
            :text-color (rt/custom-ui-param-plock-text-color p) :label-color :dim
            :plock-active (if (rt/custom-ui-param-plock-active? p) 1 0)
            :plock-default (rt/custom-ui-param-plock-default p)
            :plock-color-r (pc/param-plock-color-r)
            :plock-color-g (pc/param-plock-color-g)
            :plock-color-b (pc/param-plock-color-b)
            :width 4.4 :height 2.4
            :value-align :center
            :on-change (rt/custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-param-matrix (name width height)
  (let ((p (rt/custom-ui-current-tensor-param name)))
    (if p
      (subtree :key (str "custom-ui-matrix-" (rt/custom-ui-scope-name) "-" name)
        (matrix :rows (get p :rows) :cols (get p :cols)
          :value (rt/custom-ui-tensor-bound-values p)
          :min (get p :min) :max (get p :max)
          :control :grid
          :width width :height height
          :on-cell-change (rt/custom-ui-tensor-cell-change-callback p)))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

;; Compact knob: ~1.7 cell tall, value nestled in the lower-right of the knob
;; arc (default value-align) so the knob itself stays large. For instruments
;; that need 3-4 rows of params instead of 2.
(def ui-param-knob-c (name title)
  (let ((p (rt/custom-ui-current-param name)))
    (if p
      (rt/custom-ui-param-mod-wrapper p (str "custom-ui-knob-c-mod-" (rt/custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-knob-c-" (rt/custom-ui-scope-name) (rt/custom-ui-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (rt/custom-ui-param-binding p)
            :min (rt/custom-ui-param-control-min p) :max (rt/custom-ui-param-control-max p) :decimals 2
            :base-value (rt/custom-ui-param-base-value-prop p)
            :modulated-value (rt/custom-ui-param-modulated-value p)
            :base-min (rt/custom-ui-param-base-min-prop p) :base-max (rt/custom-ui-param-base-max-prop p)
            :mod-range-0-slot (rt/custom-ui-param-knob-mod-slot-prop p 0) :mod-range-0-depth (rt/custom-ui-param-knob-mod-depth-prop p 0)
            :mod-range-1-slot (rt/custom-ui-param-knob-mod-slot-prop p 1) :mod-range-1-depth (rt/custom-ui-param-knob-mod-depth-prop p 1)
            :mod-range-2-slot (rt/custom-ui-param-knob-mod-slot-prop p 2) :mod-range-2-depth (rt/custom-ui-param-knob-mod-depth-prop p 2)
            :mod-range-3-slot (rt/custom-ui-param-knob-mod-slot-prop p 3) :mod-range-3-depth (rt/custom-ui-param-knob-mod-depth-prop p 3)
            :mod-range-4-slot (rt/custom-ui-param-knob-mod-slot-prop p 4) :mod-range-4-depth (rt/custom-ui-param-knob-mod-depth-prop p 4)
            :mod-range-5-slot (rt/custom-ui-param-knob-mod-slot-prop p 5) :mod-range-5-depth (rt/custom-ui-param-knob-mod-depth-prop p 5)
            :mod-range-6-slot (rt/custom-ui-param-knob-mod-slot-prop p 6) :mod-range-6-depth (rt/custom-ui-param-knob-mod-depth-prop p 6)
            :mod-range-7-slot (rt/custom-ui-param-knob-mod-slot-prop p 7) :mod-range-7-depth (rt/custom-ui-param-knob-mod-depth-prop p 7)
            :mod-range-8-slot (rt/custom-ui-param-knob-mod-slot-prop p 8) :mod-range-8-depth (rt/custom-ui-param-knob-mod-depth-prop p 8)
            :mod-range-9-slot (rt/custom-ui-param-knob-mod-slot-prop p 9) :mod-range-9-depth (rt/custom-ui-param-knob-mod-depth-prop p 9)
            :selected-mod-slot (rt/custom-ui-selected-mod-slot-prop p)
            :font-size 8.5 :label-font-size 7.5
            :text-color (rt/custom-ui-param-plock-text-color p) :label-color :dim
            :plock-active (if (rt/custom-ui-param-plock-active? p) 1 0)
            :plock-default (rt/custom-ui-param-plock-default p)
            :plock-color-r (pc/param-plock-color-r)
            :plock-color-g (pc/param-plock-color-g)
            :plock-color-b (pc/param-plock-color-b)
            :width 3.8 :height 1.8
            :label-height 0.5 :knob-size 1.25
            :on-change (rt/custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def base-note-c ()
  (let ((p (rt/custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-base-note-c-" (rt/custom-ui-scope-name))
        (knob-number :label "note"
          :value (rt/custom-ui-param-binding p)
          :min (rt/custom-ui-param-control-min p) :max (rt/custom-ui-param-control-max p) :decimals 0
          :step 1
          :font-size 8.5 :label-font-size 7.5
          :text-color (rt/custom-ui-param-plock-text-color p) :label-color :dim
          :plock-active (if (rt/custom-ui-param-plock-active? p) 1 0)
          :plock-default (rt/custom-ui-param-plock-default p)
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :width 3.8 :height 1.8
          :label-height 0.5 :knob-size 1.25
          :on-change (rt/custom-ui-param-change-callback p)))
      (label "missing: base_note" :font-size 9 :color :red :bg :transparent))))

(def ui-panel-header-c (title)
  (box :width 3.5 :height :fill :h-align :end :v-align :center :padding 0.1
    (label title :font-size 6 :color :dim :bg :transparent)))

;; Compact panel: title runs along the LEFT edge (vertical strip) so each
;; row only takes the height of one knob — no separate title band on top.
(def ui-panel-c (title section body)
  (box :width :fill :height 2.0
       :background-color (sec/ui-panel-bg section)
       :border-width 1 :corner-radius 10 :padding 0.08
       :on-click (sec/ui-section-select-callback section)
    (h-stack :width :fill :gap 0.1 :align :center
      (ui-panel-header-c title)
      body)))

(def ui-param-value (name fallback)
  (let ((p (rt/custom-ui-current-param name)))
    (if p (rt/custom-ui-param-value p) fallback)))

(def ui-param-bound-value (name fallback)
  (let ((p (rt/custom-ui-current-param name)))
    (if p (rt/custom-ui-param-binding p) fallback)))

(def ui-set-param (name value)
  (let ((p (rt/custom-ui-current-param name)))
    (if p (rt/custom-ui-set-param p value) false)))
