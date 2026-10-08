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
(import eseq.effects.panel-frame :refer (fx-clear-selected-effect))
(import eseq.effects.devices :as dv)

(export eq8-source
        eq8-ui)

;; `eq8-source` is evaled by name from src/ui/state_values/tests.rs.

;; The selected band (0-7) of the one EQ8 whose band is picked (its
;; device): every other EQ8 shows band 0.
(def-kind eq8-view
  :key ()
  :state ((device :any :default nil)
          (band 0)))

(def selected-band-for (fx)
  (let ((d (dv/fx-device fx)))
    (if (and d (= eq8-view.device d)) eq8-view.band 0)))

(def select-band (fx band)
  (set! eq8-view.device (dv/fx-device fx))
  (set! eq8-view.band band))

(def param (params band suffix)
  (eseq.effects.builtin.filter-core/builtin-fx-param params (str "b" (+ band 1) " " suffix)))

(def band-type (fx p)
  (if p
    (eseq.effects.param-controls/fx-param-text-value-for fx p)
    "bell"))

;; Band `band`'s editor dict (`selected-band`: the panel's selected band).
(def band-data (fx params band selected-band)
  (let ((enabled-p (param params band "enabled"))
        (type-p (param params band "type"))
        (freq-p (param params band "freq"))
        (gain-p (param params band "gain"))
        (q-p (param params band "q")))
    (dict
      :id band
      :type (band-type fx type-p)
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

(def bands (fx params selected-band)
  (map |band| (band-data fx params band selected-band) (range 8)))

;; The spectrum the editor draws: fx's device's meter selector (its output),
;; else one built from the dict's address.
(def eq8-source (fx)
  (let ((d (dv/fx-device fx)))
    (if d d.meter (dict-source fx))))

(def dict-source (fx)
  (if (get fx :rack-fx)
    (dict :kind :rack-effect :index (get fx :track-idx)
          :rack-slot (get fx :rack-slot) :slot (get fx :slot-idx))
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx)))))

(def set-band-values (fx params band freq gain q commit)
  (let ((freq-p (param params band "freq"))
        (gain-p (param params band "gain"))
        (q-p (param params band "q")))
    (if (get fx :rack-fx)
      (do
        (eseq.effects.panel-frame/fx-clear-selected-effect)
        (host-command
          (if (seq-has-selection?) "set-rack-slot-effect-plock-batch" "set-rack-slot-effect-param-batch")
          (dict :track (get fx :track-idx)
                :rack-slot (get fx :rack-slot)
                :effect-slot (get fx :slot-idx)
                :updates (eseq.effects.param-controls/effect-param-updates fx
                  (list (list freq-p freq) (list gain-p gain) (list q-p q)))
                :commit commit)))
      (do
        (eseq.effects.param-controls/fx-set-effect-value fx freq-p freq)
        (eseq.effects.param-controls/fx-set-effect-value fx gain-p gain)
        (eseq.effects.param-controls/fx-set-effect-value fx q-p q)))))

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
              (get event :q)
              (= type :commit-band)))
          nil)))))

(def band-button (fx params band selected-band)
  (let ((band-map (band-data fx params band selected-band))
        (enabled-p (param params band "enabled"))
        (selected (= selected-band band)))
    (h-stack :gap 1.58 :align :center
      (box 
        :selected selected
        :selected-background-color :mixer-strip-selected-bg
        :width 5 :padding 0.2 :background-color :mixer-control-bg
        (h-stack :gap 0.18
          (button (str (+ band 1))
            :width 1.05 :height 0.55 :padding 0 :font-size 6.8
            :background-color (if selected :eq8-band-selected :mixer-control-bg)
            :border-color :transparent
            :corner-radius 0
            :color (if selected :black (if (get band-map :enabled) :fg :dim))
            :on-click |x y r| (select-band fx band))
          (button (if (get band-map :enabled) "on" "off")
            :border-color :transparent
            :corner-radius 6
            :width 2.15 :height 0.85 :padding 0 :font-size 8.0
            :background-color :transparent
            :active-color :eq8-active-text
            :color :dim
            :active (get band-map :enabled)
            :plock-active (if (eseq.effects.param-controls/param-plock-active? fx enabled-p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-click |x y r|
              (do
                (select-band fx band)
                (eseq.effects.param-controls/fx-toggle-effect-value fx enabled-p))))))
    ))

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
        :track-color :mixer-strip-bg
        :font-size 8.8 :label-font-size 8.6
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.45 :height 3.48 :knob-size 2.65
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

(def selected-knobs (fx params band)
  (box :width 4.9 :height 7.05 :padding 0.22
    :background-color :mixer-control-bg :corner-radius 12
    (v-stack :gap 0.22 :align :center
      (selected-knob fx "freq" (param params band "freq") 0)
      (selected-knob fx "q" (param params band "q") 2))))

(def selected-controls (fx params band)
  (let ((type-p (param params band "type")))
    (box :width 43.2 :height 1.65 :padding 0.24
      :corner-radius 7
      (h-stack :gap 0.44 :align :baseline
        (dropdown :value (eseq.effects.param-controls/param-option-label fx type-p)
        :value-index (eseq.effects.param-controls/param-option-index fx type-p)
          :options (get type-p :options)
          :on-change (lambda (v) (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx type-p v))
          :bg-color :mixer-strip-bg
          :border-color :buffer-bg
          :badge-color :eq8-badge-bg
          :chevron-color :black
          :plock-active (if (eseq.effects.param-controls/param-plock-active? fx type-p) 1 0)
          :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
          :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
          :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
          :width 6.6 :height 1.05 :font-size 9.0)
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "freq" (param params band "freq"))
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "gain" (param params band "gain"))
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "q" (param params band "q"))))))

(def eq8-ui (fx)
  (let ((params (get fx :params))
        (selected-band (selected-band-for fx)))
    (if (= (len params) 41)
      (v-stack :gap 0.020
        :padding 0.1
        (h-stack :gap 0.25 :align :start
          (selected-knobs fx params selected-band)
          (box :width 38.05 :height 6.85
            (eq8-editor
              :width 38.05 :height 6.85
              :bands (bands fx params selected-band)
              :selected-band selected-band
              :source (eq8-source fx)
              :tap-point :post-fx
              :mode :eq
              :fft-size 8192
              :time-slices 128
              :min-db -96
              :max-db 0
              :smoothing 0.65
              :background-color :mixer-control-bg
              :curve-color :eq8-curve
              :selected-color :eq8-selected
              :spectrum-color :eq8-spectrum
              :spectrum-peak-color :eq8-spectrum-peak
              :on-action |event| (handle-action fx params event))))
        (box :background-color :mixer-control-bg
          :corner-radius 12
          :padding 0.2
          (v-stack
            (box :width 43.2 :height 1.02 :padding 0.22
              :corner-radius 7
              (h-stack  :align :center :gap 0.4
                (band-button fx params 0 selected-band)
                (band-button fx params 1 selected-band)
                (band-button fx params 2 selected-band)
                (band-button fx params 3 selected-band)
                (band-button fx params 4 selected-band)
                (band-button fx params 5 selected-band)
                (band-button fx params 6 selected-band)
                (band-button fx params 7 selected-band)))
            (selected-controls fx params selected-band))))
      (eseq.effects.param-grid/fx-param-grid params fx))
    ))
