;; Dense generated-UI layout primitives and rack helpers.
(module eseq.effects.custom-ui-lego)

(import eseq.effects.custom-ui-runtime :refer
  (custom-ui-current-param custom-ui-current-tensor-param
   custom-ui-current-base-note-param custom-ui-current-scope
   custom-ui-scope-name custom-ui-param-mod-wrapper
   custom-ui-param-control-key-mode custom-ui-param-binding
   custom-ui-param-value custom-ui-param-control-min
   custom-ui-param-control-max custom-ui-param-base-value-prop
   custom-ui-param-base-min-prop custom-ui-param-base-max-prop
   custom-ui-param-knob-mod-slot-prop custom-ui-param-knob-mod-depth-prop
   custom-ui-selected-mod-slot-prop custom-ui-param-plock-text-color
   custom-ui-param-plock-active? custom-ui-param-plock-default
   custom-ui-param-change-callback custom-ui-param-change-callback-s
   custom-ui-tensor-bound-values custom-ui-tensor-cell-change-callback-s
   custom-ui-set-param-in-scope custom-ui-set-adsr-in-scope))
(import eseq.effects.custom-ui-sections :refer
  (custom-ui-select-section-in-scope custom-ui-set-active-adsr
   custom-ui-adsr-stage-active? ui-section-select-callback ui-panel-bg))
(import eseq.effects.custom-ui-controls :refer (ui-param-bound-value))
(import eseq.effects.param-controls :refer
  (custom-ui-option-index param-plock-color-r param-plock-color-g
   param-plock-color-b))

