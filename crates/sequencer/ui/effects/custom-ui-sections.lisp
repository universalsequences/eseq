;; Section selection and base panel primitives for generated custom UIs.
(defstate custom-ui-selected-sections '())
(defstate custom-ui-active-adsr false)
(def custom-ui-selected-section 0)

(def custom-ui-set-active-adsr (scope section active)
  (set! custom-ui-active-adsr
    (if active
      (dict :scope (get scope :name) :section section :stage active)
      false)))

(def custom-ui-adsr-stage-active? (section stage)
  (if custom-ui-active-adsr
    (and
      (= (get custom-ui-active-adsr :scope) (custom-ui-scope-name))
      (= (get custom-ui-active-adsr :section) section)
      (= (get custom-ui-active-adsr :stage) stage))
    false))

(def custom-ui-selected-section-for-scope (scope-name)
  (let ((entry
          (nth
            (filter |item| (= (get item :scope) scope-name)
              custom-ui-selected-sections)
            0)))
    (if entry (get entry :section) 0)))

(def custom-ui-selected-section-for-current-scope ()
  (custom-ui-selected-section-for-scope (custom-ui-scope-name)))

(def custom-ui-set-selected-section-for-scope (scope-name section)
  (set! custom-ui-selected-sections
    (cons
      (dict :scope scope-name :section section)
      (filter |item| (not (= (get item :scope) scope-name))
        custom-ui-selected-sections))))

(def custom-ui-select-section-in-scope (scope section)
  (custom-ui-set-selected-section-for-scope (get scope :name) section))

(def ui-select-section (section)
  (custom-ui-set-selected-section-for-scope (custom-ui-scope-name) section))

(def ui-section-select-callback (section)
  (let ((scope-name (custom-ui-scope-name)))
    (lambda (info)
      (custom-ui-set-selected-section-for-scope scope-name section))))

(def ui-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= custom-ui-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))

(def ui-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))

(def ui-panel-header (title)
  (box :width :fill :height 0.5 :h-align :start :v-align :center :padding 0.15
    (label title :font-size 7.5 :color :dim :bg :transparent)))

(def ui-section (title body)
  (box :width :fill :height 3.4
       :background-color :instrument-group-bg
       :border-width 1 :corner-radius 12 :padding 0.15
    (v-stack :width :fill :gap 0.2 :align :start
      (ui-panel-header title)
      body)))

(def ui-panel (title section body)
  (box :width :fill :height 3.4
       :background-color (ui-panel-bg section)
       :border-width 1 :corner-radius 12 :padding 0.15
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :gap 0.2 :align :start
      (ui-panel-header title)
      body)))
