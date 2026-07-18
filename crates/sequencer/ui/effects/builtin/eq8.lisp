;; EQ8 built-in FX panel.

(defstate builtin-fx-eq8-selected-track -1)
(defstate builtin-fx-eq8-selected-bus -1)
(defstate builtin-fx-eq8-selected-rack-slot -1)
(defstate builtin-fx-eq8-selected-slot -1)
(defstate builtin-fx-eq8-selected-band 0)

(def builtin-fx-eq8-track-key (fx)
  (if (get fx :bus-fx) -1 (get fx :track-idx)))

(def builtin-fx-eq8-bus-key (fx)
    (if (get fx :bus-fx) (get fx :bus-idx) -1))

(def builtin-fx-eq8-rack-slot-key (fx)
  (if (get fx :rack-fx) (get fx :rack-slot) -1))

(def builtin-fx-eq8-selected-band-for (fx)
  (if (and (= builtin-fx-eq8-selected-track (builtin-fx-eq8-track-key fx))
           (= builtin-fx-eq8-selected-bus (builtin-fx-eq8-bus-key fx))
           (= builtin-fx-eq8-selected-rack-slot (builtin-fx-eq8-rack-slot-key fx))
           (= builtin-fx-eq8-selected-slot (get fx :slot-idx)))
    builtin-fx-eq8-selected-band
    0))

(def builtin-fx-eq8-select-band (fx band)
  (do
    (set! builtin-fx-eq8-selected-track (builtin-fx-eq8-track-key fx))
    (set! builtin-fx-eq8-selected-bus (builtin-fx-eq8-bus-key fx))
    (set! builtin-fx-eq8-selected-rack-slot (builtin-fx-eq8-rack-slot-key fx))
    (set! builtin-fx-eq8-selected-slot (get fx :slot-idx))
    (set! builtin-fx-eq8-selected-band band)))

(def builtin-fx-eq8-param (params band suffix)
  (builtin-fx-param params (str "b" (+ band 1) " " suffix)))

(def builtin-fx-eq8-band-type (p)
  (if p
    (get p :text-value)
    "bell"))

(def builtin-fx-eq8-band (fx params band)
  (let ((enabled-p (builtin-fx-eq8-param params band "enabled"))
        (type-p (builtin-fx-eq8-param params band "type"))
        (freq-p (builtin-fx-eq8-param params band "freq"))
        (gain-p (builtin-fx-eq8-param params band "gain"))
        (q-p (builtin-fx-eq8-param params band "q"))
        (selected-band (builtin-fx-eq8-selected-band-for fx)))
    (dict
      :id band
      :type (builtin-fx-eq8-band-type type-p)
      :freq (fx-param-value-for fx freq-p)
      :freq-min (param-control-min fx freq-p)
      :freq-max (param-control-max fx freq-p)
      :gain (fx-param-value-for fx gain-p)
      :gain-min (param-control-min fx gain-p)
      :gain-max (param-control-max fx gain-p)
      :q (fx-param-value-for fx q-p)
      :q-min (param-control-min fx q-p)
      :q-max (param-control-max fx q-p)
      :enabled (fx-param-on-for? fx enabled-p)
      :selected (= selected-band band))))

(def builtin-fx-eq8-bands (fx params)
  (map |band| (builtin-fx-eq8-band fx params band) (range 8)))

(def builtin-fx-eq8-source (fx)
  (if (get fx :rack-fx)
    (dict :kind :rack-effect :index (get fx :track-idx)
          :rack-slot (get fx :rack-slot) :slot (get fx :slot-idx))
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx)))))

(def builtin-fx-eq8-set-band-values (fx params band freq gain q)
  (let ((freq-p (builtin-fx-eq8-param params band "freq"))
        (gain-p (builtin-fx-eq8-param params band "gain"))
        (q-p (builtin-fx-eq8-param params band "q")))
    (do
      (fx-set-effect-value fx freq-p freq)
      (fx-set-effect-value fx gain-p gain)
      (fx-set-effect-value fx q-p q))))

(def builtin-fx-eq8-handle-action (fx params event)
  (let ((type (get event :type))
        (band (get event :id)))
    (if (= type :select-band)
      (builtin-fx-eq8-select-band fx band)
      (if (= type :toggle-band)
        (do
          (builtin-fx-eq8-select-band fx band)
          (fx-set-effect-value fx (builtin-fx-eq8-param params band "enabled")
            (if (get event :enabled) 1 0)))
        (if (or (= type :change-band) (= type :commit-band))
          (do
            (builtin-fx-eq8-select-band fx band)
            (builtin-fx-eq8-set-band-values fx params band
              (get event :freq)
              (get event :gain)
              (get event :q)))
          nil)))))

