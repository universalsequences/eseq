;; Space Echo (RE-201) built-in FX panel.
;;
;; Layout mirrors the hardware: REPEAT RATE on the left (sync grid or free
;; knob), the 12-position MODE SELECTOR in the middle, then the echo section
;; (intensity / echo volume / bass / treble) and the tape+reverb section.
;; Palette: dark chassis, RE-green selector, signal-orange repeat controls.
(module eseq.effects.builtin.space-echo)

(import eseq.effects.param-controls :refer
  (fx-param-numeric-value
   fx-param-on-for?
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
(import eseq.effects.builtin.filter-core :refer
  (builtin-fx-param
   builtin-fx-set-effect-option
   builtin-fx-filter-mini-number
   builtin-fx-filter-mini-percent))

(export panel-ui)

(def orange () (rgba 1.00 0.62 0.25 1.0))
(def green  () (rgba 0.36 0.80 0.50 1.0))
(def cream  () (rgba 0.93 0.88 0.78 1.0))

;; Mod-wrapped knob (same pattern as the Str8 Delay knobs, so intensity /
;; rate / volumes pick up modulation rings and plock handling).
(def parameter-knob (fx label-text p decimals)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "space-echo-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "space-echo-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
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
        :font-size 9.5 :label-font-size 9.0
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.85
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

(def percent-knob (fx label-text p)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "space-echo-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "space-echo-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (eseq.effects.param-controls/fx-param-value-for fx p)
        :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :value-scale 100 :decimals 0
        :base-value (eseq.effects.param-controls/param-base-value-prop fx p)
        :base-min (eseq.effects.param-controls/param-base-min-prop fx p) :base-max (eseq.effects.param-controls/param-base-max-prop fx p)
        :mod-range-0-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (eseq.effects.param-controls/param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.0
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.85
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

;; ── Repeat rate (sync grid / free knob) ──

(def sync-button (fx p)
  (button "Sync"
    :width 4.95 :height 0.88 :padding 0 :font-size 8.5
    :background-color (if (eseq.effects.param-controls/fx-param-on-for? fx p) (orange) :mixer-control-bg)
    :color (if (eseq.effects.param-controls/fx-param-on-for? fx p) :black :dim)
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :on-click |x y r| (eseq.effects.param-controls/fx-toggle-effect-value fx p)))

(def div-button (fx p current label-text)
  (button label-text
    :width 2.72 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= current label-text) (orange) :mixer-control-bg)
    :color (if (= current label-text) :black :dim)
    :border-color :transparent
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :on-click |x y r| (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx p label-text)))

(def div-grid (fx p)
  (subtree :key (eseq.effects.builtin.filter-core/builtin-fx-param-subtree-key fx p "space-echo-div")
    (let ((current (eseq.effects.param-controls/fx-param-text-value-for fx p)))
      (v-stack :gap 0.11
        (h-stack :gap 0.12
          (div-button fx p current "1/32")
          (div-button fx p current "1/16"))
        (h-stack :gap 0.12
          (div-button fx p current "1/16t")
          (div-button fx p current "1/8"))
        (h-stack :gap 0.12
          (div-button fx p current "1/8t")
          (div-button fx p current "1/8."))
        (h-stack :gap 0.12
          (div-button fx p current "1/4")
          (div-button fx p current "1/4t"))
        (h-stack :gap 0.12
          (div-button fx p current "1/4.")
          (div-button fx p current "1/2"))
        (h-stack :gap 0.12
          (div-button fx p current "1")
          (box :width 2.72 :height 0.82))))))

(def rate-box (fx sync-p div-p offset-p rate-p)
  (box :width 8.25 :height 9.75 :padding 0.18
    :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (label "REPEAT RATE" :font-size 8.0 :width 5.4 :color :dim :bg :transparent)
      (box :height 0.5)
      (sync-button fx sync-p)
      (if (eseq.effects.param-controls/fx-param-on-for? fx sync-p)
        (v-stack :gap 0.12 :align :center
          (div-grid fx div-p)
          (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "ofs" offset-p))
        (v-stack :gap 0.30 :align :center
          (box :height 0.8)
          (percent-knob fx "rate" rate-p))))))

;; ── Mode selector ──

(def mode-button (fx p index short-label)
  (let ((selected (= (round (eseq.effects.param-controls/fx-param-numeric-value p)) index))
      (reverb-mode (> index 5)))
    (button short-label
      :width 3.0 :height 1.30 :padding 0 :font-size 8.5
      :background-color (if selected
        (if reverb-mode (green) (orange))
        :mixer-control-bg)
      :color (if selected :black :dim)
      :border-color :transparent
      :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
      :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
      :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
      :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
      :on-click |x y r| (eseq.effects.param-controls/fx-set-effect-value fx p index))))

(def mode-grid (fx p)
  (v-stack :gap 0.14
    (h-stack :gap 0.14
      (mode-button fx p 0 "1")
      (mode-button fx p 1 "2")
      (mode-button fx p 2 "3"))
    (h-stack :gap 0.14
      (mode-button fx p 3 "1+2")
      (mode-button fx p 4 "2+3")
      (mode-button fx p 5 "all"))
    (h-stack :gap 0.14
      (mode-button fx p 6 "1R")
      (mode-button fx p 7 "2R")
      (mode-button fx p 8 "3R"))
    (h-stack :gap 0.14
      (mode-button fx p 9 "1+2R")
      (mode-button fx p 10 "2+3R")
      (mode-button fx p 11 "rev"))))

(def mode-box (fx mode-p)
  (box :width 10.5 :height 8.85 :padding 0.30
    :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.22 :align :center
      (label "MODE SELECTOR" :font-size 8.0 :width 8.2 :color :dim :bg :transparent)
      (box :height 0.25)
      (mode-grid fx mode-p)
      (label (get mode-p :text-value)
        :font-size 8.5 :width 8.2 :color (cream) :bg :transparent))))

;; ── Echo section ──

(def echo-box (fx intensity-p echo-p bass-p treble-p width-p)
  (box :width 10.4 :height 9.75 :padding 0.36
    :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "ECHO" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (percent-knob fx "intensity" intensity-p)
        (parameter-knob fx "echo vol" echo-p 2))
      (box :height 0.85)
      (label "TONE" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (v-stack :gap 0.30 :align :baseline
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "bass" bass-p)
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "treb" treble-p)
        (if width-p
          (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "wide" width-p)
          (box :height 0.1))))))

