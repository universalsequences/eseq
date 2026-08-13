;; Phaser-Flanger built-in FX panel (Ableton-inspired).
;;
;; Layout mirrors the device: notch/sweep display on top of the mode buttons
;; (Phaser / Flanger / Doubler) with the per-mode control row underneath
;; (phaser: Stack/Classic circuit plus notches/center/spread/blend; delay
;; modes: their own TIME), then
;; the LFO section (free Hz knob or sync grid + shape), the sweep section
;; (amount / feedback / Ø / stereo), and output (warmth / dry-wet / output).
(module eseq.effects.builtin.phaser-flanger)

(import eseq.effects.param-controls :refer
  (fx-param-numeric-value
   fx-param-on-for?
   fx-param-value-for
   fx-set-effect-value
   fx-toggle-effect-value
   instrument-param-base-value
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
   param-set-control-value
   param-set-option))
(import eseq.effects.param-grid :refer (fx-param-grid))
(import eseq.effects.builtin.filter-core :refer
  (builtin-fx-param
   builtin-fx-set-effect-option
   builtin-fx-filter-mini-number
   builtin-fx-filter-mini-percent))

;; Migration alias (module spec §10): src/ui/state_values/tests.rs evals the
;; analyzer-source builder by its flat name (read-only, alias-covered).
(module-compat-alias builtin-fx-phaser-flanger-source analyzer-source)

(def %orange () (rgba 1.00 0.62 0.25 1.0))
(def %cyan   () (rgba 0.45 0.78 0.95 1.0))

(def analyzer-source (fx)
  (if (get fx :rack-fx)
    (dict :kind :rack-effect :index (get fx :track-idx)
          :rack-slot (get fx :rack-slot) :slot (get fx :slot-idx))
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx)))))