(def builtin-fx-eq8-band-button (fx params band)
  (let ((band-map (builtin-fx-eq8-band fx params band))
      (enabled-p (builtin-fx-eq8-param params band "enabled"))
      (selected (= (builtin-fx-eq8-selected-band-for fx) band)))
    (h-stack :gap 0.08 :align :center
      (button (str (+ band 1))
        :width 1.65 :height 1.05 :padding 0 :font-size 8.8
        :background-color (if selected (rgba 1.0 0.74 0.22 1.0) :mixer-control-bg)
        :border-color :transparent
        :color (if selected :black (if (get band-map :enabled) :fg :dim))
        :on-click |x y r| (builtin-fx-eq8-select-band fx band))
      (button (if (get band-map :enabled) "on" "off")
        :border-color :transparent
        :width 2.35 :height 1.05 :padding 0 :font-size 8.0
        :background-color (if (get band-map :enabled) (rgba 0.92 0.35 0.12 1.0) :mixer-control-bg)
        :color (if (get band-map :enabled) :white :dim)
        :plock-active (if (param-plock-active? fx enabled-p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :on-click |x y r| (do
          (builtin-fx-eq8-select-band fx band)
          (fx-toggle-effect-value fx enabled-p))))))

(def builtin-fx-eq8-selected-knob (fx label-text p decimals)
  (param-mod-wrapper fx p (str "eq8-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "eq8-param-" (get p :idx) (param-control-key-mode fx p))
      (knob-number :label label-text
        :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals decimals
        :base-value (param-base-value-prop fx p)
        :base-min (param-base-min-prop fx p) :base-max (param-base-max-prop fx p)
        :mod-range-0-slot (param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (param-selected-mod-slot-prop fx p)
        :font-size 8.8 :label-font-size 8.6
        :text-color (param-plock-text-color fx p) :label-color :dim
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.45 :height 3.48 :knob-size 1.65
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def builtin-fx-eq8-selected-knobs (fx params)
  (let ((band (builtin-fx-eq8-selected-band-for fx)))
    (let ((freq-p (builtin-fx-eq8-param params band "freq"))
          (q-p (builtin-fx-eq8-param params band "q")))
      (box :width 4.9 :height 7.05 :padding 0.22
        :background-color :fx-inner-panel-bg :corner-radius 7
        (v-stack :gap 0.22 :align :center
          (builtin-fx-eq8-selected-knob fx "freq" freq-p 0)
          (builtin-fx-eq8-selected-knob fx "q" q-p 2))))))

(def builtin-fx-eq8-selected-controls (fx params)
  (let ((band (builtin-fx-eq8-selected-band-for fx)))
    (let ((type-p (builtin-fx-eq8-param params band "type"))
          (freq-p (builtin-fx-eq8-param params band "freq"))
          (gain-p (builtin-fx-eq8-param params band "gain"))
          (q-p (builtin-fx-eq8-param params band "q")))
      (box :width 43.2 :height 1.65 :padding 0.24
        :background-color :fx-inner-panel-bg :corner-radius 7
        (h-stack :gap 0.44 :align :center
          (label (str "B" (+ band 1)) :font-size 9.5 :width 2.0 :color :dim :bg :transparent)
          (dropdown :value (get type-p :text-value)
            :options (get type-p :options)
            :on-change (lambda (v) (builtin-fx-set-effect-option fx type-p v))
            :plock-active (if (param-plock-active? fx type-p) 1 0)
            :plock-color-r (param-plock-color-r)
            :plock-color-g (param-plock-color-g)
            :plock-color-b (param-plock-color-b)
            :width 6.6 :height 1.05 :font-size 9.0)
          (builtin-fx-filter-mini-number fx "freq" freq-p)
          (builtin-fx-filter-mini-number fx "gain" gain-p)
          (builtin-fx-filter-mini-number fx "q" q-p))))))

(def builtin-fx-eq8-ui (fx)
  (let ((params (get fx :params)))
    (if (= (len params) 41)
      (v-stack :gap 0.020
        (h-stack :gap 0.25 :align :start
          (builtin-fx-eq8-selected-knobs fx params)
          (box :width 38.05 :height 7.05
            (eq8-editor
              :width 38.05 :height 7.05
              :bands (builtin-fx-eq8-bands fx params)
              :selected-band (builtin-fx-eq8-selected-band-for fx)
              :source (builtin-fx-eq8-source fx)
              :tap-point :post-fx
              :mode :eq
              :fft-size 8192
              :time-slices 128
              :min-db -96
              :max-db 0
              :smoothing 0.65
              :background-color (rgba 0.045 0.048 0.052 1.0)
              :curve-color (rgba 1.0 0.54 0.14 1.0)
              :selected-color (rgba 1.0 0.78 0.18 1.0)
              :spectrum-color (rgba 0.08 0.52 0.54 0.30)
              :spectrum-peak-color (rgba 0.40 0.92 0.86 0.74)
              :on-action |event| (builtin-fx-eq8-handle-action fx params event))))
        (box :width 43.2 :heighl 1.02 :padding 0.22
          :background-color :fx-inner-panel-bg :corner-radius 7
          (h-stack :gap 0.18 :align :center
            (builtin-fx-eq8-band-button fx params 0)
            (builtin-fx-eq8-band-button fx params 1)
            (builtin-fx-eq8-band-button fx params 2)
            (builtin-fx-eq8-band-button fx params 3)
            (builtin-fx-eq8-band-button fx params 4)
            (builtin-fx-eq8-band-button fx params 5)
            (builtin-fx-eq8-band-button fx params 6)
            (builtin-fx-eq8-band-button fx params 7)))
        (builtin-fx-eq8-selected-controls fx params))
      (fx-param-grid params fx))))
