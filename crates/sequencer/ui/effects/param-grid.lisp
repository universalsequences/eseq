;; Generic parameter-grid rows used by instruments and effects.
(def fx-param-row (p fx subtree-key)
  (subtree :key subtree-key
    (param-mod-wrapper fx p (str subtree-key "-mod-wrapper")
    (box :height 1.25
      (h-stack :gap 0.45 :align :center
        (box :width 13.2 :height 1.25
          (h-stack :gap 0.25 :align :baseline
            (label (substring (get p :name) 0 9) :font-size 12 :width 7
                   :color :dim :bg :transparent)
            (if (get p :boolean)
              (button (if (fx-param-on? p) "ON" "OFF")
                   :width 5.5 :height 1.25 :padding 0 :font-size 11
                   :background-color :transparent
                   :border-color :transparent
                   :color :white
                   :plock-active (if (param-plock-active? fx p) 1 0)
                   :plock-color-r (param-plock-color-r)
                   :plock-color-g (param-plock-color-g)
                   :plock-color-b (param-plock-color-b)
                   :on-click |x y r|
                     (if fx
                       (fx-toggle-effect-value fx p)
                       (fx-toggle-instrument-value p)))
              (if (get p :options)
              (dropdown :value (fx-param-text-value-for fx p)
                :options (get p :options)
                :on-change (lambda (v) (param-set-option fx p v))
                :plock-active (if (param-plock-active? fx p) 1 0)
                :plock-color-r (param-plock-color-r)
                :plock-color-g (param-plock-color-g)
                :plock-color-b (param-plock-color-b)
                :width 5.8 :height 1.2 :font-size 11)
              (number-picker :value (fx-param-value-for fx p)
                :min (param-control-min fx p) :max (param-control-max fx p) :decimals 2
                :noui true :font-size 12 :text-color (param-plock-text-color fx p)
                :plock-active (if (param-plock-active? fx p) 1 0)
                :plock-color-r (param-plock-color-r)
                :plock-color-g (param-plock-color-g)
                :plock-color-b (param-plock-color-b)
                :on-change (lambda (v)
                  (param-set-control-value fx p v))
                :width 5.2 :height 1.1)))))
        (if (or (get p :options) (get p :boolean))
          (label "" :width 7.8 :bg :transparent)
          (hslider :width 7.8 :min (param-control-min fx p) :max (param-control-max fx p)
                   :value (fx-param-value-for fx p)
                   :material (aqua-slider-material)
                   :plock-active (if (param-plock-active? fx p) 1 0)
                   :plock-color-r (param-plock-color-r)
                   :plock-color-g (param-plock-color-g)
                   :plock-color-b (param-plock-color-b)
                   :on-change (lambda (v)
                     (param-set-control-value fx p v)))))))))

