;; Dense generated-UI layout primitives and rack helpers.
(def ui-accent-blue () (rgba 0.00 0.48 0.95 1.0))
(def ui-accent-cyan () (rgba 0.05 0.78 0.90 1.0))
(def ui-accent-orange () (rgba 1.0 0.62 0.25 1.0))
(def ui-accent-green () (rgba 0.30 0.82 0.48 1.0))
(def ui-accent-violet () (rgba 0.62 0.45 0.95 1.0))

(def ui-lego-gap () 0.25)
(def ui-lego-small-h () 1.95)
(def ui-lego-medium-h () 4.08)
(def ui-lego-dense-h () 3.08)
(def ui-lego-full-h ()
  (+ (ui-lego-medium-h) (ui-lego-small-h) (ui-lego-small-h)
     (ui-lego-gap) (ui-lego-gap)))
(def ui-lego-col-w () 24.0)
(def ui-lego-wide-col-w () 30.0)
(def ui-lego-strip-w () 7.2)

(def ui-lego-title (title accent)
  (box :width :fill :height 0.48 :h-align :start :v-align :center :padding 0.08
    (label title :font-size 8.6 :color :dim :bg :transparent)))

(def ui-lego-surface (title height accent surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color surface
       :corner-radius 7
       :border-width 1
       :padding 0.24
    (v-stack :width :fill :height :fill :gap 0.18
      (ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.12 body))))

(def ui-lego-surface-s (title height accent section surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.24
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :height :fill :gap 0.18
      (ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.12 body))))

(def ui-lego-surface-width-s (title width height accent section surface body)
  (box :width width :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.24
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :height :fill :gap 0.18
      (ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.12 body))))

(def ui-lego-panel-s (height section surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.18
       :on-click (ui-section-select-callback section)
    (box :width :fill :height :fill :padding 0.04 body)))

(def ui-lego-panel-width-s (width height section surface body)
  (box :width width :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.18
       :on-click (ui-section-select-callback section)
    (box :width :fill :height :fill :padding 0.04 body)))

(def ui-lego-plain-surface (height surface body)
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

(def ui-lego-plain-surface-s (height section surface body)
  (box :width (ui-lego-col-w) :height height
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

(def ui-lego-plain-surface-width-s (width height section surface body)
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
  (ui-lego-surface title (ui-lego-small-h) accent :instrument-group-bg body))

(def ui-control-block-medium (title accent body)
  (ui-lego-surface title (ui-lego-medium-h) accent :instrument-group-bg body))

(def ui-control-block-full (title accent body)
  (ui-lego-surface title (ui-lego-full-h) accent :instrument-group-bg body))

(def ui-control-block-small-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-small-h) accent section :instrument-group-bg body))

(def ui-control-block-medium-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-medium-h) accent section :instrument-group-bg body))

(def ui-control-block-small-wide-s (title accent section body)
  (ui-lego-surface-width-s title (ui-lego-wide-col-w) (ui-lego-small-h) accent section :instrument-group-bg body))

(def ui-control-block-medium-wide-s (title accent section body)
  (ui-lego-surface-width-s title (ui-lego-wide-col-w) (ui-lego-medium-h) accent section :instrument-group-bg body))

(def ui-control-block-dense-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-dense-h) accent section :instrument-group-bg body))

(def ui-control-panel-dense-s (section body)
  (ui-lego-panel-s (ui-lego-dense-h) section :instrument-group-bg body))

(def ui-control-panel-small-s (section body)
  (ui-lego-panel-s (ui-lego-small-h) section :instrument-group-bg body))

(def ui-control-panel-medium-s (section body)
  (ui-lego-panel-s (ui-lego-medium-h) section :instrument-group-bg body))

(def ui-control-block-full-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-full-h) accent section :instrument-group-bg body))

