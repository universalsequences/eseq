;; EQ8 built-in FX panel.

(module eseq.effects.builtin.eq8)

(import eseq.effects.builtin.filter-core :refer
  (builtin-fx-param
   builtin-fx-filter-mini-number
   builtin-fx-set-effect-option))
(import eseq.effects.param-controls :refer
  (fx-param-on-for?
   fx-param-value-for
   fx-set-effect-value
   fx-toggle-effect-value
   param-base-max-prop
   param-base-min-prop
   param-base-value-prop
   param-control-key-mode
   param-control-max
   param-control-min
   param-knob-mod-depth-prop
   param-knob-mod-slot-prop
   param-mod-wrapper
   param-plock-active?
   param-plock-color-b
   param-plock-color-g
   param-plock-color-r
   param-plock-default
   param-plock-text-color
   param-selected-mod-slot-prop
   param-set-control-value))
(import eseq.effects.param-grid :refer (fx-param-grid))

(export eq8-source
        eq8-ui)

;; Migration alias (module spec §10). `builtin-fx-eq8-source` is evaled by
;; name from src/ui/state_values/tests.rs (read-only, alias-covered).

(defstate selected-track -1)
(defstate selected-bus -1)
(defstate selected-rack-slot -1)
(defstate selected-slot -1)
(defstate selected-band 0)

(def track-key (fx)
  (if (get fx :bus-fx) -1 (get fx :track-idx)))

(def bus-key (fx)
    (if (get fx :bus-fx) (get fx :bus-idx) -1))

(def rack-slot-key (fx)
  (if (get fx :rack-fx) (get fx :rack-slot) -1))

(def selected-band-for (fx)
  (if (and (= selected-track (track-key fx))
           (= selected-bus (bus-key fx))
           (= selected-rack-slot (rack-slot-key fx))
           (= selected-slot (get fx :slot-idx)))
    selected-band
    0))

(def select-band (fx band)
  (do
    (set! selected-track (track-key fx))
    (set! selected-bus (bus-key fx))
    (set! selected-rack-slot (rack-slot-key fx))
    (set! selected-slot (get fx :slot-idx))
    (set! selected-band band)))

(def param (params band suffix)
  (eseq.effects.builtin.filter-core/builtin-fx-param params (str "b" (+ band 1) " " suffix)))

(def band-type (p)
  (if p
    (get p :text-value)
    "bell"))

(def band-data (fx params band)
  (let ((enabled-p (param params band "enabled"))
        (type-p (param params band "type"))
        (freq-p (param params band "freq"))
        (gain-p (param params band "gain"))
        (q-p (param params band "q"))
        (selected-band (selected-band-for fx)))
    (dict
      :id band
      :type (band-type type-p)
      :freq (eseq.effects.param-controls/fx-param-value-for fx freq-p)
      :freq-min (eseq.effects.param-controls/param-control-min fx freq-p)
      :freq-max (eseq.effects.param-controls/param-control-max fx freq-p)
      :gain (eseq.effects.param-controls/fx-param-value-for fx gain-p)
      :gain-min (eseq.effects.param-controls/param-control-min fx gain-p)
      :gain-max (eseq.effects.param-controls/param-control-max fx gain-p)
      :q (eseq.effects.param-controls/fx-param-value-for fx q-p)
      :q-min (eseq.effects.param-controls/param-control-min fx q-p)
      :q-max (eseq.effects.param-controls/param-control-max fx q-p)
      :enabled (eseq.effects.param-controls/fx-param-on-for? fx enabled-p)
      :selected (= selected-band band))))

(def bands (fx params)
  (map |band| (band-data fx params band) (range 8)))

(def eq8-source (fx)
  (if (get fx :rack-fx)
    (dict :kind :rack-effect :index (get fx :track-idx)
          :rack-slot (get fx :rack-slot) :slot (get fx :slot-idx))
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx)))))

(def set-band-values (fx params band freq gain q)
  (let ((freq-p (param params band "freq"))
        (gain-p (param params band "gain"))
        (q-p (param params band "q")))
    (do
      (eseq.effects.param-controls/fx-set-effect-value fx freq-p freq)
      (eseq.effects.param-controls/fx-set-effect-value fx gain-p gain)
      (eseq.effects.param-controls/fx-set-effect-value fx q-p q))))