;; Migration aliases (module spec §10). Every alias is an identity alias:
;; this file is the generated-custom-UI *layout* vocabulary (hub-file
;; precedent — the flat spellings are the contract generated code speaks).
;; Callers: per-instrument generated ui.lisp files
;; (crates/sequencer/instruments/**/ui.lisp), on-disk effect UIs
;; (crates/sequencer/effects/*/ui.lisp), Rust-generated lisp
;; (src/ui/custom_ui.rs emits calls into implicit-module units), the
;; agent-validated vocabulary (src/agent/ui_validate.rs stubs +
;; prompts/{instrument,effect}.md), unconverted effects/modulator-panel.lisp
;; (ui-accent-orange), and Rust tests that eval flat spellings
;; (src/ui/state_values/tests.rs, which also def-stubs many of these names
;; in one-shot preludes — aliases apply to writes, so the stubs keep
;; working). Bare callers cannot see qualified names, so the spellings stay
;; put. `%`-private helpers get none; ui-lego-badge-dark's only caller is
;; the converted eseq.effects.instrument-modulation (imports us), so it is
;; public but needs no alias.
(module-compat-alias ui-accent-blue ui-accent-blue)
(module-compat-alias ui-accent-cyan ui-accent-cyan)
(module-compat-alias ui-accent-orange ui-accent-orange)
(module-compat-alias ui-accent-green ui-accent-green)
(module-compat-alias ui-accent-violet ui-accent-violet)
(module-compat-alias ui-lego-gap ui-lego-gap)
(module-compat-alias ui-lego-small-h ui-lego-small-h)
(module-compat-alias ui-lego-medium-h ui-lego-medium-h)
(module-compat-alias ui-lego-dense-h ui-lego-dense-h)
(module-compat-alias ui-lego-full-h ui-lego-full-h)
(module-compat-alias ui-lego-col-w ui-lego-col-w)
(module-compat-alias ui-lego-strip-w ui-lego-strip-w)
(module-compat-alias ui-lego-text-row-3 ui-lego-text-row-3)
(module-compat-alias ui-lego-text-row-4 ui-lego-text-row-4)
(module-compat-alias ui-control-block-small ui-control-block-small)
(module-compat-alias ui-control-block-medium ui-control-block-medium)
(module-compat-alias ui-control-block-full ui-control-block-full)
(module-compat-alias ui-control-block-small-s ui-control-block-small-s)
(module-compat-alias ui-control-block-medium-s ui-control-block-medium-s)
(module-compat-alias ui-control-block-small-wide-s ui-control-block-small-wide-s)
(module-compat-alias ui-control-block-medium-wide-s ui-control-block-medium-wide-s)
(module-compat-alias ui-control-block-dense-s ui-control-block-dense-s)
(module-compat-alias ui-control-panel-dense-s ui-control-panel-dense-s)
(module-compat-alias ui-control-panel-small-s ui-control-panel-small-s)
(module-compat-alias ui-control-panel-medium-s ui-control-panel-medium-s)
(module-compat-alias ui-control-block-full-s ui-control-block-full-s)
(module-compat-alias ui-readout-block-small ui-readout-block-small)
(module-compat-alias ui-readout-block-small-s ui-readout-block-small-s)
(module-compat-alias ui-readout-block-small-wide-s ui-readout-block-small-wide-s)
(module-compat-alias ui-readout-block-dense-s ui-readout-block-dense-s)
(module-compat-alias ui-readout-panel-small-s ui-readout-panel-small-s)
(module-compat-alias ui-readout-panel-dense-s ui-readout-panel-dense-s)
(module-compat-alias ui-readout-panel-medium-s ui-readout-panel-medium-s)
(module-compat-alias ui-readout-block-medium ui-readout-block-medium)
(module-compat-alias ui-readout-block-full ui-readout-block-full)
(module-compat-alias ui-lego-column ui-lego-column)
(module-compat-alias ui-lego-column-2 ui-lego-column-2)
(module-compat-alias ui-lego-column-full ui-lego-column-full)
(module-compat-alias ui-lego-column-wide ui-lego-column-wide)
(module-compat-alias ui-lego-column-wide-2 ui-lego-column-wide-2)
(module-compat-alias ui-lego-column-wide-full ui-lego-column-wide-full)
(module-compat-alias ui-lego-strip-s ui-lego-strip-s)
(module-compat-alias ui-lego-strip-half-s ui-lego-strip-half-s)
(module-compat-alias ui-lego-strip-panel-s ui-lego-strip-panel-s)
(module-compat-alias ui-lego-badge ui-lego-badge)
(module-compat-alias ui-lego-badge-s ui-lego-badge-s)
(module-compat-alias ui-lego-knob ui-lego-knob)
(module-compat-alias ui-lego-knob-s ui-lego-knob-s)
(module-compat-alias ui-lego-big-knob-s ui-lego-big-knob-s)
(module-compat-alias ui-lego-num ui-lego-num)
(module-compat-alias ui-lego-num-s ui-lego-num-s)
(module-compat-alias ui-lego-micro-num-s ui-lego-micro-num-s)
(module-compat-alias ui-lego-matrix-s ui-lego-matrix-s)
(module-compat-alias ui-lego-option ui-lego-option)
(module-compat-alias ui-lego-option-s ui-lego-option-s)
(module-compat-alias ui-lego-micro-option-s ui-lego-micro-option-s)
(module-compat-alias ui-lego-micro-toggle-s ui-lego-micro-toggle-s)
(module-compat-alias ui-lego-micro-base-note-s ui-lego-micro-base-note-s)
(module-compat-alias ui-lego-row ui-lego-row)
(module-compat-alias ui-lego-base-note ui-lego-base-note)
(module-compat-alias ui-adsr-number-s ui-adsr-number-s)
(module-compat-alias ui-lego-adsr-s ui-lego-adsr-s)
(module-compat-alias ui-adsr ui-adsr)
(module-compat-alias ui-adsr-switch ui-adsr-switch)
(module-compat-alias ui-detail-adsr-s ui-detail-adsr-s)
(module-compat-alias ui-detail-adsr-switch-s ui-detail-adsr-switch-s)
(module-compat-alias ui-detail-adsr-divider ui-detail-adsr-divider)
(module-compat-alias ui-lego-underline-tab ui-lego-underline-tab)
(module-compat-alias ui-detail-adsr-wide-switch-s ui-detail-adsr-wide-switch-s)
(module-compat-alias ui-adsr-compact-s ui-adsr-compact-s)
(module-compat-alias ui-adsr-compact-switch-s ui-adsr-compact-switch-s)
(module-compat-alias ui-rack ui-rack)
(module-compat-alias ui-adsr-c ui-adsr-c)
(module-compat-alias ui-adsr-switch-c ui-adsr-switch-c)
(module-compat-alias ui-lego-panel-x-s ui-lego-panel-x-s)
(module-compat-alias ui-lego-tab-s ui-lego-tab-s)
(module-compat-alias ui-lego-header-s ui-lego-header-s)
(module-compat-alias ui-lego-vfader-s ui-lego-vfader-s)
(module-compat-alias ui-lego-fader-s ui-lego-fader-s)
(module-compat-alias ui-lego-sel-surface ui-lego-sel-surface)
(module-compat-alias ui-lego-mode-tab-s ui-lego-mode-tab-s)
(module-compat-alias ui-detail-adsr-body-x-s ui-detail-adsr-body-x-s)
(module-compat-alias ui-lego-divider ui-lego-divider)

(def ui-accent-blue () (rgba 0.00 0.48 0.95 1.0))
(def ui-accent-cyan () (rgba 0.05 0.78 0.90 1.0))
(def ui-accent-orange () (rgba 1.0 0.62 0.25 1.0))
(def ui-accent-green () (rgba 0.30 0.82 0.48 1.0))
(def ui-accent-violet () (rgba 0.62 0.45 0.95 1.0))

(def ui-lego-gap () 0.06125)
(def ui-lego-small-h () 1.95)
(def ui-lego-medium-h () 4.08)
(def %ui-lego-large-h () 5.58)
(def ui-lego-dense-h () 3.8)
(def ui-lego-full-h ()
  (+ (ui-lego-medium-h) (ui-lego-small-h) (ui-lego-small-h)
     (ui-lego-gap) (ui-lego-gap)))
(def ui-lego-col-w () 24.0)
(def %ui-lego-wide-col-w () 30.0)
(def ui-lego-strip-w () 7.2)

(def %ui-lego-title (title accent)
  (box :width :fill :height 0.48 :h-align :start :v-align :center :padding 0.08
    (label title :font-size 8.6 :color :dim :bg :transparent)))

(def %ui-lego-surface (title height accent surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color surface
       :corner-radius 7
       :border-width 1
       :padding 0.24
    (v-stack :width :fill :height :fill :gap 0.18
      (%ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.12 body))))

(def %ui-lego-surface-s (title height accent section surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 24
       :border-width 1
       :padding 0.24
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :height :fill :gap 0.18
      (%ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.12 body))))

(def %ui-lego-surface-width-s (title width height accent section surface body)
  (box :width width :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.24
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :height :fill :gap 0.18
      (%ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.12 body))))

(def %ui-lego-panel-s (height section surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 16
       :border-width 1
       :padding 0.18
       :on-click (ui-section-select-callback section)
    (box :width :fill :height :fill :padding 0.04 body)))

(def %ui-lego-panel-width-s (width height section surface body)
  (box :width width :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.18
       :on-click (ui-section-select-callback section)
    (box :width :fill :height :fill :padding 0.04 body)))

(def %ui-lego-plain-surface (height surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color surface
       :corner-radius 7
       :border-width 1
       :padding 0.16
       :debug-name "ui-lego-plain-surface"
       :v-align :center
    (box :width :fill :padding 0.12
      (h-stack :width :fill :gap 0 :align :center
        (box :width 0.55 :height 0.1)
        body))))

(def %ui-lego-plain-surface-s (height section surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color surface
       :corner-radius 24
       :border-width 1
       :padding 0.16
       :debug-name "ui-lego-plain-surface"
       :v-align :center
       :on-click (ui-section-select-callback section)
    (box :width :fill :padding 0.12
      (h-stack :width :fill :gap 0 :align :center
        (box :width 0.55 :height 0.1)
        body))))

(def %ui-lego-plain-surface-width-s (width height section surface body)
  (box :width width :height height
       :background-color surface
       :corner-radius 7
       :border-width 1
       :padding 0.16
       :debug-name "ui-lego-plain-surface"
       :v-align :center
       :on-click (ui-section-select-callback section)
    (box :width :fill :padding 0.12
      (h-stack :width :fill :gap 0 :align :center
        (box :width 0.55 :height 0.1)
        body))))

(def ui-lego-text-row-3 (a b c)
  (box :width :fill :height 1.28 :v-align :start :debug-name "ui-lego-text-row"
    (h-stack :gap 0.34 :align :start a b c)))

(def ui-lego-text-row-4 (a b c d)
  (box :width :fill :height 1.28 :v-align :start :debug-name "ui-lego-text-row"
    (h-stack :gap 0.34 :align :start a b c d)))

(def ui-control-block-small (title accent body)
  (%ui-lego-surface title (ui-lego-small-h) accent :instrument-group-bg body))

(def ui-control-block-medium (title accent body)
  (%ui-lego-surface title (ui-lego-medium-h) accent :instrument-group-bg body))

(def ui-control-block-full (title accent body)
  (%ui-lego-surface title (ui-lego-full-h) accent :instrument-group-bg body))

(def ui-control-block-small-s (title accent section body)
  (%ui-lego-surface-s title (ui-lego-small-h) accent section :instrument-group-bg body))

(def ui-control-block-medium-s (title accent section body)
  (%ui-lego-surface-s title (ui-lego-medium-h) accent section :instrument-group-bg body))

(def ui-control-block-small-wide-s (title accent section body)
  (%ui-lego-surface-width-s title (%ui-lego-wide-col-w) (ui-lego-small-h) accent section :instrument-group-bg body))

(def ui-control-block-medium-wide-s (title accent section body)
  (%ui-lego-surface-width-s title (%ui-lego-wide-col-w) (ui-lego-medium-h) accent section :instrument-group-bg body))

(def ui-control-block-dense-s (title accent section body)
  (%ui-lego-surface-s title (ui-lego-dense-h) accent section :instrument-group-bg body))

(def ui-control-panel-dense-s (section body)
  (%ui-lego-panel-s (ui-lego-dense-h) section :instrument-group-bg body))

(def ui-control-panel-small-s (section body)
  (%ui-lego-panel-s (ui-lego-small-h) section :instrument-group-bg body))

(def ui-control-panel-medium-s (section body)
  (%ui-lego-panel-s (ui-lego-medium-h) section :instrument-group-bg body))

(def ui-control-block-full-s (title accent section body)
  (%ui-lego-surface-s title (ui-lego-full-h) accent section :instrument-group-bg body))

(def ui-readout-block-small (title accent body)
  (%ui-lego-plain-surface (ui-lego-small-h) (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-small-s (title accent section body)
  (%ui-lego-plain-surface-s (ui-lego-small-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-small-wide-s (title accent section body)
  (%ui-lego-plain-surface-width-s (%ui-lego-wide-col-w) (ui-lego-small-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-dense-s (title accent section body)
  (%ui-lego-surface-s title (ui-lego-dense-h) accent section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-small-s (section body)
  (%ui-lego-panel-s (ui-lego-small-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-dense-s (section body)
  (%ui-lego-panel-s (ui-lego-dense-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-medium-s (section body)
  (%ui-lego-panel-s (%ui-lego-large-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-medium (title accent body)
  (%ui-lego-surface title (ui-lego-medium-h) accent (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-full (title accent body)
  (%ui-lego-surface title (ui-lego-full-h) accent (rgba 0.055 0.058 0.064 1.0) body))

(def ui-lego-column (a b c)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a b c))

(def ui-lego-column-2 (a b)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a b))

(def ui-lego-column-full (a)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a))

(def ui-lego-column-wide (a b c)
  (v-stack :width (%ui-lego-wide-col-w) :gap (ui-lego-gap) a b c))

(def ui-lego-column-wide-2 (a b)
  (v-stack :width (%ui-lego-wide-col-w) :gap (ui-lego-gap) a b))

(def ui-lego-column-wide-full (a)
  (v-stack :width (%ui-lego-wide-col-w) :gap (ui-lego-gap) a))

(def ui-lego-strip-s (title accent section body)
  (%ui-lego-surface-width-s title (ui-lego-strip-w) (ui-lego-full-h) accent section :instrument-group-bg body))

(def ui-lego-strip-half-s (title accent section body)
  (%ui-lego-surface-width-s title (ui-lego-strip-w) (ui-lego-medium-h) accent section :instrument-group-bg body))

(def ui-lego-strip-panel-s (section body)
  (%ui-lego-panel-width-s (ui-lego-strip-w) (ui-lego-full-h) section :instrument-group-bg body))

(def ui-lego-badge (title width accent)
  (box :width width :height 1.18 :v-align :end
    (badge title
      :width width :height 0.82 :padding 0 :font-size 9.2
      :variant :secondary
      :color accent)))

(def ui-lego-badge-s (section title width accent)
  (box :width width :height 1.18 :v-align :end
    (badge title
      :width width :height 0.82 :padding 0 :font-size 9.2
      :border-color :transparent
      :variant :secondary
      :color accent)))

(def ui-lego-badge-dark (title width accent)
  (box :width width :height 1.18 :v-align :end
    (badge title
      :width width :height 0.82 :padding 0 :font-size 9.2
      :background-color :instrument-control-bg
      :color accent)))

(def ui-lego-knob (name title width accent decimals)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-knob-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-knob-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (custom-ui-param-binding p)
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
            :base-value (custom-ui-param-base-value-prop p)
            :base-min (custom-ui-param-base-min-prop p) :base-max (custom-ui-param-base-max-prop p)
            :mod-range-0-slot (custom-ui-param-knob-mod-slot-prop p 0) :mod-range-0-depth (custom-ui-param-knob-mod-depth-prop p 0)
            :mod-range-1-slot (custom-ui-param-knob-mod-slot-prop p 1) :mod-range-1-depth (custom-ui-param-knob-mod-depth-prop p 1)
            :mod-range-2-slot (custom-ui-param-knob-mod-slot-prop p 2) :mod-range-2-depth (custom-ui-param-knob-mod-depth-prop p 2)
            :mod-range-3-slot (custom-ui-param-knob-mod-slot-prop p 3) :mod-range-3-depth (custom-ui-param-knob-mod-depth-prop p 3)
            :mod-range-4-slot (custom-ui-param-knob-mod-slot-prop p 4) :mod-range-4-depth (custom-ui-param-knob-mod-depth-prop p 4)
            :mod-range-5-slot (custom-ui-param-knob-mod-slot-prop p 5) :mod-range-5-depth (custom-ui-param-knob-mod-depth-prop p 5)
            :mod-range-6-slot (custom-ui-param-knob-mod-slot-prop p 6) :mod-range-6-depth (custom-ui-param-knob-mod-depth-prop p 6)
            :mod-range-7-slot (custom-ui-param-knob-mod-slot-prop p 7) :mod-range-7-depth (custom-ui-param-knob-mod-depth-prop p 7)
            :mod-range-8-slot (custom-ui-param-knob-mod-slot-prop p 8) :mod-range-8-depth (custom-ui-param-knob-mod-depth-prop p 8)
            :mod-range-9-slot (custom-ui-param-knob-mod-slot-prop p 9) :mod-range-9-depth (custom-ui-param-knob-mod-depth-prop p 9)
            :selected-mod-slot (custom-ui-selected-mod-slot-prop p)
            :font-size 10.8 :label-font-size 9.6
            :text-color (custom-ui-param-plock-text-color p) :label-color :dim
            :plock-active (if (custom-ui-param-plock-active? p) 1 0)
            :plock-default (custom-ui-param-plock-default p)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
	    :track-color '(rgba 0.4, 0.4, 0.4, 1)
            :width width :height 2.82
            :value-align :center
            :on-change (custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def %ui-lego-knob-sized-s (section name title width height knob-size accent decimals)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-knob-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-knob-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (custom-ui-param-binding p)
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
            :base-value (custom-ui-param-base-value-prop p)
            :base-min (custom-ui-param-base-min-prop p) :base-max (custom-ui-param-base-max-prop p)
            :mod-range-0-slot (custom-ui-param-knob-mod-slot-prop p 0) :mod-range-0-depth (custom-ui-param-knob-mod-depth-prop p 0)
            :mod-range-1-slot (custom-ui-param-knob-mod-slot-prop p 1) :mod-range-1-depth (custom-ui-param-knob-mod-depth-prop p 1)
            :mod-range-2-slot (custom-ui-param-knob-mod-slot-prop p 2) :mod-range-2-depth (custom-ui-param-knob-mod-depth-prop p 2)
            :mod-range-3-slot (custom-ui-param-knob-mod-slot-prop p 3) :mod-range-3-depth (custom-ui-param-knob-mod-depth-prop p 3)
            :mod-range-4-slot (custom-ui-param-knob-mod-slot-prop p 4) :mod-range-4-depth (custom-ui-param-knob-mod-depth-prop p 4)
            :mod-range-5-slot (custom-ui-param-knob-mod-slot-prop p 5) :mod-range-5-depth (custom-ui-param-knob-mod-depth-prop p 5)
            :mod-range-6-slot (custom-ui-param-knob-mod-slot-prop p 6) :mod-range-6-depth (custom-ui-param-knob-mod-depth-prop p 6)
            :mod-range-7-slot (custom-ui-param-knob-mod-slot-prop p 7) :mod-range-7-depth (custom-ui-param-knob-mod-depth-prop p 7)
            :mod-range-8-slot (custom-ui-param-knob-mod-slot-prop p 8) :mod-range-8-depth (custom-ui-param-knob-mod-depth-prop p 8)
            :mod-range-9-slot (custom-ui-param-knob-mod-slot-prop p 9) :mod-range-9-depth (custom-ui-param-knob-mod-depth-prop p 9)
            :selected-mod-slot (custom-ui-selected-mod-slot-prop p)
            :font-size 10.8 :label-font-size 9.6
            :text-color (custom-ui-param-plock-text-color p) :label-color :dim
            :plock-active (if (custom-ui-param-plock-active? p) 1 0)
            :plock-default (custom-ui-param-plock-default p)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
	    :track-color '(rgba 0.4, 0.4, 0.4, 1)
	    :width width :height height :knob-size knob-size
            :value-align :center
	    :arc-color accent
            :on-change (custom-ui-param-change-callback-s section p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-knob-s (section name title width accent decimals)
  (%ui-lego-knob-sized-s section name title width 3.12 3.12 accent decimals))

(def ui-lego-big-knob-s (section name title width accent decimals)
  (%ui-lego-knob-sized-s section name title width 4.30 2.65 accent decimals))

(def ui-lego-num (name title width decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-num-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-num-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (number-picker :value (custom-ui-param-binding p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :text-align :left
              :width width :height 0.68
              :on-change (custom-ui-param-change-callback p)))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-num-s (section name title width decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-num-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-num-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (number-picker :value (custom-ui-param-binding p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :text-align :left
              :width width :height 0.68
              :on-change (custom-ui-param-change-callback-s section p)))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def %ui-lego-micro-num-stage-s (section stage name title width decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-micro-num-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-micro-num-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (v-stack :width width :height 1.0 :gap 0.06 :align :start
            (label title :font-size 7.4 :width width :height 0.68 :color :dim :bg :transparent)
            (number-picker :value (custom-ui-param-binding p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 9.0
              :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
              :active (custom-ui-adsr-stage-active? section stage)
              :active-color (ui-accent-cyan)
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :text-align :left
              :width width :height 0.50
              :on-change (custom-ui-param-change-callback-s section p)))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ui-lego-micro-num-s (section name title width decimals unit accent)
  (%ui-lego-micro-num-stage-s section false name title width decimals unit accent))

(def ui-lego-matrix-s (section name title width height accent)
  (let ((p (custom-ui-current-tensor-param name)))
    (if p
      (subtree :key (str "custom-ui-lego-matrix-" (custom-ui-scope-name) "-" name)
        (v-stack :width width :height height :gap 0.10 :align :start
          (label title :font-size 8.2 :width width :height 0.50 :color accent :bg :transparent)
          (matrix :rows (get p :rows) :cols (get p :cols)
            :value (custom-ui-tensor-bound-values p)
            :min (get p :min) :max (get p :max)
            :control :grid
            :width width :height (- height 0.60)
            :on-cell-change (custom-ui-tensor-cell-change-callback-s section p))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ui-lego-option (name title width options accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-option-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-option-" (custom-ui-scope-name) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (dropdown :value-index (custom-ui-param-binding p)
              :value-index-offset (get p :min)
              :options options
              :bg-color :instrument-control-bg
              :text-color accent
              :chevron-color accent
              :badge-color (rgba 0.16 0.17 0.20 1.0)
              :border-color accent
              :border-width 0.05
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :width width :height 0.78 :font-size 8.0
              :on-change (lambda (v)
                (custom-ui-set-param-in-scope
                  scope
                  p
                  (+ (get p :min) (custom-ui-option-index options v))))))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-option-s (section name title width options accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-option-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-option-" (custom-ui-scope-name) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (dropdown :value-index (custom-ui-param-binding p)
              :value-index-offset (get p :min)
              :options options
              :bg-color :instrument-control-bg
              :text-color accent
              :chevron-color accent
              :badge-color (rgba 0.16 0.17 0.20 1.0)
              :border-color accent
              :border-width 0.05
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :width width :height 0.78 :font-size 8.0
              :on-change (lambda (v)
                (do
                  (custom-ui-select-section-in-scope scope section)
                  (custom-ui-set-param-in-scope
                    scope
                    p
                    (+ (get p :min) (custom-ui-option-index options v)))))))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-micro-option-s (section name title width options accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-micro-option-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-micro-option-" (custom-ui-scope-name) "-" name)
          (box :width width :height 1.18 :v-align :end
            (dropdown :value-index (custom-ui-param-binding p)
              :value-index-offset (get p :min)
              :options options
              :bg-color '(rgba 0.1 0.1 0.1 1) ;:instrument-control-bg
              :text-color accent
              :chevron-color accent
              :badge-color (rgba 0.16 0.17 0.20 1.0)
              :border-color (rgba 1.0 1.0 1.0 0.2) 
              :border-width 0.05
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :width width :height 0.92 :font-size 8.6
              :on-change (lambda (v)
                (do
                  (custom-ui-select-section-in-scope scope section)
                  (custom-ui-set-param-in-scope
                    scope
                    p
                    (+ (get p :min) (custom-ui-option-index options v)))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ui-lego-micro-toggle-s (section name width accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (let ((on (> (reactive-value (custom-ui-param-value p)) 0.5)))
        (custom-ui-param-mod-wrapper p
          (str "custom-ui-lego-micro-toggle-mod-" (custom-ui-scope-name) "-" name)
          (subtree :key (str "custom-ui-lego-micro-toggle-" (custom-ui-scope-name)
                             "-" (if on 1 0) "-" name)
            (box :debug-name (str "custom-ui-lego-micro-toggle-" name)
                 :width width :height 1.18 :v-align :end
              (toggle
                :value on
                :color accent
                :off-color :instrument-control-bg
                :knob-color :black
                :off-knob-color :dim
                :on-change (lambda (next-on)
                  (do
                    (custom-ui-select-section-in-scope scope section)
                    (custom-ui-set-param-in-scope scope p (if next-on 1 0)))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ui-lego-micro-base-note-s (section width accent)
  (let ((p (custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-lego-micro-base-note-" (custom-ui-scope-name))
        (v-stack :width width :height 1.18 :gap 0.16 :align :start
          (label "note" :font-size 7.4 :width width :height 0.52 :color :dim :bg :transparent)
          (number-picker :value (custom-ui-param-binding p)
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals 0
            :step 1
            :noui true :font-size 9.0
            :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
            :plock-active (if (custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
            :text-align :left
            :width width :height 0.50
            :on-change (custom-ui-param-change-callback p))))
      (label "missing: base_note" :font-size 8 :color :red :bg :transparent))))

(def ui-lego-row (name title decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-row-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-row-" (custom-ui-scope-name) "-" name)
          (h-stack :width :fill :height 0.86 :gap 0.35 :align :baseline
            (label title :font-size 8.8 :width 6.2 :color :dim :bg :transparent)
            (number-picker :value (custom-ui-param-binding p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-align :left
              :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :width 6.0 :height 0.78
              :on-change (custom-ui-param-change-callback p)))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-base-note (width accent)
  (let ((p (custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-lego-base-note-" (custom-ui-scope-name))
        (v-stack :width width :height 1.12 :gap 0.08 :align :start
          (label "note" :font-size 8.2 :width width :color :dim :bg :transparent)
          (number-picker :value (custom-ui-param-binding p)
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals 0
            :step 1
            :noui true :font-size 10.2
            :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
            :plock-active (if (custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
            :text-align :left
            :width width :height 0.68
            :on-change (custom-ui-param-change-callback p))))
      (label "missing: base_note" :font-size 9 :color :red :bg :transparent))))

(def %ui-adsr-number (stage name title decimals unit)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-adsr-number-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-adsr-number-" (custom-ui-scope-name) "-" name)
          (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
            (label title :font-size 10 :color :dim :bg :transparent)
            (number-picker :value (custom-ui-param-binding p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.5
              :text-align :center
              :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
              :active (custom-ui-adsr-stage-active? -1 stage)
              :active-color (ui-accent-cyan)
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :width 5.0 :height 0.95
              :on-change (custom-ui-param-change-callback p)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-adsr-number-s (section stage name title decimals unit)
  (if name
    (let ((p (custom-ui-current-param name)))
      (if p
        (custom-ui-param-mod-wrapper p (str "custom-ui-adsr-number-mod-" (custom-ui-scope-name) "-" name)
          (subtree :key (str "custom-ui-adsr-number-" (custom-ui-scope-name) "-" name)
            (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
              (label title :font-size 10 :color :dim :bg :transparent)
              (number-picker :value (custom-ui-param-binding p)
                :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
                :unit unit
                :noui true :font-size 10.5
                :text-align :center
                :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
                :active (custom-ui-adsr-stage-active? section stage)
                :active-color (ui-accent-cyan)
                :plock-active (if (custom-ui-param-plock-active? p) 1 0)
                :plock-color-r (param-plock-color-r)
                :plock-color-g (param-plock-color-g)
                :plock-color-b (param-plock-color-b)
                :width 5.0 :height 0.95
                :on-change (custom-ui-param-change-callback-s section p)))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :dim :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit
          :noui true :font-size 10.5
          :text-align :center
          :text-color :dim :edit-color :dim
          :width 5.0 :height 0.95)))))

(def ui-lego-adsr-s (section title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
  (box :width (ui-lego-col-w) :height (ui-lego-full-h)
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 12 :padding 0.15
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :height :fill :gap 0.10
      (box :width :fill :height 0.35 :h-align :start :v-align :center
        (label title :font-size 8.5 :color :dim :bg :transparent))
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (if release (ui-param-bound-value release 120) 0)
        :width :fill :flex 1
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-select-section-in-scope scope section)
            (custom-ui-set-active-adsr scope section (get env :active))
            (custom-ui-set-adsr-in-scope scope attack decay sustain release env))))
      (box :width :fill :height 1.75 :padding 0.15
        (h-stack :width :fill :gap 0.20 :align :start
          (ui-adsr-number-s section :attack attack "atk" 0 "ms")
          (ui-adsr-number-s section :decay decay "dec" 0 "ms")
          (ui-adsr-number-s section :sustain sustain "sus" 2 false)
          (ui-adsr-number-s section :release release "rel" 0 "ms")))))))

(def ui-adsr (title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
  (box :width 23.1 :height :fill
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 12 :padding 0.15
    (v-stack :width :fill :height :fill :gap 0.10
      (box :width :fill :height 0.35 :h-align :start :v-align :center
        (label title :font-size 8.5 :color :dim :bg :transparent))
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (ui-param-bound-value release 120)
        :width :fill :flex 1
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-set-active-adsr scope -1 (get env :active))
            (custom-ui-set-adsr-in-scope scope attack decay sustain release env))))
      (box :width :fill :height 1.75 :padding 0.15
        (h-stack :width :fill :gap 0.20 :align :start
          (%ui-adsr-number :attack attack "atk" 0 "ms")
          (%ui-adsr-number :decay decay "dec" 0 "ms")
          (%ui-adsr-number :sustain sustain "sus" 2 false)
          (%ui-adsr-number :release release "rel" 0 "ms")))))))

(def ui-adsr-switch (section-a title-a attack-a decay-a sustain-a release-a
                     section-b title-b attack-b decay-b sustain-b release-b)
  (if (= eseq.vanilla/custom-ui-selected-section section-b)
    (ui-adsr title-b attack-b decay-b sustain-b release-b)
    (ui-adsr title-a attack-a decay-a sustain-a release-a)))

(def ui-detail-adsr-s (section title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
    (ui-readout-panel-medium-s section
      (v-stack :width :fill :height :fill :gap 0.28 :align :stretch
        (box :width :fill :height 0.34 :h-align :start :v-align :center
          (h-stack (box :width 0.5)
            (label title :font-size 7.8 :color :dim :bg :transparent))
          )
        (adsr-editor
          :attack (ui-param-bound-value attack 5)
          :decay (ui-param-bound-value decay 120)
          :sustain (ui-param-bound-value sustain 0.7)
          :release (ui-param-bound-value release 120)
          :width :fill :height 3.08
          :background-color :instrument-control-bg
          :on-change (lambda (env)
            (do
              (custom-ui-select-section-in-scope scope section)
              (custom-ui-set-active-adsr scope section (get env :active))
              (custom-ui-set-adsr-in-scope scope attack decay sustain release env))))
        (h-stack :width :fill :height 1.0 :gap 0.24 :align :start
          (box :width 1)
          (%ui-lego-micro-num-stage-s section :attack attack "atk" 5.1 0 "ms" (ui-accent-cyan))
          (%ui-lego-micro-num-stage-s section :decay decay "dec" 5.1 0 "ms" (ui-accent-cyan))
          (%ui-lego-micro-num-stage-s section :sustain sustain "sus" 5.1 2 false (ui-accent-cyan))
          (%ui-lego-micro-num-stage-s section :release release "rel" 5.1 0 "ms" (ui-accent-cyan)))))))

(def ui-detail-adsr-switch-s (section-a title-a attack-a decay-a sustain-a release-a
                              section-b title-b attack-b decay-b sustain-b release-b)
  (if (= eseq.vanilla/custom-ui-selected-section section-b)
    (ui-detail-adsr-s section-b title-b attack-b decay-b sustain-b release-b)
    (ui-detail-adsr-s section-a title-a attack-a decay-a sustain-a release-a)))

;; Full-height envelope surface for instruments that dedicate a complete
;; horizontal slice to envelope editing. The plot owns all space not reserved
;; for the compact title and the single A/D/S/R readout row.
(def ui-detail-adsr-divider (debug-name)
  (box :width :fill :height 0.05
       :background-color (rgba 1.0 1.0 1.0 0.14)
       :debug-name debug-name))

(def %ui-detail-adsr-wide-content-s (section attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
    (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (ui-param-bound-value release 120)
        :width :fill :flex 1
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-select-section-in-scope scope section)
            (custom-ui-set-active-adsr scope section (get env :active))
            (custom-ui-set-adsr-in-scope scope attack decay sustain release env))))
      (ui-detail-adsr-divider "adsr-controls-divider")
      (box :width :fill :height 1.75 :h-align :center :v-align :center
        (h-stack :gap 0.42 :align :start
          (ui-adsr-number-s section :attack attack "atk" 0 "ms")
          (ui-adsr-number-s section :decay decay "dec" 0 "ms")
          (ui-adsr-number-s section :sustain sustain "sus" 2 false)
          (ui-adsr-number-s section :release release "rel" 0 "ms"))))))

(def ui-lego-underline-tab (title width selected accent on-click debug-name)
  (v-stack :width width :height 1.02 :gap 0.0 :align :stretch
    (box :width :fill :height 0.96 :h-align :center :v-align :center
         :debug-name debug-name :on-click on-click
      (label title :font-size 9.2
        :color (if selected accent (rgba 0.72 0.66 0.55 1.0))
        :bg :transparent))
    (box :width :fill :height 0.06
      :background-color (if selected accent :transparent))))

(def %ui-detail-adsr-wide-tab-s (section title width selected)
  (ui-lego-underline-tab
    title width selected (ui-accent-cyan)
    (ui-section-select-callback section)
    (str "adsr-tab-" title)))

(def %ui-detail-adsr-wide-s (width height section title attack decay sustain release)
  (%ui-lego-panel-width-s width height section (rgba 0.055 0.058 0.064 1.0)
    (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
      (box :width :fill :height 1.02 :h-align :start :v-align :center
        (label title :font-size 9.2 :color (ui-accent-cyan) :bg :transparent))
      (ui-detail-adsr-divider "adsr-header-divider")
      (box :width :fill :flex 1
        (%ui-detail-adsr-wide-content-s section attack decay sustain release)))))

(def ui-detail-adsr-wide-switch-s
  (width height
   section-a title-a attack-a decay-a sustain-a release-a
   section-b title-b attack-b decay-b sustain-b release-b)
  (let ((show-b (= eseq.vanilla/custom-ui-selected-section section-b))
        (tab-width (/ (- width 1.0) 2.0)))
    (box :width width :height height
         :background-color (rgba 0.055 0.058 0.064 1.0)
         :corner-radius 7 :border-width 1 :padding 0.18
      (box :width :fill :height :fill :padding 0.04
        (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
          (h-stack :width :fill :height 1.02 :gap 0.0 :align :stretch
            (%ui-detail-adsr-wide-tab-s section-a title-a tab-width (not show-b))
            (%ui-detail-adsr-wide-tab-s section-b title-b tab-width show-b))
          (ui-detail-adsr-divider "adsr-tabs-divider")
          (box :width :fill :flex 1
            (if show-b
              (%ui-detail-adsr-wide-content-s section-b attack-b decay-b sustain-b release-b)
              (%ui-detail-adsr-wide-content-s section-a attack-a decay-a sustain-a release-a))))))))

(def ui-adsr-compact-s (section title attack decay sustain release)
  (ui-detail-adsr-s section title attack decay sustain release))

(def ui-adsr-compact-switch-s (section-a title-a attack-a decay-a sustain-a release-a
                               section-b title-b attack-b decay-b sustain-b release-b)
  (ui-detail-adsr-switch-s
    section-a title-a attack-a decay-a sustain-a release-a
    section-b title-b attack-b decay-b sustain-b release-b))

;; ui-rack — auto-arrange a flat list of panels into columns based on mode.
;;   mode          :breathe (2 panels per column) or :compact (4 panels per col)
;;   left-panels   ordered list of panels to place LEFT of the ADSR
;;   adsr-form     a pre-built ADSR widget (ui-adsr / ui-adsr-switch / -c variants)
;;   right-panels  ordered list of panels to place RIGHT of the ADSR
;;
;; The instrument doesn't have to know how many fit per column — just list
;; panels in order, pick :breathe or :compact, and the helper chunks them.
(def %ui-rack-col-breathe (col)
  (v-stack :width 31.0 :gap 0.10 col))
(def %ui-rack-col-compact (col)
  (v-stack :width 20.0 :gap 0.08 col))
(def ui-rack (mode left-panels adsr-form right-panels)
  (if (= mode :compact)
    (h-stack :width :fill :gap 0.35 :align :stretch
      (map %ui-rack-col-compact (chunks left-panels 4))
      adsr-form
      (map %ui-rack-col-compact (chunks right-panels 4)))
    (h-stack :width :fill :gap 0.4 :align :stretch
      (map %ui-rack-col-breathe (chunks left-panels 2))
      adsr-form
      (map %ui-rack-col-breathe (chunks right-panels 2)))))

;; Compact ADSR for use alongside ui-panel-c. Fills the available height —
;; the outer h-stack must use `:align :stretch` so the box stretches to the
;; tallest sibling column. ADSR-editor takes the remaining vertical space
;; via `:flex 1`; controls + caption hold their natural height.
(def ui-adsr-c (title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
  (box :width 21.0 :height :fill
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 10 :padding 0.1
    (v-stack :width :fill :height :fill :gap 0.08
      (box :width :fill :height 0.3 :h-align :start :v-align :center
        (label title :font-size 7.5 :color :dim :bg :transparent))
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (ui-param-bound-value release 120)
        :width :fill :flex 1
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-set-active-adsr scope -1 (get env :active))
            (custom-ui-set-adsr-in-scope scope attack decay sustain release env))))
      (box :width :fill :height 1.45 :padding 0.1
        (h-stack :width :fill :gap 0.15 :align :start
          (%ui-adsr-number :attack attack "atk" 0 "ms")
          (%ui-adsr-number :decay decay "dec" 0 "ms")
          (%ui-adsr-number :sustain sustain "sus" 2 false)
          (%ui-adsr-number :release release "rel" 0 "ms")))))))

(def ui-adsr-switch-c (section-a title-a attack-a decay-a sustain-a release-a
                       section-b title-b attack-b decay-b sustain-b release-b)
  (if (= eseq.vanilla/custom-ui-selected-section section-b)
    (ui-adsr-c title-b attack-b decay-b sustain-b release-b)
    (ui-adsr-c title-a attack-a decay-a sustain-a release-a)))

;; ---------------------------------------------------------------------------
;; Character variants — parametric pieces for giving an instrument its own
;; visual identity instead of the uniform badge+dropdown look.
;; ---------------------------------------------------------------------------

;; Fully parametric panel: custom surface + border colors and an optional
;; accent stripe down the left edge. stripe-color may be false for no stripe.
(def ui-lego-panel-x-s (section width height surface border-color stripe-color body)
  (box :width width :height height
       :background-color surface
       :corner-radius 16
       :border-width 1
       :border-color border-color
       :padding 0.18
       :on-click (ui-section-select-callback section)
    (h-stack :width :fill :height :fill :gap 0.24 :align :center
      (if stripe-color
        (box :width 0.26 :height (* height 0.6) :background-color stripe-color :corner-radius 2)
        (box :width 0.02 :height 0.1))
      (box :flex 1 :height :fill :padding 0.04 body))))

;; Solid colored tab block (Ableton-style source tag like "1" / "2" / "N").
(def ui-lego-tab-s (section text width height color text-color)
  (button text :width width :height height
       :font-size 8.8 :color text-color
       :background-color color
       :corner-radius 3
       :h-align :center :v-align :center
       :on-click (ui-section-select-callback section)
   ; (label text :font-size 8.8 :color text-color :bg :transparent)
    ))

;; Accent text header with underline — alternative to ui-lego-badge-s.
(def ui-lego-header-s (section title width accent)
  (box :width width :height 1.18 :v-align :end :on-click (ui-section-select-callback section)
    (v-stack :width width :gap 0.14 :align :start
      (label title :font-size 9.2 :width width :color accent :bg :transparent)
      (box :width width :height 0.10 :background-color accent :corner-radius 1))))

;; Param-bound vertical fader.
(def ui-lego-vfader-s (section name width height accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-vfader-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-vfader-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (vslider :width 0.1 :height height
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p)
            :origin (custom-ui-param-control-min p)
            :value (custom-ui-param-binding p)
            :color :white
            :fill accent
            :dot-color (rgba 0.16 0.16 0.18 1.0)
            :plock-active (if (custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
            :on-change (custom-ui-param-change-callback-s section p))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

;; Fader cell: vertical fader with an editable value readout under it.
(def ui-lego-fader-s (section name width fader-height accent decimals unit)
  (let ((p (custom-ui-current-param name)))
    (if p
      (v-stack :width width :gap 0.10 :align :center
        (ui-lego-vfader-s section name 0.7 fader-height accent)
        (number-picker :value (custom-ui-param-binding p)
          :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
          :unit unit
          :noui true :font-size 8.6
          :text-color (custom-ui-param-plock-text-color p) :edit-color :yellow
          :plock-active (if (custom-ui-param-plock-active? p) 1 0)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :text-align :center
          :width width :height 0.46
          :on-change (custom-ui-param-change-callback-s section p)))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

;; Click-to-cycle chip: shows the current option, click advances (wraps).
;; For 2-3 option params where a dropdown is overkill.
(def %ui-lego-chip-cycle-s (section name labels width accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      ;; the value is read concretely, so it must be part of the subtree key
      ;; for the chip to rebuild when the param changes
      (let ((idx (round (- (reactive-value (custom-ui-param-value p)) (get p :min))))
            (n (length labels)))
        (subtree :key (str "custom-ui-lego-chip-cycle-" (custom-ui-scope-name) "-" name "-" idx)
          (box :width width :height 1.18 :v-align :end
            (button (nth labels idx)
              :width width :height 0.92 :padding 0 :font-size 8.8
              :background-color :instrument-control-bg
              :color accent
              :plock-active (if (custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :on-click (lambda (x y r)
                (do
                  (custom-ui-select-section-in-scope scope section)
                  (custom-ui-set-param-in-scope scope p
                    (+ (get p :min) (mod (+ idx 1) n)))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

;; ---------------------------------------------------------------------------
;; Mode-driven center-panel pieces: a section click (any panel's on-click or a
;; ui-lego-mode-tab-s) sets custom-ui-selected-section; the instrument
;; dispatches on it to swap center views.
;; ---------------------------------------------------------------------------

;; Selection-aware surface: brighter color when the section drives the center.
(def ui-lego-sel-surface (section selected-surface surface)
  (if (= eseq.vanilla/custom-ui-selected-section section) selected-surface surface))

;; Mode tab (Ableton's little filled selector box): accent-filled when its
;; section is selected, dark with accent text otherwise. Click selects.
(def ui-lego-mode-tab-s (section text width height accent)
  (ui-lego-tab-s section text width height
    (if (= eseq.vanilla/custom-ui-selected-section section) accent (rgba 0.13 0.135 0.15 1.0))
    (if (= eseq.vanilla/custom-ui-selected-section section) :black accent)))

;; Accent-tinted detail ADSR body — panel-less, for composing inside a larger
;; continuous surface (mode-driven center panels).
(def ui-detail-adsr-body-x-s (section title accent attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
      (v-stack :width :fill :height :fill :gap 0.08 :align :stretch
        (box :width :fill :height 0.34 :h-align :start :v-align :center
          (label title :font-size 7.8 :color accent :bg :transparent))
        (adsr-editor
          :attack (ui-param-bound-value attack 5)
          :decay (ui-param-bound-value decay 120)
          :sustain (ui-param-bound-value sustain 0.7)
          :release (ui-param-bound-value release 120)
          :width :fill :height 2.08
          :background-color :instrument-control-bg
          :curve-color accent
          :on-change (lambda (env)
            (do
              (custom-ui-select-section-in-scope scope section)
              (custom-ui-set-active-adsr scope section (get env :active))
              (custom-ui-set-adsr-in-scope scope attack decay sustain release env))))
        (h-stack :width :fill :height 1.0 :gap 0.24 :align :start
          (%ui-lego-micro-num-stage-s section :attack attack "atk" 5.1 0 "ms" accent)
          (%ui-lego-micro-num-stage-s section :decay decay "dec" 5.1 0 "ms" accent)
          (%ui-lego-micro-num-stage-s section :sustain sustain "sus" 5.1 2 false accent)
          (%ui-lego-micro-num-stage-s section :release release "rel" 5.1 0 "ms" accent)))))

;; Accent-tinted detail ADSR — like ui-detail-adsr-s but with a custom accent
;; so per-mode views keep their identity color.
(def %ui-detail-adsr-x-s (section title accent attack decay sustain release)
  (ui-readout-panel-medium-s section
    (ui-detail-adsr-body-x-s section title accent attack decay sustain release)))

;; Hairline divider for sectioning a continuous panel.
(def ui-lego-divider ()
  (box :width :fill :height 0.05 :background-color (rgba 1.0 1.0 1.0 0.07) :corner-radius 1))

;; On/off chip: filled with accent when on, dark when off. Click toggles.
(def %ui-lego-chip-toggle-s (section name text width accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (let ((on (> (reactive-value (custom-ui-param-value p)) 0.5)))
        (subtree :key (str "custom-ui-lego-chip-toggle-" (custom-ui-scope-name) "-" name "-" (if on 1 0))
          (box :width width :height 1.18 :v-align :end
            (button text
              :width width :height 0.92 :padding 0 :font-size 8.8
              :background-color (if on accent :instrument-control-bg)
              :color (if on :black :dim)
              :on-click (lambda (x y r)
                (do
                  (custom-ui-select-section-in-scope scope section)
                  (custom-ui-set-param-in-scope scope p (if on 0 1))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))