(def ui-readout-block-small (title accent body)
  (ui-lego-plain-surface (ui-lego-small-h) (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-small-s (title accent section body)
  (ui-lego-plain-surface-s (ui-lego-small-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-small-wide-s (title accent section body)
  (ui-lego-plain-surface-width-s (ui-lego-wide-col-w) (ui-lego-small-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-dense-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-dense-h) accent section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-small-s (section body)
  (ui-lego-panel-s (ui-lego-small-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-dense-s (section body)
  (ui-lego-panel-s (ui-lego-dense-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-medium-s (section body)
  (ui-lego-panel-s (ui-lego-medium-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-medium (title accent body)
  (ui-lego-surface title (ui-lego-medium-h) accent (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-full (title accent body)
  (ui-lego-surface title (ui-lego-full-h) accent (rgba 0.055 0.058 0.064 1.0) body))

(def ui-lego-column (a b c)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a b c))

(def ui-lego-column-2 (a b)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a b))

(def ui-lego-column-full (a)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a))

(def ui-lego-column-wide (a b c)
  (v-stack :width (ui-lego-wide-col-w) :gap (ui-lego-gap) a b c))

(def ui-lego-column-wide-2 (a b)
  (v-stack :width (ui-lego-wide-col-w) :gap (ui-lego-gap) a b))

(def ui-lego-column-wide-full (a)
  (v-stack :width (ui-lego-wide-col-w) :gap (ui-lego-gap) a))

(def ui-lego-strip-s (title accent section body)
  (ui-lego-surface-width-s title (ui-lego-strip-w) (ui-lego-full-h) accent section :instrument-group-bg body))

(def ui-lego-strip-half-s (title accent section body)
  (ui-lego-surface-width-s title (ui-lego-strip-w) (ui-lego-medium-h) accent section :instrument-group-bg body))

(def ui-lego-strip-panel-s (section body)
  (ui-lego-panel-width-s (ui-lego-strip-w) (ui-lego-full-h) section :instrument-group-bg body))

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
            :value (custom-ui-param-value p)
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
            :text-color accent :label-color :dim
            :width width :height 2.62
            :value-align :center
            :on-change (custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-knob-s (section name title width accent decimals)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-knob-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-knob-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (custom-ui-param-value p)
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
            :text-color accent :label-color :dim
            :width width :height 2.62
            :value-align :center
            :on-change (custom-ui-param-change-callback-s section p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-num (name title width decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-num-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-num-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (number-picker :value (custom-ui-param-value p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-color accent :edit-color :yellow
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
            (number-picker :value (custom-ui-param-value p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-color accent :edit-color :yellow
              :text-align :left
              :width width :height 0.68
              :on-change (custom-ui-param-change-callback-s section p)))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-micro-num-s (section name title width decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-micro-num-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-micro-num-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (v-stack :width width :height 1.18 :gap 0.16 :align :start
            (label title :font-size 7.4 :width width :height 0.52 :color :dim :bg :transparent)
            (number-picker :value (custom-ui-param-value p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 9.0
              :text-color accent :edit-color :yellow
              :text-align :left
              :width width :height 0.50
              :on-change (custom-ui-param-change-callback-s section p)))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ui-lego-option (name title width options accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-lego-option-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-option-" (custom-ui-scope-name) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (dropdown :value-index (custom-ui-param-value p)
              :value-index-offset (get p :min)
              :options options
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
            (dropdown :value-index (custom-ui-param-value p)
              :value-index-offset (get p :min)
              :options options
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
            (dropdown :value-index (custom-ui-param-value p)
              :value-index-offset (get p :min)
              :options options
              :width width :height 0.92 :font-size 8.6
              :on-change (lambda (v)
                (do
                  (custom-ui-select-section-in-scope scope section)
                  (custom-ui-set-param-in-scope
                    scope
                    p
                    (+ (get p :min) (custom-ui-option-index options v)))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ui-lego-micro-base-note-s (section width accent)
  (let ((p (custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-lego-micro-base-note-" (custom-ui-scope-name))
        (v-stack :width width :height 1.18 :gap 0.16 :align :start
          (label "note" :font-size 7.4 :width width :height 0.52 :color :dim :bg :transparent)
          (number-picker :value (custom-ui-param-value p)
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals 0
            :step 1
            :noui true :font-size 9.0
            :text-color accent :edit-color :yellow
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
            (number-picker :value (custom-ui-param-value p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-align :left
              :text-color accent :edit-color :yellow
              :width 6.0 :height 0.78
              :on-change (custom-ui-param-change-callback p)))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-base-note (width accent)
  (let ((p (custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-lego-base-note-" (custom-ui-scope-name))
        (v-stack :width width :height 1.12 :gap 0.08 :align :start
          (label "note" :font-size 8.2 :width width :color :dim :bg :transparent)
          (number-picker :value (custom-ui-param-value p)
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals 0
            :step 1
            :noui true :font-size 10.2
            :text-color accent :edit-color :yellow
            :text-align :left
            :width width :height 0.68
            :on-change (custom-ui-param-change-callback p))))
      (label "missing: base_note" :font-size 9 :color :red :bg :transparent))))

(def ui-adsr-number (name title decimals unit)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-adsr-number-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-adsr-number-" (custom-ui-scope-name) "-" name)
          (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
            (label title :font-size 10 :color :dim :bg :transparent)
            (number-picker :value (custom-ui-param-value p)
              :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.5
              :text-align :center
              :text-color :widget_focus_bg :edit-color :yellow
              :width 5.0 :height 0.95
              :on-change (custom-ui-param-change-callback p)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-adsr-number-s (section name title decimals unit)
  (if name
    (let ((p (custom-ui-current-param name)))
      (if p
        (custom-ui-param-mod-wrapper p (str "custom-ui-adsr-number-mod-" (custom-ui-scope-name) "-" name)
          (subtree :key (str "custom-ui-adsr-number-" (custom-ui-scope-name) "-" name)
            (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
              (label title :font-size 10 :color :dim :bg :transparent)
              (number-picker :value (custom-ui-param-value p)
                :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals decimals
                :unit unit
                :noui true :font-size 10.5
                :text-align :center
                :text-color :widget_focus_bg :edit-color :yellow
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
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (if release (ui-param-bound-value release 120) 0)
        :width 22.0 :height 4.0
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-select-section-in-scope scope section)
            (custom-ui-set-param-by-name-in-scope scope attack (get env :attack))
            (custom-ui-set-param-by-name-in-scope scope decay (get env :decay))
            (custom-ui-set-param-by-name-in-scope scope sustain (get env :sustain))
            (if release
              (custom-ui-set-param-by-name-in-scope scope release (get env :release))
              false))))
      (box :width :fill :height 1.75 :padding 0.15
        (h-stack :width :fill :gap 0.20 :align :start
          (ui-adsr-number-s section attack "atk" 0 "ms")
          (ui-adsr-number-s section decay "dec" 0 "ms")
          (ui-adsr-number-s section sustain "sus" 2 false)
          (ui-adsr-number-s section release "rel" 0 "ms")))
      (box :width :fill :height 0.35 :h-align :center :v-align :center
        (label title :font-size 8.5 :color :dim :bg :transparent))
      (box :width :fill :flex 1)))))

(def ui-adsr (title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
  (box :width 23.1 :height :fill
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 12 :padding 0.15
    (v-stack :width :fill :height :fill :gap 0.10
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (ui-param-bound-value release 120)
        :width 22.0 :height 4.0
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-set-param-by-name-in-scope scope attack (get env :attack))
            (custom-ui-set-param-by-name-in-scope scope decay (get env :decay))
            (custom-ui-set-param-by-name-in-scope scope sustain (get env :sustain))
            (custom-ui-set-param-by-name-in-scope scope release (get env :release)))))
      (box :width :fill :height 1.75 :padding 0.15
        (h-stack :width :fill :gap 0.20 :align :start
          (ui-adsr-number attack "atk" 0 "ms")
          (ui-adsr-number decay "dec" 0 "ms")
          (ui-adsr-number sustain "sus" 2 false)
          (ui-adsr-number release "rel" 0 "ms")))
      (box :width :fill :height 0.35 :h-align :center :v-align :center
        (label title :font-size 8.5 :color :dim :bg :transparent))
      (box :width :fill :flex 1)))))

(def ui-adsr-switch (section-a title-a attack-a decay-a sustain-a release-a
                     section-b title-b attack-b decay-b sustain-b release-b)
  (if (= custom-ui-selected-section section-b)
    (ui-adsr title-b attack-b decay-b sustain-b release-b)
    (ui-adsr title-a attack-a decay-a sustain-a release-a)))

(def ui-detail-adsr-s (section title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
    (ui-readout-panel-medium-s section
      (h-stack :width :fill :height :fill :gap 0.24 :align :stretch
        (adsr-editor
          :attack (ui-param-bound-value attack 5)
          :decay (ui-param-bound-value decay 120)
          :sustain (ui-param-bound-value sustain 0.7)
          :release (ui-param-bound-value release 120)
          :width 13.2 :height :fill
          :background-color :instrument-control-bg
          :on-change (lambda (env)
            (do
              (custom-ui-select-section-in-scope scope section)
              (custom-ui-set-param-by-name-in-scope scope attack (get env :attack))
              (custom-ui-set-param-by-name-in-scope scope decay (get env :decay))
              (custom-ui-set-param-by-name-in-scope scope sustain (get env :sustain))
              (custom-ui-set-param-by-name-in-scope scope release (get env :release)))))
        (v-stack :width 8.2 :height :fill :gap 0.10 :align :start
          (ui-lego-badge-dark title 7.7 (ui-accent-blue))
          (h-stack :gap 0.14 :align :start
            (ui-lego-micro-num-s section attack "atk" 3.7 0 "ms" (ui-accent-blue))
            (ui-lego-micro-num-s section decay "dec" 3.7 0 "ms" (ui-accent-blue)))
          (h-stack :gap 0.14 :align :start
            (ui-lego-micro-num-s section sustain "sus" 3.7 2 false (ui-accent-blue))
            (ui-lego-micro-num-s section release "rel" 3.7 0 "ms" (ui-accent-blue))))))))

(def ui-detail-adsr-switch-s (section-a title-a attack-a decay-a sustain-a release-a
                              section-b title-b attack-b decay-b sustain-b release-b)
  (if (= custom-ui-selected-section section-b)
    (ui-detail-adsr-s section-b title-b attack-b decay-b sustain-b release-b)
    (ui-detail-adsr-s section-a title-a attack-a decay-a sustain-a release-a)))

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
(def ui-rack-col-breathe (col)
  (v-stack :width 31.0 :gap 0.10 col))
(def ui-rack-col-compact (col)
  (v-stack :width 20.0 :gap 0.08 col))
(def ui-rack (mode left-panels adsr-form right-panels)
  (if (= mode :compact)
    (h-stack :width :fill :gap 0.35 :align :stretch
      (map ui-rack-col-compact (chunks left-panels 4))
      adsr-form
      (map ui-rack-col-compact (chunks right-panels 4)))
    (h-stack :width :fill :gap 0.4 :align :stretch
      (map ui-rack-col-breathe (chunks left-panels 2))
      adsr-form
      (map ui-rack-col-breathe (chunks right-panels 2)))))

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
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (ui-param-bound-value release 120)
        :width 20.0 :height 4.0
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-set-param-by-name-in-scope scope attack (get env :attack))
            (custom-ui-set-param-by-name-in-scope scope decay (get env :decay))
            (custom-ui-set-param-by-name-in-scope scope sustain (get env :sustain))
            (custom-ui-set-param-by-name-in-scope scope release (get env :release)))))
      (box :width :fill :height 1.45 :padding 0.1
        (h-stack :width :fill :gap 0.15 :align :start
          (ui-adsr-number attack "atk" 0 "ms")
          (ui-adsr-number decay "dec" 0 "ms")
          (ui-adsr-number sustain "sus" 2 false)
          (ui-adsr-number release "rel" 0 "ms")))
      (box :width :fill :height 0.3 :h-align :center :v-align :center
        (label title :font-size 7.5 :color :dim :bg :transparent))
      (box :width :fill :flex 1)))))

(def ui-adsr-switch-c (section-a title-a attack-a decay-a sustain-a release-a
                       section-b title-b attack-b decay-b sustain-b release-b)
  (if (= custom-ui-selected-section section-b)
    (ui-adsr-c title-b attack-b decay-b sustain-b release-b)
    (ui-adsr-c title-a attack-a decay-a sustain-a release-a)))
