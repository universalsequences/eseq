;; Generic parameter-grid rows used by instruments and effects.
(module eseq.effects.param-grid)

(import eseq.effects.param-controls :as pc)
(import eseq.effects.state :refer (instrument-panel-tab))
(import eseq.effects.effect-panels :refer (visible-params))

(export fx-param-row
        fx-list-contains?
        fx-param-grid)

;; Migration aliases (module spec §10). Identity aliases only: this file's
;; three public names are called by their flat spelling from many still-
;; unconverted files (effects/panel-bodies.lisp, the effects/builtin/* panels,
;; effects/custom-effect-ui.lisp,
;; effects/instrument-modulation.lisp, ui/capture-fixtures/*) and from Rust
;; test harnesses in src/ui/state_values/tests.rs, and bare callers cannot see
;; qualified names (hub-file precedent: keep public spellings). Converted
;; callers (eseq.effects.effect-modulation, eseq.effects.instrument-sources)
;; import this module instead. The aliases are deleted as callers convert.
;; Everything else is private. Subtree :key strings and :debug-name strings
;; are byte-identical (hazard e) — they are flat keyspaces that never qualify.

(def param-display-name (p)
  (let ((name (if (get p :display-name) (get p :display-name) (get p :name))))
    (let ((group-prefix (if (get p :group) (str (get p :group) ".") false)))
      (if (and group-prefix (string-starts-with? name group-prefix))
        (substring name (len group-prefix) (len name))
        name))))

(def param-options (p)
  (let ((options (get p :options)))
    (if (get options :file)
      (let ((metadata
              (if (get options :asset-base)
                (asset-metadata (get options :file) (get options :asset-base))
                (asset-metadata (get options :file)))))
        (if metadata
          (get metadata (get options :key))
          false))
      options)))

(def param-with-resolved-options (p)
  (let ((options (get p :options)))
    (merge p
      :options (param-options p)
      :integer-option-fallback (if (get options :file) true false))))

(def param-grid-control-value (p value)
  (if (get p :integer-option-fallback) (round value) value))

(def fx-param-row (p fx subtree-key)
  (let ((p (param-with-resolved-options p)))
    (subtree :key subtree-key
    (pc/param-mod-wrapper fx p (str subtree-key "-mod-wrapper")
    (box :height 1.25
      (h-stack :gap 0.45 :align :center
        (box :width 13.2 :height 1.25
          (h-stack :gap 0.25 :align :baseline
            (label (substring (param-display-name p) 0 9) :font-size 12 :width 7
                   :color :dim :bg :transparent)
            (if (get p :boolean)
              (button (if (pc/fx-param-on? p) "ON" "OFF")
                   :width 5.5 :height 1.25 :padding 0 :font-size 11
                   :background-color :transparent
                   :border-color :transparent
                   :color :white
                   :plock-active (if (pc/param-plock-active? fx p) 1 0)
                   :plock-color-r (pc/param-plock-color-r)
                   :plock-color-g (pc/param-plock-color-g)
                   :plock-color-b (pc/param-plock-color-b)
                   :on-click |x y r|
                     (if fx
                       (pc/fx-toggle-effect-value fx p)
                       (pc/fx-toggle-instrument-value p)))
              (if (get p :options)
              (dropdown :value (pc/fx-param-text-value-for fx p)
                :options (get p :options)
                :on-change (lambda (v) (pc/param-set-option fx p v))
                :plock-active (if (pc/param-plock-active? fx p) 1 0)
                :plock-color-r (pc/param-plock-color-r)
                :plock-color-g (pc/param-plock-color-g)
                :plock-color-b (pc/param-plock-color-b)
                :width 5.8 :height 1.2 :font-size 11)
              (number-picker :value (pc/fx-param-value-for fx p)
                :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
                :decimals (if (get p :integer-option-fallback) 0 2)
                :noui true :font-size 12 :text-color (pc/param-plock-text-color fx p)
                :plock-active (if (pc/param-plock-active? fx p) 1 0)
                :plock-color-r (pc/param-plock-color-r)
                :plock-color-g (pc/param-plock-color-g)
                :plock-color-b (pc/param-plock-color-b)
                :on-change (lambda (v)
                  (pc/param-set-control-value fx p (param-grid-control-value p v)))
                :width 5.2 :height 1.1)))))
        (if (or (get p :options) (get p :boolean))
          (label "" :width 7.8 :bg :transparent)
          (hslider :width 7.8 :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
                   :value (pc/fx-param-value-for fx p)
                   :material (eseq.materials/slider-material)
                   :plock-active (if (pc/param-plock-active? fx p) 1 0)
                   :plock-color-r (pc/param-plock-color-r)
                   :plock-color-g (pc/param-plock-color-g)
                   :plock-color-b (pc/param-plock-color-b)
                   :on-change (lambda (v)
                     (pc/param-set-control-value fx p (param-grid-control-value p v)))))))))))

(def param-subtree-key (fx p ci)
  (if fx
    (if (get fx :midi-fx)
      (str "midi-fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))
      (if (get fx :bus-fx)
        (str "bus-fx-slot-" (get fx :bus-idx) "-" (get fx :slot-idx) "-param-" (get p :idx))
        (str "fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))))
    (str "instrument-tab-" eseq.effects.state/instrument-panel-tab "-chunk-" ci "-param-" (get p :idx))))

(def flat-grid (params fx)
  (h-stack :gap 1.5 :padding 0.525
    (each (chunks (visible-params params) 4) |chunk ci|
      (v-stack :gap 0.25
        (each chunk |p pi|
          (fx-param-row p fx (param-subtree-key fx p ci)))))))

(def fx-list-contains? (items value)
  (> (len (filter |item| (= item value) items)) 0))

(def has-metadata? (params)
  (> (len (filter |p| (or (get p :group) (get p :env)) params)) 0))

(defstate selected-sections '())

(def scope-key (fx)
  (if fx
    (if (get fx :midi-fx)
      (str "midi-fx-slot-" (get fx :slot-idx))
      (if (get fx :bus-fx)
        (str "bus-fx-" (get fx :bus-idx) "-slot-" (get fx :slot-idx))
        (str "audio-fx-slot-" (get fx :slot-idx))))
    (str "instrument-tab-" eseq.effects.state/instrument-panel-tab)))

(def set-selected-section (scope-key section)
  (set! selected-sections
    (cons
      (dict :scope scope-key :section section)
      (filter |item| (not (= (get item :scope) scope-key))
        selected-sections))))

(def section-select-callback (fx section)
  (let ((scope-key (scope-key fx)))
    (lambda (info)
      (set-selected-section scope-key section))))

(def group-names (params)
  (reduce |groups p|
    (if (get p :group)
      (if (fx-list-contains? groups (get p :group))
        groups
        (append groups (list (get p :group))))
      groups)
    '()
    params))

(def group-index (groups group-name)
  (let ((idx
          (nth
            (filter |i| (= (nth groups i) group-name)
              (range (len groups)))
            0)))
    (if idx idx 0)))

(def in-group? (p group-name)
  (if group-name
    (= (get p :group) group-name)
    (not (get p :group))))

(def env-role-param (params env-name role-name)
  (nth (filter |p| (and (= (get p :env) env-name)
                        (= (get p :role) role-name))
       params)
       0))

(def env-complete? (params env-name)
  (and (env-role-param params env-name "attack")
       (env-role-param params env-name "decay")
       (env-role-param params env-name "sustain")
       (env-role-param params env-name "release")))

(def env-first-param (params env-name)
  (nth (filter |p| (= (get p :env) env-name) params) 0))

(def env-first-param? (params p)
  (let ((first-p (env-first-param params (get p :env))))
    (and first-p (= (get first-p :idx) (get p :idx)))))

(def adsr-role? (role-name)
  (or (= role-name "attack")
      (= role-name "decay")
      (= role-name "sustain")
      (= role-name "release")))

(def consumed-by-adsr? (params p)
  (and (get p :env)
       (adsr-role? (get p :role))
       (env-complete? params (get p :env))))

(def adsr-source? (params p)
  (and (get p :env)
       (env-complete? params (get p :env))
       (env-first-param? params p)))

(def normal-metadata-control? (params p)
  (and (not (consumed-by-adsr? params p))
       (not (adsr-source? params p))))

(def env-source-for-group (params group-name)
  (nth (filter |p| (and (adsr-source? params p)
                        (= (get p :group) group-name))
       params)
       0))

(def first-env-source (params)
  (nth (filter |p| (adsr-source? params p) params) 0))

(def default-env-source (params)
  (let ((amp-source (env-source-for-group params "amp")))
    (if amp-source
      amp-source
      (first-env-source params))))

(def default-env-section (params groups)
  (let ((source (default-env-source params)))
    (if source
      (group-index groups (get source :group))
      0)))

(def selected-section (params groups fx)
  (let ((scope-key (scope-key fx)))
    (let ((entry
            (nth
              (filter |item| (= (get item :scope) scope-key)
                selected-sections)
              0)))
      (if entry
        (get entry :section)
        (default-env-section params groups)))))

(def panel-select-section (params groups group-name)
  (if (env-source-for-group params group-name)
    (group-index groups group-name)
    (default-env-section params groups)))

(def panel-select-callback (params groups fx group-name)
  (section-select-callback fx
    (panel-select-section params groups group-name)))

(def default-section-callback (params groups fx)
  (section-select-callback fx
    (default-env-section params groups)))

(def panel-bg (params groups fx group-name)
  (let ((section (group-index groups group-name)))
    (if (and (env-source-for-group params group-name)
             (= (selected-section params groups fx) section))
      :instrument-group-selected-bg
      :instrument-group-bg)))

(def selected-env-source (params groups fx)
  (let ((selected-group (nth groups (selected-section params groups fx))))
    (let ((selected-source
            (if selected-group
              (env-source-for-group params selected-group)
              false)))
      (if selected-source
        selected-source
        (default-env-source params)))))

(def compact-label (p)
  (substring (param-display-name p) 0 9))

(def compact-control-width () 6.3)
(def compact-control-height () 2.70)
(def group-label-width () 5.4)
(def group-label-gap () 0.25)
(def group-control-gap () 0.22)
(def group-row-gap () 0.18)
(def group-panel-padding () 0.16)
(def group-label-padding () 0.22)

(def compact-button (p fx key)
  (pc/param-mod-wrapper fx p (str key "-mod-wrapper")
    (subtree :key key
      (v-stack :width (compact-control-width)
               :height (compact-control-height) :gap 0.12 :align :center
        (label (compact-label p) :font-size 8.7 :width (compact-control-width)
               :color :dim :bg :transparent)
        (button (if (pc/fx-param-on? p) "ON" "OFF")
          :width 4.2 :height 1.05 :padding 0 :font-size 10.0
          :background-color (if (pc/fx-param-on? p) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
          :color (if (pc/fx-param-on? p) :black :dim)
          :plock-active (if (pc/param-plock-active? fx p) 1 0)
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :on-click |x y r|
            (if fx
              (pc/fx-toggle-effect-value fx p)
              (pc/fx-toggle-instrument-value p)))))))

(def compact-option (p fx key)
  (pc/param-mod-wrapper fx p (str key "-mod-wrapper")
    (subtree :key key
      (v-stack :width (compact-control-width)
               :height (compact-control-height) :gap 0.12 :align :center
        (label (compact-label p) :font-size 8.7 :width (compact-control-width)
               :color :dim :bg :transparent)
        (dropdown :value (pc/fx-param-text-value-for fx p)
          :options (get p :options)
          :on-change (lambda (v) (pc/param-set-option fx p v))
          :plock-active (if (pc/param-plock-active? fx p) 1 0)
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :width 5.9 :height 1.05 :font-size 9.2)))))

(def compact-knob (p fx key)
  (pc/param-mod-wrapper fx p (str key "-mod-wrapper")
    (subtree :key key
      (box :debug-name (str "fx-param-compact-knob-" (get p :name))
        :width (compact-control-width)
        :height (compact-control-height) :padding 0
        (knob-number :label (compact-label p)
          :value (pc/fx-param-value-for fx p)
          :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
          :decimals (if (get p :integer-option-fallback) 0 2)
          :mod-offset (pc/param-mod-offset p)
          :mod-scale (pc/param-mod-scale p)
          :unit (pc/param-control-unit fx p)
          :font-size 10.0 :label-font-size 8.8
          :text-color (pc/param-plock-text-color fx p) :label-color :dim
          :plock-active (if (pc/param-plock-active? fx p) 1 0)
          :plock-default (pc/param-plock-default fx p)
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :width (compact-control-width) :height 2.42
          :on-change (lambda (v)
            (pc/param-set-control-value fx p (param-grid-control-value p v))))))))

(def compact-control (p fx key)
  (let ((p (param-with-resolved-options p)))
    (if (get p :boolean)
      (compact-button p fx key)
      (if (get p :options)
        (compact-option p fx key)
        (compact-knob p fx key)))))

(def adsr-number (p fx key title decimals unit)
  (pc/param-mod-wrapper fx p (str key "-mod-wrapper")
    (subtree :key key
      (box :debug-name (str "fx-param-adsr-number-" title)
           :width 5.2 :height 1.55 :padding 0
        (v-stack :width 5.2 :height :fill :gap 0.0 :align :center
          (label title :font-size 9.0 :color :dim :bg :transparent)
          (number-picker :value (pc/fx-param-value-for fx p)
            :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
            :decimals decimals :unit unit
            :noui true :font-size 10.0
            :text-align :center
            :text-color (pc/param-plock-text-color fx p) :edit-color :yellow
            :plock-active (if (pc/param-plock-active? fx p) 1 0)
            :plock-color-r (pc/param-plock-color-r)
            :plock-color-g (pc/param-plock-color-g)
            :plock-color-b (pc/param-plock-color-b)
            :width 5.0 :height 0.82
            :on-change (lambda (v)
              (pc/param-set-control-value fx p v))))))))

(def adsr-param-editor (params fx env-name key-prefix)
  (let ((attack-p (env-role-param params env-name "attack"))
        (decay-p (env-role-param params env-name "decay"))
        (sustain-p (env-role-param params env-name "sustain"))
        (release-p (env-role-param params env-name "release")))
    (subtree :key key-prefix
      (box :width 23.2 :height :fill
           :background-color :instrument-control-bg
           :border-width 1 :corner-radius 16 :padding 0.16
           :debug-name (str "fx-param-env-" env-name)
        (v-stack :width :fill :height :fill :gap 0.12
          (box :width :fill :height 2.95 :padding 0.08
            (adsr-editor
              :attack (pc/fx-param-value-for fx attack-p)
              :decay (pc/fx-param-value-for fx decay-p)
              :sustain (pc/fx-param-value-for fx sustain-p)
              :release (pc/fx-param-value-for fx release-p)
              :width :fill :height :fill
              :background-color :instrument-control-bg
              :on-change (lambda (env)
                (if (and fx (not (get fx :rack-fx)) (not (get fx :bus-fx)) (not (get fx :midi-fx)))
                  (host-command
                    (if (seq-has-selection?) "set-effect-plock-batch" "set-effect-param-batch")
                    (dict :slot-idx (get fx :slot-idx)
                          :target-node-id (get fx :target-node-id)
                          :updates (list
                            (dict :param-idx (get attack-p :idx) :value (get env :attack))
                            (dict :param-idx (get decay-p :idx) :value (get env :decay))
                            (dict :param-idx (get sustain-p :idx) :value (get env :sustain))
                            (dict :param-idx (get release-p :idx) :value (get env :release)))
                          :commit (not (get env :active))))
                  (do
                    (pc/param-set-control-value fx attack-p (get env :attack))
                    (pc/param-set-control-value fx decay-p (get env :decay))
                    (pc/param-set-control-value fx sustain-p (get env :sustain))
                    (pc/param-set-control-value fx release-p (get env :release)))))))
          (box :width :fill :height 1.58 :padding 0.08
            (h-stack :width :fill :gap 0.20 :align :start
              (adsr-number attack-p fx (str key-prefix "-attack") "atk" 2 false)
              (adsr-number decay-p fx (str key-prefix "-decay") "dec" 2 false)
              (adsr-number sustain-p fx (str key-prefix "-sustain") "sus" 2 false)
              (adsr-number release-p fx (str key-prefix "-release") "rel" 2 false)))
          (box :width :fill :height 0.52 :h-align :center :v-align :center
            (label env-name :font-size 9.4 :color :dim :bg :transparent
                   :debug-name (str "fx-param-env-label-" env-name)))
          (box :width :fill :flex 1))))))

(def group-has-controls? (all-params group-params)
  (> (len (filter |p| (normal-metadata-control? all-params p) group-params)) 0))

(def group-has-visible-panel? (all-params group-name)
  (group-has-controls? all-params
    (filter |p| (in-group? p group-name) all-params)))

(def visible-group-names (all-params groups)
  (filter |group-name| (group-has-visible-panel? all-params group-name) groups))

(def visible-group-controls (all-params group-params)
  (filter |p| (normal-metadata-control? all-params p) group-params))

(def group-row-extra-gap (row-count)
  (if (> row-count 1)
    (* (- row-count 1) (group-row-gap))
    0))

(def group-controls-per-row (controls)
  (let ((control-count (len controls)))
    (max 1
      (if (> control-count 6)
        (ceil (/ control-count 2))
        control-count))))

(def group-control-rows (controls)
  (chunks controls (group-controls-per-row controls)))

(def group-row-panel-height (controls)
  (let ((row-count (len (group-control-rows controls))))
    (+ (* (group-panel-padding) 2)
       (* row-count (compact-control-height))
       (group-row-extra-gap row-count))))

(def group-control-row-width (control-count)
  (if (> control-count 0)
    (+ (* control-count (compact-control-width))
       (* (max (- control-count 1) 0) (group-control-gap)))
    0))

(def group-controls-column-width (controls)
  (reduce |width row|
    (max width (group-control-row-width (len row)))
    0
    (group-control-rows controls)))

(def group-row-panel-width (controls)
  (max
    (+ (* (group-panel-padding) 2)
       (group-label-width)
       (group-label-gap)
       (group-controls-column-width controls))
    (+ (* (group-panel-padding) 2)
       (group-label-width)
       (group-label-gap)
       (compact-control-width))))

(def group-controls-by-name (all-params group-name)
  (visible-group-controls all-params
    (filter |p| (in-group? p group-name) all-params)))

(def left-column-width-for-groups (all-params group-names)
  (reduce |width group-name|
    (max width (group-row-panel-width
                 (group-controls-by-name all-params group-name)))
    0
    group-names))

(def left-column-width (all-params groups ungrouped)
  (max
    (left-column-width-for-groups all-params
      (visible-group-names all-params groups))
    (if (> (len ungrouped) 0)
      (group-row-panel-width
        (visible-group-controls all-params ungrouped))
      0)))

(def group-panel-items (all-params groups)
  (reduce |items group-name|
    (append items (list (dict :label group-name :group group-name :misc false)))
    '()
    (visible-group-names all-params groups)))

(def panel-items (all-params groups ungrouped)
  (let ((group-items (group-panel-items all-params groups)))
    (if (> (len ungrouped) 0)
      (append group-items (list (dict :label "misc" :group false :misc true)))
      group-items)))

(def panel-item-group-params (all-params item)
  (if (get item :misc)
    (filter |p| (not (get p :group)) all-params)
    (filter |p| (in-group? p (get item :group)) all-params)))

(def group-row-panel (all-params groups group-params group-label group-name fx ci)
  (let ((controls (visible-group-controls all-params group-params)))
    (box :width (group-row-panel-width controls)
         :height (group-row-panel-height controls)
         :background-color (panel-bg all-params groups fx group-name)
         :border-width 1 :corner-radius 16 :padding (group-panel-padding)
         :debug-name (str "fx-param-group-" group-label)
         :on-click (panel-select-callback all-params groups fx group-name)
      (h-stack :width :fill :height :fill :gap (group-label-gap) :align :center
        (box :width (group-label-width) :height :fill
             :h-align :start :v-align :center :padding (group-label-padding)
          (label group-label :font-size 11.4 :width 4.8
                 :color :dim :bg :transparent))
        (v-stack :width (group-controls-column-width controls)
                 :height :fill :gap (group-row-gap) :align :start
          (each (group-control-rows controls) |chunk row-idx|
            (h-stack :width :fill :gap (group-control-gap) :align :start
              (each chunk |p pi|
                (compact-control p fx (param-subtree-key fx p (+ ci row-idx)))))))))))

(def panel-item (all-params groups item fx ci)
  (group-row-panel all-params groups
    (panel-item-group-params all-params item)
    (get item :label)
    (get item :group)
    fx
    ci))

(def panel-columns (all-params groups ungrouped fx)
  (h-stack :gap 0.35 :align :start
    (each (chunks (panel-items all-params groups ungrouped) 3) |column col-idx|
      (v-stack :gap 0.16
        (each column |item row-idx|
          (panel-item all-params groups item fx (+ (* col-idx 3) row-idx)))))))

(def selected-env-panel (params groups fx)
  (let ((source (selected-env-source params groups fx)))
    (if source
      (adsr-param-editor params fx (get source :env)
        (str "metadata-selected-env-" (get source :env)))
      (box :width 0 :height 0))))

(def metadata-grid (params fx)
  (let ((visible (visible-params params))
        (groups (group-names visible))
        (ungrouped (filter |p| (not (get p :group)) visible)))
    (box :debug-name "fx-param-metadata-grid"
         :width :fill :height 9.8 :padding 0.525
         :on-click (default-section-callback visible groups fx)
      (h-stack :width :fill :height :fill :gap 1.0 :align :stretch
        (panel-columns visible groups ungrouped fx)
        (selected-env-panel visible groups fx)))))

(def fx-param-grid (params fx)
  (let ((visible (visible-params params)))
    (if (has-metadata? visible)
      (metadata-grid params fx)
      (flat-grid params fx))))
