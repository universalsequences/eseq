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
              (box :width 5.5 :height 1.25 :align :center
                   :bg :transparent
                   :on-click |x y r|
                     (if fx
                       (fx-toggle-effect-value fx p)
                       (fx-toggle-instrument-value p))
                (label (if (fx-param-on? p) "ON" "OFF")
                       :font-size 11 :width 5.5
                       :color :white :bg :transparent))
              (if (get p :options)
              (dropdown :value (get p :text-value)
                :options (get p :options)
                :on-change (lambda (v) (param-set-option fx p v))
                :width 5.8 :height 1.2 :font-size 11)
              (number-picker :value (fx-param-value-for fx p)
                :min (param-control-min fx p) :max (param-control-max fx p) :decimals 2
                :noui true :font-size 12 :text-color :dim
                :on-change (lambda (v)
                  (param-set-control-value fx p v))
                :width 5.2 :height 1.1)))))
        (if (or (get p :options) (get p :boolean))
          (label "" :width 7.8 :bg :transparent)
          (hslider :width 7.8 :min (param-control-min fx p) :max (param-control-max fx p)
                   :value (fx-param-value-for fx p)
                   :material (aqua-slider-material)
                   :on-change (lambda (v)
                     (param-set-control-value fx p v)))))))))

(def fx-param-grid (params fx)
  (h-stack :gap 1.5 :padding 0.525
    (each (chunks (visible-params params) 4) |chunk ci|
      (v-stack :gap 0.25
        (each chunk |p pi|
          (fx-param-row p fx
            (if fx
              (if (get fx :midi-fx)
                (str "midi-fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))
                (if (get fx :bus-fx)
                  (str "bus-fx-slot-" (get fx :bus-idx) "-" (get fx :slot-idx) "-param-" (get p :idx))
                  (str "fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))))
              (str "instrument-tab-" instrument-panel-tab "-chunk-" ci "-param-" (get p :idx)))))))))