(def handle-action (fx params event)
  (let ((type (get event :type))
        (band (get event :id)))
    (if (= type :select-band)
      (select-band fx band)
      (if (= type :toggle-band)
        (do
          (select-band fx band)
          (eseq.effects.param-controls/fx-set-effect-value fx (param params band "enabled")
            (if (get event :enabled) 1 0)))
        (if (or (= type :change-band) (= type :commit-band))
          (do
            (select-band fx band)
            (set-band-values fx params band
              (get event :freq)
              (get event :gain)
              (get event :q)))
          nil)))))

(def band-button (fx params band)
  (let ((band-map (band-data fx params band))
      (enabled-p (param params band "enabled"))
      (selected (= (selected-band-for fx) band)))
    (h-stack :gap 0.08 :align :center
      (button (str (+ band 1))
        :width 1.65 :height 1.05 :padding 0 :font-size 8.8
        :background-color (if selected (rgba 1.0 0.74 0.22 1.0) :mixer-control-bg)
        :border-color :transparent
        :color (if selected :black (if (get band-map :enabled) :fg :dim))
        :on-click |x y r| (select-band fx band))
      (button (if (get band-map :enabled) "on" "off")
        :border-color :transparent
        :width 2.35 :height 1.05 :padding 0 :font-size 8.0
        :background-color (if (get band-map :enabled) (rgba 0.92 0.35 0.12 1.0) :mixer-control-bg)
        :color (if (get band-map :enabled) :white :dim)
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx enabled-p) 1 0)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :on-click |x y r| (do
          (select-band fx band)
          (eseq.effects.param-controls/fx-toggle-effect-value fx enabled-p))))))

(def selected-knob (fx label-text p decimals)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "eq8-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "eq8-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (eseq.effects.param-controls/fx-param-value-for fx p)
        :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :decimals decimals
        :base-value (eseq.effects.param-controls/param-base-value-prop fx p)
        :base-min (eseq.effects.param-controls/param-base-min-prop fx p) :base-max (eseq.effects.param-controls/param-base-max-prop fx p)
        :mod-range-0-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (eseq.effects.param-controls/param-selected-mod-slot-prop fx p)
        :font-size 8.8 :label-font-size 8.6
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.45 :height 3.48 :knob-size 1.65
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

(def selected-knobs (fx params)
  (let ((band (selected-band-for fx)))
    (let ((freq-p (param params band "freq"))
          (q-p (param params band "q")))
      (box :width 4.9 :height 7.05 :padding 0.22
        :background-color :fx-inner-panel-bg :corner-radius 7
        (v-stack :gap 0.22 :align :center
          (selected-knob fx "freq" freq-p 0)
          (selected-knob fx "q" q-p 2))))))

(def selected-controls (fx params)
  (let ((band (selected-band-for fx)))
    (let ((type-p (param params band "type"))
          (freq-p (param params band "freq"))
          (gain-p (param params band "gain"))
          (q-p (param params band "q")))
      (box :width 43.2 :height 1.65 :padding 0.24
        :background-color :fx-inner-panel-bg :corner-radius 7
        (h-stack :gap 0.44 :align :center
          (label (str "B" (+ band 1)) :font-size 9.5 :width 2.0 :color :dim :bg :transparent)
          (dropdown :value (get type-p :text-value)
            :options (get type-p :options)
            :on-change (lambda (v) (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx type-p v))
            :plock-active (if (eseq.effects.param-controls/param-plock-active? fx type-p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :width 6.6 :height 1.05 :font-size 9.0)
          (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "freq" freq-p)
          (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "gain" gain-p)
          (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "q" q-p))))))

(def eq8-ui (fx)
  (let ((params (get fx :params)))
    (if (= (len params) 41)
      (v-stack :gap 0.020
        (h-stack :gap 0.25 :align :start
          (selected-knobs fx params)
          (box :width 38.05 :height 7.05
            (eq8-editor
              :width 38.05 :height 7.05
              :bands (bands fx params)
              :selected-band (selected-band-for fx)
              :source (eq8-source fx)
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
              :on-action |event| (handle-action fx params event))))
        (box :width 43.2 :heighl 1.02 :padding 0.22
          :background-color :fx-inner-panel-bg :corner-radius 7
          (h-stack :gap 0.18 :align :center
            (band-button fx params 0)
            (band-button fx params 1)
            (band-button fx params 2)
            (band-button fx params 3)
            (band-button fx params 4)
            (band-button fx params 5)
            (band-button fx params 6)
            (band-button fx params 7)))
        (selected-controls fx params))
      (eseq.effects.param-grid/fx-param-grid params fx))))
