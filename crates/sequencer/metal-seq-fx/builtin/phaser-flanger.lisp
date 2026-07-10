;; Phaser-Flanger built-in FX panel (Ableton-inspired).
;;
;; Layout mirrors the device: notch/sweep display on top of the mode buttons
;; (Phaser / Flanger / Doubler) with the per-mode control row underneath
;; (phaser: notches/center/spread/blend; delay modes: their own TIME), then
;; the LFO section (free Hz knob or sync grid + shape), the sweep section
;; (amount / feedback / Ø / stereo), and output (warmth / dry-wet / output).

(def phaser-flanger-orange () (rgba 1.00 0.62 0.25 1.0))
(def phaser-flanger-cyan   () (rgba 0.45 0.78 0.95 1.0))

(def builtin-fx-phaser-flanger-source (fx)
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx))))

;; Mod-wrapped knobs (same pattern as the Space Echo knobs so amount /
;; feedback / dry-wet pick up modulation rings and plock handling).
(def builtin-fx-phaser-flanger-knob (fx label-text p decimals)
  (param-mod-wrapper fx p (str "phaser-flanger-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "phaser-flanger-param-" (get p :idx) (param-control-key-mode fx p))
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
        :font-size 9.5 :label-font-size 9.0
        :text-color (param-plock-text-color fx p) :label-color :dim
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.85
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def builtin-fx-phaser-flanger-percent-knob (fx label-text p)
  (param-mod-wrapper fx p (str "phaser-flanger-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "phaser-flanger-param-" (get p :idx) (param-control-key-mode fx p))
      (knob-number :label label-text
        :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :value-scale 100 :decimals 0
        :base-value (param-base-value-prop fx p)
        :base-min (param-base-min-prop fx p) :base-max (param-base-max-prop fx p)
        :mod-range-0-slot (param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 7.5
        :text-color (param-plock-text-color fx p) :label-color :dim
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.85
        :on-change (lambda (v) (param-set-control-value fx p v))))))

;; ── Mode section (display + Phaser/Flanger/Doubler + per-mode controls) ──

(def builtin-fx-phaser-flanger-mode-button (fx p index label-text)
  (let ((selected (= (round (get p :value)) index)))
    (button label-text
      :width 4.85 :height 1.15 :padding 0 :font-size 8.5
      :background-color (if selected (phaser-flanger-orange) :mixer-control-bg)
      :color (if selected :black :dim)
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :on-click |x y r| (fx-set-effect-value fx p index))))

(def builtin-fx-phaser-flanger-mode-row (fx p)
  (h-stack :gap 0.16
    (builtin-fx-phaser-flanger-mode-button fx p 0 "Phaser")
    (builtin-fx-phaser-flanger-mode-button fx p 1 "Flanger")
    (builtin-fx-phaser-flanger-mode-button fx p 2 "Doubler")))

(def builtin-fx-phaser-flanger-display (fx mode-p notches-p center-p spread-p blend-p flt-p dbt-p sync-p rate-p div-p shape-p amount-p stereo-p)
  (phaser-notch
    :width 14.9 :height 3.05
    :source (builtin-fx-phaser-flanger-source fx)
    :tap-point :post-fx
    :fft-size 4096 :time-slices 64 :min-db -84 :max-db 0 :smoothing 0.72
    ;; These must be the base-value bindings, not the snapshot :value fields.
    ;; Knob drags update :value-field in place and do not rebuild the panel.
    :mode (instrument-param-base-value mode-p)
    :notches (instrument-param-base-value notches-p)
    :center (instrument-param-base-value center-p)
    :spread (instrument-param-base-value spread-p)
    :blend (instrument-param-base-value blend-p)
    :flanger-time (instrument-param-base-value flt-p)
    :doubler-time (instrument-param-base-value dbt-p)
    :amount (instrument-param-base-value amount-p)
    :stereo (instrument-param-base-value stereo-p)
    :sync (instrument-param-base-value sync-p)
    :rate (instrument-param-base-value rate-p)
    :sync-div (instrument-param-base-value div-p)
    :lfo-shape (instrument-param-base-value shape-p)
    :bpm (bind-seq "bpm")))

(def builtin-fx-phaser-flanger-notches-control (fx p)
  (param-mod-wrapper fx p (str "phaser-flanger-param-" (get p :idx) "-mod-wrapper")
    (subtree :key "phaser-flanger-notches-control"
      (h-stack :gap 0.18 :align :baseline
        (label "ntch" :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
        (number-picker :value (fx-param-value-for fx p)
          :min 1 :max 12 :step 1 :decimals 0
          :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
          :plock-active (if (param-plock-active? fx p) 1 0)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :on-change (lambda (v) (param-set-control-value fx p (round v)))
          :width 4.6 :height 1.0)))))

(def builtin-fx-phaser-flanger-mode-controls (fx mode-p notches-p center-p spread-p blend-p flt-p dbt-p)
  (let ((m (round (get mode-p :value))))
    (if (= m 0)
      (h-stack :gap 0.45 :align :start
        (v-stack :gap 0.22 :align :baseline
          (builtin-fx-phaser-flanger-notches-control fx notches-p)
          (builtin-fx-filter-mini-percent fx "sprd" spread-p))
        (v-stack :gap 0.22 :align :baseline
          (builtin-fx-filter-mini-number fx "cntr" center-p)
          (builtin-fx-filter-mini-number fx "blnd" blend-p)))
      (if (= m 1)
        (builtin-fx-filter-mini-number fx "time" flt-p)
        (builtin-fx-filter-mini-number fx "time" dbt-p)))))

(def builtin-fx-phaser-flanger-mode-box (fx mode-p notches-p center-p spread-p blend-p flt-p dbt-p sync-p rate-p div-p shape-p amount-p stereo-p)
  (box :width 16.0 :height 8.55 :padding 0.36
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.22 :align :center
      (builtin-fx-phaser-flanger-display fx mode-p notches-p center-p spread-p blend-p flt-p dbt-p sync-p rate-p div-p shape-p amount-p stereo-p)
      (builtin-fx-phaser-flanger-mode-row fx mode-p)
      (builtin-fx-phaser-flanger-mode-controls fx mode-p notches-p center-p spread-p blend-p flt-p dbt-p))))

;; ── LFO section (free/sync rate + shape) ──

(def builtin-fx-phaser-flanger-sync-button (fx p)
  (button "Sync"
    :width 4.95 :height 0.88 :padding 0 :font-size 8.5
    :background-color (if (> (get p :value) 0.5) (phaser-flanger-orange) :mixer-control-bg)
    :color (if (> (get p :value) 0.5) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def builtin-fx-phaser-flanger-div-button (fx p label-text)
  (button label-text
    :width 2.72 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (phaser-flanger-orange) :mixer-control-bg)
    :color (if (= (get p :text-value) label-text) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def builtin-fx-phaser-flanger-div-grid (fx p)
  (v-stack :gap 0.11
    (h-stack :gap 0.12
      (builtin-fx-phaser-flanger-div-button fx p "1/32")
      (builtin-fx-phaser-flanger-div-button fx p "1/16"))
    (h-stack :gap 0.12
      (builtin-fx-phaser-flanger-div-button fx p "1/16t")
      (builtin-fx-phaser-flanger-div-button fx p "1/8"))
    (h-stack :gap 0.12
      (builtin-fx-phaser-flanger-div-button fx p "1/8t")
      (builtin-fx-phaser-flanger-div-button fx p "1/8."))
    (h-stack :gap 0.12
      (builtin-fx-phaser-flanger-div-button fx p "1/4")
      (builtin-fx-phaser-flanger-div-button fx p "1/4t"))
    (h-stack :gap 0.12
      (builtin-fx-phaser-flanger-div-button fx p "1/4.")
      (builtin-fx-phaser-flanger-div-button fx p "1/2"))
    (h-stack :gap 0.12
      (builtin-fx-phaser-flanger-div-button fx p "1")
      (box :width 2.72 :height 0.82))))

(def builtin-fx-phaser-flanger-shape-button (fx p label-text)
  (button label-text
    :width 3.85 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (phaser-flanger-cyan) :mixer-control-bg)
    :color (if (= (get p :text-value) label-text) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def builtin-fx-phaser-flanger-shape-column (fx p)
  (v-stack :gap 0.11 :align :center
    (builtin-fx-phaser-flanger-shape-button fx p "sine")
    (builtin-fx-phaser-flanger-shape-button fx p "triangle")
    (builtin-fx-phaser-flanger-shape-button fx p "ramp")
    (builtin-fx-phaser-flanger-shape-button fx p "square")))

(def builtin-fx-phaser-flanger-lfo-box (fx sync-p rate-p div-p shape-p)
  (box :width 10.6 :height 8.55 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.16 :align :center
      (label "LFO" :font-size 8.0 :width 8.6 :color :dim :bg :transparent)
      (builtin-fx-phaser-flanger-sync-button fx sync-p)
      (h-stack :gap 0.30 :align :start
        (if (> (get sync-p :value) 0.5)
          (builtin-fx-phaser-flanger-div-grid fx div-p)
          (v-stack :gap 0.30 :align :center
            (box :height 0.8)
            (builtin-fx-phaser-flanger-knob fx "freq" rate-p 2)))
        (builtin-fx-phaser-flanger-shape-column fx shape-p)))))

;; ── Sweep section (amount / feedback / Ø / stereo) ──

(def builtin-fx-phaser-flanger-invert-button (fx p)
  (button "Ø"
    :width 1.45 :height 1.05 :padding 0 :font-size 9.5
    :background-color (if (> (get p :value) 0.5) (phaser-flanger-orange) :mixer-control-bg)
    :color (if (> (get p :value) 0.5) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def builtin-fx-phaser-flanger-sweep-box (fx amount-p feedback-p invert-p stereo-p)
  (box :width 10.0 :height 8.55 :padding 0.36
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "SWEEP" :font-size 8.0 :width 9.0 :color :dim :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (builtin-fx-phaser-flanger-percent-knob fx "amount" amount-p)
        (builtin-fx-phaser-flanger-percent-knob fx "feedback" feedback-p))
      (h-stack :gap 0.30 :align :center
        (label "polarity" :font-size 8.5 :width 3.4 :color :dim :bg :transparent)
        (builtin-fx-phaser-flanger-invert-button fx invert-p))
      (builtin-fx-filter-mini-number fx "ster" stereo-p))))

;; ── Output section ──

(def builtin-fx-phaser-flanger-out-box (fx warmth-p mix-p output-p)
  (box :width 5.3 :height 8.55 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.10 :align :center
      (label "OUT" :font-size 8.0 :width 4.4 :color :dim :bg :transparent)
      (builtin-fx-phaser-flanger-knob fx "output" output-p 1)
      (builtin-fx-phaser-flanger-percent-knob fx "warmth" warmth-p)
      (builtin-fx-phaser-flanger-percent-knob fx "dry/wet" mix-p))))

(def builtin-fx-phaser-flanger-ui (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (builtin-fx-param params "mode"))
          (notches-p (builtin-fx-param params "notches"))
          (center-p (builtin-fx-param params "center"))
          (spread-p (builtin-fx-param params "spread"))
          (blend-p (builtin-fx-param params "blend"))
          (flt-p (builtin-fx-param params "flanger time"))
          (dbt-p (builtin-fx-param params "doubler time"))
          (sync-p (builtin-fx-param params "sync"))
          (rate-p (builtin-fx-param params "rate"))
          (div-p (builtin-fx-param params "sync div"))
          (shape-p (builtin-fx-param params "lfo shape"))
          (amount-p (builtin-fx-param params "amount"))
          (feedback-p (builtin-fx-param params "feedback"))
          (invert-p (builtin-fx-param params "fb invert"))
          (stereo-p (builtin-fx-param params "stereo"))
          (warmth-p (builtin-fx-param params "warmth"))
          (mix-p (builtin-fx-param params "dry/wet"))
          (output-p (builtin-fx-param params "output")))
      (if (and mode-p notches-p center-p spread-p blend-p flt-p dbt-p
               sync-p rate-p div-p shape-p amount-p feedback-p invert-p
               stereo-p warmth-p mix-p output-p)
        (h-stack :gap 0.35 :align :start
          (builtin-fx-phaser-flanger-mode-box fx mode-p notches-p center-p spread-p blend-p flt-p dbt-p sync-p rate-p div-p shape-p amount-p stereo-p)
          (builtin-fx-phaser-flanger-lfo-box fx sync-p rate-p div-p shape-p)
          (builtin-fx-phaser-flanger-sweep-box fx amount-p feedback-p invert-p stereo-p)
          (builtin-fx-phaser-flanger-out-box fx warmth-p mix-p output-p))
        (fx-param-grid params fx)))))