;; ── Tape + reverb section ──

;; Spring type selector (which physical tank the model is tuned to).
(def spring-button (fx p index short-label)
  (let ((selected (= (round (eseq.effects.param-controls/fx-param-numeric-value p)) index)))
    (button short-label
      :width 4.35 :height 0.92 :padding 0 :font-size 8.0
      :background-color (if selected (green) :mixer-control-bg)
      :border-color :transparent
      :color (if selected :black :dim)
      :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
      :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
      :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
      :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
      :on-click |x y r| (eseq.effects.param-controls/fx-set-effect-value fx p index))))

(def spring-row (fx p)
  (v-stack
    (h-stack :gap 0.14
      (spring-button fx p 0 "RE-201")
      (spring-button fx p 1 "Tubby")))
  )

(def tape-box (fx reverb-p tension-p spring-p wf-p age-p drive-p dry-p)
  (box :width 10.4 :height 9.75 :padding 0.36
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.16 :align :center
      (label "REVERB" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (parameter-knob fx "reverb vol" reverb-p 2)
        (if tension-p
          (percent-knob fx "tension" tension-p)
          (box :width 4.35 :height 2.45)))
      (if spring-p
        (spring-row fx spring-p)
        (box :height 0.1))
      (label "TAPE" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "w/f" wf-p)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "age" age-p)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "drv" drive-p)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "dry" dry-p))))

(def panel-ui (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mode"))
          (rate-p (eseq.effects.builtin.filter-core/builtin-fx-param params "repeat rate"))
          (sync-p (eseq.effects.builtin.filter-core/builtin-fx-param params "sync"))
          (div-p (eseq.effects.builtin.filter-core/builtin-fx-param params "sync div"))
          (offset-p (eseq.effects.builtin.filter-core/builtin-fx-param params "sync offset"))
          (intensity-p (eseq.effects.builtin.filter-core/builtin-fx-param params "intensity"))
          (bass-p (eseq.effects.builtin.filter-core/builtin-fx-param params "bass"))
          (treble-p (eseq.effects.builtin.filter-core/builtin-fx-param params "treble"))
          (echo-p (eseq.effects.builtin.filter-core/builtin-fx-param params "echo volume"))
          (reverb-p (eseq.effects.builtin.filter-core/builtin-fx-param params "reverb volume"))
          (tension-p (eseq.effects.builtin.filter-core/builtin-fx-param params "tension"))
          (spring-p (eseq.effects.builtin.filter-core/builtin-fx-param params "spring type"))
          (width-p (eseq.effects.builtin.filter-core/builtin-fx-param params "stereo width"))
          (dry-p (eseq.effects.builtin.filter-core/builtin-fx-param params "dry"))
          (drive-p (eseq.effects.builtin.filter-core/builtin-fx-param params "input drive"))
          (wf-p (eseq.effects.builtin.filter-core/builtin-fx-param params "wow/flutter"))
          (age-p (eseq.effects.builtin.filter-core/builtin-fx-param params "tape age")))
      (if (and mode-p rate-p sync-p intensity-p echo-p reverb-p)
        (h-stack :gap 0.35 :align :start
          (rate-box fx sync-p div-p offset-p rate-p)
          (mode-box fx mode-p)
          (echo-box fx intensity-p echo-p bass-p treble-p width-p)
          (tape-box fx reverb-p tension-p spring-p wf-p age-p drive-p dry-p))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