(def fx-param-subtree-key (fx p ci)
  (if fx
    (if (get fx :midi-fx)
      (str "midi-fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))
      (if (get fx :bus-fx)
        (str "bus-fx-slot-" (get fx :bus-idx) "-" (get fx :slot-idx) "-param-" (get p :idx))
        (str "fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))))
    (str "instrument-tab-" instrument-panel-tab "-chunk-" ci "-param-" (get p :idx))))

(def fx-flat-param-grid (params fx)
  (h-stack :gap 1.5 :padding 0.525
    (each (chunks (visible-params params) 4) |chunk ci|
      (v-stack :gap 0.25
        (each chunk |p pi|
          (fx-param-row p fx (fx-param-subtree-key fx p ci)))))))

(def fx-list-contains? (items value)
  (> (len (filter |item| (= item value) items)) 0))

(def fx-param-grid-has-metadata? (params)
  (> (len (filter |p| (or (get p :group) (get p :env)) params)) 0))

(defstate fx-param-grid-selected-sections '())

(def fx-param-grid-scope-key (fx)
  (if fx
    (if (get fx :midi-fx)
      (str "midi-fx-slot-" (get fx :slot-idx))
      (if (get fx :bus-fx)
        (str "bus-fx-" (get fx :bus-idx) "-slot-" (get fx :slot-idx))
        (str "audio-fx-slot-" (get fx :slot-idx))))
    (str "instrument-tab-" instrument-panel-tab)))

(def fx-param-grid-set-selected-section (scope-key section)
  (set! fx-param-grid-selected-sections
    (cons
      (dict :scope scope-key :section section)
      (filter |item| (not (= (get item :scope) scope-key))
        fx-param-grid-selected-sections))))

(def fx-param-grid-section-select-callback (fx section)
  (let ((scope-key (fx-param-grid-scope-key fx)))
    (lambda (info)
      (fx-param-grid-set-selected-section scope-key section))))

(def fx-param-group-names (params)
  (reduce |groups p|
    (if (get p :group)
      (if (fx-list-contains? groups (get p :group))
        groups
        (append groups (list (get p :group))))
      groups)
    '()
    params))

(def fx-param-group-index (groups group-name)
  (let ((idx
          (nth
            (filter |i| (= (nth groups i) group-name)
              (range (len groups)))
            0)))
    (if idx idx 0)))

(def fx-param-in-group? (p group-name)
  (if group-name
    (= (get p :group) group-name)
    (not (get p :group))))

(def fx-env-role-param (params env-name role-name)
  (nth (filter |p| (and (= (get p :env) env-name)
                        (= (get p :role) role-name))
       params)
       0))

(def fx-env-complete? (params env-name)
  (and (fx-env-role-param params env-name "attack")
       (fx-env-role-param params env-name "decay")
       (fx-env-role-param params env-name "sustain")
       (fx-env-role-param params env-name "release")))

(def fx-env-first-param (params env-name)
  (nth (filter |p| (= (get p :env) env-name) params) 0))

(def fx-env-first-param? (params p)
  (let ((first-p (fx-env-first-param params (get p :env))))
    (and first-p (= (get first-p :idx) (get p :idx)))))

(def fx-adsr-role? (role-name)
  (or (= role-name "attack")
      (= role-name "decay")
      (= role-name "sustain")
      (= role-name "release")))

(def fx-param-consumed-by-adsr? (params p)
  (and (get p :env)
       (fx-adsr-role? (get p :role))
       (fx-env-complete? params (get p :env))))

(def fx-param-adsr-source? (params p)
  (and (get p :env)
       (fx-env-complete? params (get p :env))
       (fx-env-first-param? params p)))

(def fx-param-normal-metadata-control? (params p)
  (and (not (fx-param-consumed-by-adsr? params p))
       (not (fx-param-adsr-source? params p))))

(def fx-param-env-source-for-group (params group-name)
  (nth (filter |p| (and (fx-param-adsr-source? params p)
                        (= (get p :group) group-name))
       params)
       0))

(def fx-param-first-env-source (params)
  (nth (filter |p| (fx-param-adsr-source? params p) params) 0))

(def fx-param-default-env-source (params)
  (let ((amp-source (fx-param-env-source-for-group params "amp")))
    (if amp-source
      amp-source
      (fx-param-first-env-source params))))

(def fx-param-default-env-section (params groups)
  (let ((source (fx-param-default-env-source params)))
    (if source
      (fx-param-group-index groups (get source :group))
      0)))

(def fx-param-grid-selected-section (params groups fx)
  (let ((scope-key (fx-param-grid-scope-key fx)))
    (let ((entry
            (nth
              (filter |item| (= (get item :scope) scope-key)
                fx-param-grid-selected-sections)
              0)))
      (if entry
        (get entry :section)
        (fx-param-default-env-section params groups)))))

(def fx-param-grid-panel-select-section (params groups group-name)
  (if (fx-param-env-source-for-group params group-name)
    (fx-param-group-index groups group-name)
    (fx-param-default-env-section params groups)))

(def fx-param-grid-panel-select-callback (params groups fx group-name)
  (fx-param-grid-section-select-callback fx
    (fx-param-grid-panel-select-section params groups group-name)))

(def fx-param-grid-default-section-callback (params groups fx)
  (fx-param-grid-section-select-callback fx
    (fx-param-default-env-section params groups)))

(def fx-param-grid-panel-bg (params groups fx group-name)
  (let ((section (fx-param-group-index groups group-name)))
    (if (and (fx-param-env-source-for-group params group-name)
             (= (fx-param-grid-selected-section params groups fx) section))
      :instrument-group-selected-bg
      :instrument-group-bg)))

(def fx-param-selected-env-source (params groups fx)
  (let ((selected-group (nth groups (fx-param-grid-selected-section params groups fx))))
    (let ((selected-source
            (if selected-group
              (fx-param-env-source-for-group params selected-group)
              false)))
      (if selected-source
        selected-source
        (fx-param-default-env-source params)))))

(def fx-param-compact-label (p)
  (substring (get p :name) 0 9))

(def fx-param-compact-control-width () 6.3)
(def fx-param-compact-control-height () 2.55)
(def fx-param-group-label-width () 5.4)
(def fx-param-group-label-gap () 0.25)
(def fx-param-group-control-gap () 0.22)
(def fx-param-group-row-gap () 0.18)
(def fx-param-group-panel-padding () 0.16)
(def fx-param-group-label-padding () 0.22)

(def fx-param-compact-button (p fx key)
  (param-mod-wrapper fx p (str key "-mod-wrapper")
    (subtree :key key
      (v-stack :width (fx-param-compact-control-width)
               :height (fx-param-compact-control-height) :gap 0.12 :align :center
        (label (fx-param-compact-label p) :font-size 8.7 :width (fx-param-compact-control-width)
               :color :dim :bg :transparent)
        (button (if (fx-param-on? p) "ON" "OFF")
          :width 4.2 :height 1.05 :padding 0 :font-size 10.0
          :background-color (if (fx-param-on? p) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
          :color (if (fx-param-on? p) :black :dim)
          :plock-active (if (param-plock-active? fx p) 1 0)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :on-click |x y r|
            (if fx
              (fx-toggle-effect-value fx p)
              (fx-toggle-instrument-value p)))))))

(def fx-param-compact-option (p fx key)
  (param-mod-wrapper fx p (str key "-mod-wrapper")
    (subtree :key key
      (v-stack :width (fx-param-compact-control-width)
               :height (fx-param-compact-control-height) :gap 0.12 :align :center
        (label (fx-param-compact-label p) :font-size 8.7 :width (fx-param-compact-control-width)
               :color :dim :bg :transparent)
        (dropdown :value (get p :text-value)
          :options (get p :options)
          :on-change (lambda (v) (param-set-option fx p v))
          :plock-active (if (param-plock-active? fx p) 1 0)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :width 5.9 :height 1.05 :font-size 9.2)))))

(def fx-param-compact-knob (p fx key)
  (param-mod-wrapper fx p (str key "-mod-wrapper")
    (subtree :key key
      (box :debug-name (str "fx-param-compact-knob-" (get p :name))
           :width (fx-param-compact-control-width)
           :height (fx-param-compact-control-height) :padding 0
        (knob-number :label (fx-param-compact-label p)
          :value (fx-param-value-for fx p)
          :min (param-control-min fx p) :max (param-control-max fx p) :decimals 2
          :font-size 10.0 :label-font-size 8.8
          :text-color (param-plock-text-color fx p) :label-color :dim
          :plock-active (if (param-plock-active? fx p) 1 0)
          :plock-default (param-plock-default fx p)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :width (fx-param-compact-control-width) :height 2.42
          :on-change (lambda (v)
            (param-set-control-value fx p v)))))))

(def fx-param-compact-control (p fx key)
  (if (get p :boolean)
    (fx-param-compact-button p fx key)
    (if (get p :options)
      (fx-param-compact-option p fx key)
      (fx-param-compact-knob p fx key))))

(def fx-param-adsr-number (p fx key title decimals unit)
  (param-mod-wrapper fx p (str key "-mod-wrapper")
    (subtree :key key
      (box :debug-name (str "fx-param-adsr-number-" title)
           :width 5.2 :height 1.55 :padding 0
        (v-stack :width 5.2 :height :fill :gap 0.0 :align :center
          (label title :font-size 9.0 :color :dim :bg :transparent)
          (number-picker :value (fx-param-value-for fx p)
            :min (param-control-min fx p) :max (param-control-max fx p)
            :decimals decimals :unit unit
            :noui true :font-size 10.0
            :text-align :center
            :text-color (param-plock-text-color fx p) :edit-color :yellow
            :plock-active (if (param-plock-active? fx p) 1 0)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
            :width 5.0 :height 0.82
            :on-change (lambda (v)
              (param-set-control-value fx p v))))))))

(def fx-param-adsr-editor (params fx env-name key-prefix)
  (let ((attack-p (fx-env-role-param params env-name "attack"))
        (decay-p (fx-env-role-param params env-name "decay"))
        (sustain-p (fx-env-role-param params env-name "sustain"))
        (release-p (fx-env-role-param params env-name "release")))
    (subtree :key key-prefix
      (box :width 23.2 :height :fill
           :background-color :instrument-control-bg
           :border-width 1 :corner-radius 7 :padding 0.16
           :debug-name (str "fx-param-env-" env-name)
        (v-stack :width :fill :height :fill :gap 0.12
          (box :width :fill :height 2.95 :padding 0.08
            (adsr-editor
              :attack (fx-param-value-for fx attack-p)
              :decay (fx-param-value-for fx decay-p)
              :sustain (fx-param-value-for fx sustain-p)
              :release (fx-param-value-for fx release-p)
              :width :fill :height :fill
              :background-color :instrument-control-bg
              :on-change (lambda (env)
                (if (and fx (not (get fx :rack-fx)) (not (get fx :bus-fx)) (not (get fx :midi-fx)))
                  (host-command
                    (if (seq-has-selection?) "set-effect-plock-batch" "set-effect-param-batch")
                    (dict :slot-idx (get fx :slot-idx)
                          :updates (list
                            (dict :param-idx (get attack-p :idx) :value (get env :attack))
                            (dict :param-idx (get decay-p :idx) :value (get env :decay))
                            (dict :param-idx (get sustain-p :idx) :value (get env :sustain))
                            (dict :param-idx (get release-p :idx) :value (get env :release)))
                          :commit (not (get env :active))))
                  (do
                    (param-set-control-value fx attack-p (get env :attack))
                    (param-set-control-value fx decay-p (get env :decay))
                    (param-set-control-value fx sustain-p (get env :sustain))
                    (param-set-control-value fx release-p (get env :release)))))))
          (box :width :fill :height 1.58 :padding 0.08
            (h-stack :width :fill :gap 0.20 :align :start
              (fx-param-adsr-number attack-p fx (str key-prefix "-attack") "atk" 2 false)
              (fx-param-adsr-number decay-p fx (str key-prefix "-decay") "dec" 2 false)
              (fx-param-adsr-number sustain-p fx (str key-prefix "-sustain") "sus" 2 false)
              (fx-param-adsr-number release-p fx (str key-prefix "-release") "rel" 2 false)))
          (box :width :fill :height 0.52 :h-align :center :v-align :center
            (label env-name :font-size 9.4 :color :dim :bg :transparent
                   :debug-name (str "fx-param-env-label-" env-name)))
          (box :width :fill :flex 1))))))

(def fx-param-group-has-controls? (all-params group-params)
  (> (len (filter |p| (fx-param-normal-metadata-control? all-params p) group-params)) 0))

(def fx-param-group-has-visible-panel? (all-params group-name)
  (fx-param-group-has-controls? all-params
    (filter |p| (fx-param-in-group? p group-name) all-params)))

(def fx-param-visible-group-names (all-params groups)
  (filter |group-name| (fx-param-group-has-visible-panel? all-params group-name) groups))

(def fx-param-visible-group-controls (all-params group-params)
  (filter |p| (fx-param-normal-metadata-control? all-params p) group-params))

(def fx-param-group-row-extra-gap (row-count)
  (if (> row-count 1)
    (* (- row-count 1) (fx-param-group-row-gap))
    0))

(def fx-param-group-controls-per-row (controls)
  (let ((control-count (len controls)))
    (max 1
      (if (> control-count 6)
        (ceil (/ control-count 2))
        control-count))))

(def fx-param-group-control-rows (controls)
  (chunks controls (fx-param-group-controls-per-row controls)))

(def fx-param-group-row-panel-height (controls)
  (let ((row-count (len (fx-param-group-control-rows controls))))
    (+ (* (fx-param-group-panel-padding) 2)
       (* row-count (fx-param-compact-control-height))
       (fx-param-group-row-extra-gap row-count))))

(def fx-param-group-control-row-width (control-count)
  (if (> control-count 0)
    (+ (* control-count (fx-param-compact-control-width))
       (* (max (- control-count 1) 0) (fx-param-group-control-gap)))
    0))

(def fx-param-group-controls-column-width (controls)
  (reduce |width row|
    (max width (fx-param-group-control-row-width (len row)))
    0
    (fx-param-group-control-rows controls)))

(def fx-param-group-row-panel-width (controls)
  (max
    (+ (* (fx-param-group-panel-padding) 2)
       (fx-param-group-label-width)
       (fx-param-group-label-gap)
       (fx-param-group-controls-column-width controls))
    (+ (* (fx-param-group-panel-padding) 2)
       (fx-param-group-label-width)
       (fx-param-group-label-gap)
       (fx-param-compact-control-width))))

(def fx-param-group-controls-by-name (all-params group-name)
  (fx-param-visible-group-controls all-params
    (filter |p| (fx-param-in-group? p group-name) all-params)))

(def fx-param-left-column-width-for-groups (all-params group-names)
  (reduce |width group-name|
    (max width (fx-param-group-row-panel-width
                 (fx-param-group-controls-by-name all-params group-name)))
    0
    group-names))

(def fx-param-left-column-width (all-params groups ungrouped)
  (max
    (fx-param-left-column-width-for-groups all-params
      (fx-param-visible-group-names all-params groups))
    (if (> (len ungrouped) 0)
      (fx-param-group-row-panel-width
        (fx-param-visible-group-controls all-params ungrouped))
      0)))

(def fx-param-group-panel-items (all-params groups)
  (reduce |items group-name|
    (append items (list (dict :label group-name :group group-name :misc false)))
    '()
    (fx-param-visible-group-names all-params groups)))

(def fx-param-panel-items (all-params groups ungrouped)
  (let ((group-items (fx-param-group-panel-items all-params groups)))
    (if (> (len ungrouped) 0)
      (append group-items (list (dict :label "misc" :group false :misc true)))
      group-items)))

(def fx-param-panel-item-group-params (all-params item)
  (if (get item :misc)
    (filter |p| (not (get p :group)) all-params)
    (filter |p| (fx-param-in-group? p (get item :group)) all-params)))

(def fx-param-group-row-panel (all-params groups group-params group-label group-name fx ci)
  (let ((controls (fx-param-visible-group-controls all-params group-params)))
    (box :width (fx-param-group-row-panel-width controls)
         :height (fx-param-group-row-panel-height controls)
         :background-color (fx-param-grid-panel-bg all-params groups fx group-name)
         :border-width 1 :corner-radius 7 :padding (fx-param-group-panel-padding)
         :debug-name (str "fx-param-group-" group-label)
         :on-click (fx-param-grid-panel-select-callback all-params groups fx group-name)
      (h-stack :width :fill :height :fill :gap (fx-param-group-label-gap) :align :center
        (box :width (fx-param-group-label-width) :height :fill
             :h-align :start :v-align :center :padding (fx-param-group-label-padding)
          (label group-label :font-size 9.4 :width 4.8
                 :color :dim :bg :transparent))
        (v-stack :width (fx-param-group-controls-column-width controls)
                 :height :fill :gap (fx-param-group-row-gap) :align :start
          (each (fx-param-group-control-rows controls) |chunk row-idx|
            (h-stack :width :fill :gap (fx-param-group-control-gap) :align :start
              (each chunk |p pi|
                (fx-param-compact-control p fx (fx-param-subtree-key fx p (+ ci row-idx)))))))))))

(def fx-param-panel-item (all-params groups item fx ci)
  (fx-param-group-row-panel all-params groups
    (fx-param-panel-item-group-params all-params item)
    (get item :label)
    (get item :group)
    fx
    ci))

(def fx-param-panel-columns (all-params groups ungrouped fx)
  (h-stack :gap 0.35 :align :start
    (each (chunks (fx-param-panel-items all-params groups ungrouped) 3) |column col-idx|
      (v-stack :gap 0.16
        (each column |item row-idx|
          (fx-param-panel-item all-params groups item fx (+ (* col-idx 3) row-idx)))))))

(def fx-param-selected-env-panel (params groups fx)
  (let ((source (fx-param-selected-env-source params groups fx)))
    (if source
      (fx-param-adsr-editor params fx (get source :env)
        (str "metadata-selected-env-" (get source :env)))
      (box :width 0 :height 0))))

(def fx-metadata-param-grid (params fx)
  (let ((visible (visible-params params))
        (groups (fx-param-group-names visible))
        (ungrouped (filter |p| (not (get p :group)) visible)))
    (box :debug-name "fx-param-metadata-grid"
         :width :fill :height 9.8 :padding 0.525
         :on-click (fx-param-grid-default-section-callback visible groups fx)
      (h-stack :width :fill :height :fill :gap 1.0 :align :stretch
        (fx-param-panel-columns visible groups ungrouped fx)
        (fx-param-selected-env-panel visible groups fx)))))

(def fx-param-grid (params fx)
  (let ((visible (visible-params params)))
    (if (fx-param-grid-has-metadata? visible)
      (fx-metadata-param-grid params fx)
      (fx-flat-param-grid params fx))))
