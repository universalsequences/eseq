;; Standard generated-UI controls.
(def ui-param-knob (name title)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-knob-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-knob-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (custom-ui-param-value p)
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals 2
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
            :font-size 10.5 :label-font-size 10
            :text-color (custom-ui-param-plock-text-color p) :label-color :dim
            :plock-active (if (custom-ui-param-plock-active? p) 1 0)
            :plock-default (custom-ui-param-plock-default p)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
            :width 4.4 :height 2.4
            :value-align :center
            :on-change (custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-param-matrix (name width height)
  (let ((p (custom-ui-current-tensor-param name)))
    (if p
      (subtree :key (str "custom-ui-matrix-" (custom-ui-scope-name) "-" name)
        (matrix :rows (get p :rows) :cols (get p :cols)
          :value (custom-ui-tensor-bound-values p)
          :min (get p :min) :max (get p :max)
          :control :grid
          :width width :height height
          :on-cell-change (custom-ui-tensor-cell-change-callback p)))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

;; Compact knob: ~1.7 cell tall, value nestled in the lower-right of the knob
;; arc (default value-align) so the knob itself stays large. For instruments
;; that need 3-4 rows of params instead of 2.
(def ui-param-knob-c (name title)
  (let ((p (custom-ui-current-param name)))
    (if p
      (custom-ui-param-mod-wrapper p (str "custom-ui-knob-c-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-knob-c-" (custom-ui-scope-name) (custom-ui-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (custom-ui-param-value p)
            :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals 2
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
            :font-size 8.5 :label-font-size 7.5
            :text-color (custom-ui-param-plock-text-color p) :label-color :dim
            :plock-active (if (custom-ui-param-plock-active? p) 1 0)
            :plock-default (custom-ui-param-plock-default p)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
            :width 3.8 :height 1.8
            :label-height 0.5 :knob-size 1.25
            :on-change (custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def base-note-c ()
  (let ((p (custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-base-note-c-" (custom-ui-scope-name))
        (knob-number :label "note"
          :value (custom-ui-param-value p)
          :min (custom-ui-param-control-min p) :max (custom-ui-param-control-max p) :decimals 0
          :step 1
          :font-size 8.5 :label-font-size 7.5
          :text-color (custom-ui-param-plock-text-color p) :label-color :dim
          :plock-active (if (custom-ui-param-plock-active? p) 1 0)
          :plock-default (custom-ui-param-plock-default p)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :width 3.8 :height 1.8
          :label-height 0.5 :knob-size 1.25
          :on-change (custom-ui-param-change-callback p)))
      (label "missing: base_note" :font-size 9 :color :red :bg :transparent))))

(def ui-panel-header-c (title)
  (box :width 3.5 :height :fill :h-align :end :v-align :center :padding 0.1
    (label title :font-size 6 :color :dim :bg :transparent)))

;; Compact panel: title runs along the LEFT edge (vertical strip) so each
;; row only takes the height of one knob — no separate title band on top.
(def ui-panel-c (title section body)
  (box :width :fill :height 2.0
       :background-color (ui-panel-bg section)
       :border-width 1 :corner-radius 10 :padding 0.08
       :on-click (ui-section-select-callback section)
    (h-stack :width :fill :gap 0.1 :align :center
      (ui-panel-header-c title)
      body)))

(def ui-param-value (name fallback)
  (let ((p (custom-ui-current-param name)))
    (if p (get p :value) fallback)))

(def ui-param-bound-value (name fallback)
  (let ((p (custom-ui-current-param name)))
    (if p (custom-ui-param-value p) fallback)))

(def ui-set-param (name value)
  (let ((p (custom-ui-current-param name)))
    (if p (custom-ui-set-param p value) false)))