;; Mod-wrapped knobs (same pattern as the Space Echo knobs so amount /
;; feedback / dry-wet pick up modulation rings and plock handling).
(def %knob (fx label-text p decimals)
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
        :width 5.35 :height 3.55 :knob-size 2.85
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def %percent-knob (fx label-text p)
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

(def %mode-button (fx p index label-text)
  (let ((selected (= (round (fx-param-numeric-value p)) index)))
    (button label-text
      :width 4.85 :height 1.15 :padding 0 :font-size 8.5
      :background-color (if selected (%orange) :mixer-control-bg)
      :color (if selected :black :dim)
    :border-color :transparent
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :on-click |x y r| (fx-set-effect-value fx p index))))

(def %mode-row (fx p)
  (h-stack :gap 0.16
    (%mode-button fx p 0 "Phaser")
    (%mode-button fx p 1 "Flanger")
    (%mode-button fx p 2 "Doubler")))

(def %display (fx mode-p circuit-p notches-p center-p spread-p blend-p flt-p dbt-p sync-p rate-p div-p shape-p amount-p stereo-p)
  (phaser-notch
    :width 14.9 :height 3.05
    :source (analyzer-source fx)
    :tap-point :post-fx
    :fft-size 4096 :time-slices 64 :min-db -84 :max-db 0 :smoothing 0.72
    ;; These must be the base-value bindings, not the snapshot :value fields.
    ;; Knob drags update :value-field in place and do not rebuild the panel.
    :mode (instrument-param-base-value mode-p)
    :circuit (instrument-param-base-value circuit-p)
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

(def %notches-control (fx p)
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

(def %circuit-control (fx p)
  (subtree :key "phaser-flanger-circuit-control"
    (h-stack :gap 0.18 :align :center
      (label "circ" :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
      (dropdown :value (get p :text-value)
        :options (get p :options)
        :on-change (lambda (v) (param-set-option fx p v))
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 6.4 :height 1.05 :font-size 9.0))))

(def %mode-controls (fx mode-p circuit-p notches-p center-p spread-p blend-p flt-p dbt-p)
  (let ((m (round (fx-param-numeric-value mode-p))))
    (if (= m 0)
      (v-stack :gap 0.16 :align :center
        (h-stack :gap 0.45 :align :start
          (v-stack :gap 0.22 :align :baseline
            (%notches-control fx notches-p)
            (builtin-fx-filter-mini-percent fx "sprd" spread-p))
          (v-stack :gap 0.22 :align :baseline
            (builtin-fx-filter-mini-number fx "cntr" center-p)
            (builtin-fx-filter-mini-number fx "blnd" blend-p)))
        (%circuit-control fx circuit-p))
      (if (= m 1)
        (builtin-fx-filter-mini-number fx "time" flt-p)
        (builtin-fx-filter-mini-number fx "time" dbt-p)))))

(def %mode-box (fx mode-p circuit-p notches-p center-p spread-p blend-p flt-p dbt-p sync-p rate-p div-p shape-p amount-p stereo-p)
  (box :width 16.0 :height 9.55 :padding 0.36
       :background-color :bg :corner-radius 16
    (v-stack :gap 0.16 :align :center
      (%display fx mode-p circuit-p notches-p center-p spread-p blend-p flt-p dbt-p sync-p rate-p div-p shape-p amount-p stereo-p)
      (%mode-row fx mode-p)
      (%mode-controls fx mode-p circuit-p notches-p center-p spread-p blend-p flt-p dbt-p))))

;; ── LFO section (free/sync rate + shape) ──

(def %sync-button (fx p)
  (button "Sync"
    :width 4.95 :height 0.88 :padding 0 :font-size 8.5
    :background-color (if (fx-param-on-for? fx p) (%orange) :mixer-control-bg)
    :border-color :transparent
    :color (if (fx-param-on-for? fx p) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def %div-button (fx p label-text)
  (button label-text
    :width 2.72 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (%orange) :mixer-control-bg)
    :border-color :transparent
    :color (if (= (get p :text-value) label-text) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def %div-grid (fx p)
  (v-stack :gap 0.11
    (h-stack :gap 0.12
      (%div-button fx p "1/32")
      (%div-button fx p "1/16"))
    (h-stack :gap 0.12
      (%div-button fx p "1/16t")
      (%div-button fx p "1/8"))
    (h-stack :gap 0.12
      (%div-button fx p "1/8t")
      (%div-button fx p "1/8."))
    (h-stack :gap 0.12
      (%div-button fx p "1/4")
      (%div-button fx p "1/4t"))
    (h-stack :gap 0.12
      (%div-button fx p "1/4.")
      (%div-button fx p "1/2"))
    (h-stack :gap 0.12
      (%div-button fx p "1")
      (box :width 2.72 :height 0.82))))

(def %shape-button (fx p label-text)
  (button label-text
    :width 3.85 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (%cyan) :mixer-control-bg)
    :color (if (= (get p :text-value) label-text) :black :dim)
    :border-color :transparent
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def %shape-column (fx p)
  (v-stack :gap 0.11 :align :center
    (%shape-button fx p "sine")
    (%shape-button fx p "triangle")
    (%shape-button fx p "ramp")
    (%shape-button fx p "square")))

(def %lfo-box (fx sync-p rate-p div-p shape-p)
  (box :width 10.6 :height 9.55 :padding 0.30
       :background-color :bg :corner-radius 16
    (v-stack :gap 0.16 :align :center
      (label "LFO" :font-size 8.0 :width 8.6 :color :dim :bg :transparent)
      (%sync-button fx sync-p)
      (h-stack :gap 0.30 :align :start
        (if (fx-param-on-for? fx sync-p)
          (%div-grid fx div-p)
          (v-stack :gap 0.30 :align :center
            (box :height 0.8)
            (%knob fx "freq" rate-p 2)))
        (%shape-column fx shape-p)))))

;; ── Sweep section (amount / feedback / Ø / stereo) ──

(def %invert-button (fx p)
  (button "Ø"
    :width 1.45 :height 1.05 :padding 0 :font-size 9.5
    :background-color (if (fx-param-on-for? fx p) (%orange) :mixer-control-bg)
    :color (if (fx-param-on-for? fx p) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def %sweep-box (fx amount-p feedback-p invert-p stereo-p)
  (box :width 10.0 :height 9.55 :padding 0.36
       :background-color :bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "SWEEP" :font-size 8.0 :width 9.0 :color :dim :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (%percent-knob fx "amount" amount-p)
        (%percent-knob fx "feedback" feedback-p))
      (h-stack :gap 0.30 :align :center
        (label "polarity" :font-size 8.5 :width 3.4 :color :dim :bg :transparent)
        (%invert-button fx invert-p))
      (builtin-fx-filter-mini-number fx "ster" stereo-p))))

;; ── Output section ──

(def %out-box (fx warmth-p mix-p output-p)
  (box :width 5.3 :height 9.55 :padding 0.30
       :background-color :bg :corner-radius 7
    (v-stack :gap 0.10 :align :center
      (label "OUT" :font-size 8.0 :width 4.4 :color :dim :bg :transparent)
      (%knob fx "output" output-p 1)
      (%percent-knob fx "warmth" warmth-p)
      (%percent-knob fx "dry/wet" mix-p))))

(def panel-ui (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (builtin-fx-param params "mode"))
          (circuit-p (builtin-fx-param params "phaser circuit"))
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
      (if (and mode-p circuit-p notches-p center-p spread-p blend-p flt-p dbt-p
               sync-p rate-p div-p shape-p amount-p feedback-p invert-p
               stereo-p warmth-p mix-p output-p)
        (h-stack :gap 0.35 :align :start
          (%mode-box fx mode-p circuit-p notches-p center-p spread-p blend-p flt-p dbt-p sync-p rate-p div-p shape-p amount-p stereo-p)
          (%lfo-box fx sync-p rate-p div-p shape-p)
          (%sweep-box fx amount-p feedback-p invert-p stereo-p)
          (%out-box fx warmth-p mix-p output-p))
        (fx-param-grid params fx)))))
