;; Space Echo (RE-201) built-in FX panel.
;;
;; Layout mirrors the hardware: REPEAT RATE on the left (sync grid or free
;; knob), the 12-position MODE SELECTOR in the middle, then the echo section
;; (intensity / echo volume / bass / treble) and the tape+reverb section.
;; Palette: dark chassis, RE-green selector, signal-orange repeat controls.

(def space-echo-orange () (rgba 1.00 0.62 0.25 1.0))
(def space-echo-green  () (rgba 0.36 0.80 0.50 1.0))
(def space-echo-cream  () (rgba 0.93 0.88 0.78 1.0))

;; Mod-wrapped knob (same pattern as the Str8 Delay knobs, so intensity /
;; rate / volumes pick up modulation rings and plock handling).
(def builtin-fx-space-echo-knob (fx label-text p decimals)
  (param-mod-wrapper fx p (str "space-echo-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "space-echo-param-" (get p :idx) (param-control-key-mode fx p))
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

(def builtin-fx-space-echo-percent-knob (fx label-text p)
  (param-mod-wrapper fx p (str "space-echo-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "space-echo-param-" (get p :idx) (param-control-key-mode fx p))
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
        :font-size 9.5 :label-font-size 9.0
        :text-color (param-plock-text-color fx p) :label-color :dim
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.85
        :on-change (lambda (v) (param-set-control-value fx p v))))))

;; ── Repeat rate (sync grid / free knob) ──

(def builtin-fx-space-echo-sync-button (fx p)
  (button "Sync"
    :width 4.95 :height 0.88 :padding 0 :font-size 8.5
    :background-color (if (> (get p :value) 0.5) (space-echo-orange) :mixer-control-bg)
    :color (if (> (get p :value) 0.5) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def builtin-fx-space-echo-div-button (fx p label-text)
  (button label-text
    :width 2.72 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (space-echo-orange) :mixer-control-bg)
    :color (if (= (get p :text-value) label-text) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def builtin-fx-space-echo-div-grid (fx p)
  (v-stack :gap 0.11
    (h-stack :gap 0.12
      (builtin-fx-space-echo-div-button fx p "1/32")
      (builtin-fx-space-echo-div-button fx p "1/16"))
    (h-stack :gap 0.12
      (builtin-fx-space-echo-div-button fx p "1/16t")
      (builtin-fx-space-echo-div-button fx p "1/8"))
    (h-stack :gap 0.12
      (builtin-fx-space-echo-div-button fx p "1/8t")
      (builtin-fx-space-echo-div-button fx p "1/8."))
    (h-stack :gap 0.12
      (builtin-fx-space-echo-div-button fx p "1/4")
      (builtin-fx-space-echo-div-button fx p "1/4t"))
    (h-stack :gap 0.12
      (builtin-fx-space-echo-div-button fx p "1/4.")
      (builtin-fx-space-echo-div-button fx p "1/2"))
    (h-stack :gap 0.12
      (builtin-fx-space-echo-div-button fx p "1")
      (box :width 2.72 :height 0.82))))

(def builtin-fx-space-echo-rate-box (fx sync-p div-p offset-p rate-p)
  (box :width 8.25 :height 9.75 :padding 0.18
    :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (label "REPEAT RATE" :font-size 8.0 :width 5.4 :color :dim :bg :transparent)
      (box :height 0.5)
      (builtin-fx-space-echo-sync-button fx sync-p)
      (if (> (get sync-p :value) 0.5)
        (v-stack :gap 0.12 :align :center
          (builtin-fx-space-echo-div-grid fx div-p)
          (builtin-fx-filter-mini-percent fx "ofs" offset-p))
        (v-stack :gap 0.30 :align :center
          (box :height 0.8)
          (builtin-fx-space-echo-percent-knob fx "rate" rate-p))))))

;; ── Mode selector ──

(def builtin-fx-space-echo-mode-button (fx p index short-label)
  (let ((selected (= (round (get p :value)) index))
        (reverb-mode (> index 5)))
    (button short-label
      :width 2.55 :height 1.30 :padding 0 :font-size 8.5
      :background-color (if selected
                          (if reverb-mode (space-echo-green) (space-echo-orange))
                          :mixer-control-bg)
      :color (if selected :black :dim)
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :on-click |x y r| (fx-set-effect-value fx p index))))

(def builtin-fx-space-echo-mode-grid (fx p)
  (v-stack :gap 0.14
    (h-stack :gap 0.14
      (builtin-fx-space-echo-mode-button fx p 0 "1")
      (builtin-fx-space-echo-mode-button fx p 1 "2")
      (builtin-fx-space-echo-mode-button fx p 2 "3"))
    (h-stack :gap 0.14
      (builtin-fx-space-echo-mode-button fx p 3 "1+2")
      (builtin-fx-space-echo-mode-button fx p 4 "2+3")
      (builtin-fx-space-echo-mode-button fx p 5 "all"))
    (h-stack :gap 0.14
      (builtin-fx-space-echo-mode-button fx p 6 "1R")
      (builtin-fx-space-echo-mode-button fx p 7 "2R")
      (builtin-fx-space-echo-mode-button fx p 8 "3R"))
    (h-stack :gap 0.14
      (builtin-fx-space-echo-mode-button fx p 9 "1+2R")
      (builtin-fx-space-echo-mode-button fx p 10 "2+3R")
      (builtin-fx-space-echo-mode-button fx p 11 "rev"))))

(def builtin-fx-space-echo-mode-box (fx mode-p)
  (box :width 10.5 :height 8.85 :padding 0.30
    :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.22 :align :center
      (label "MODE SELECTOR" :font-size 8.0 :width 8.2 :color :dim :bg :transparent)
      (box :height 0.25)
      (builtin-fx-space-echo-mode-grid fx mode-p)
      (label (get mode-p :text-value)
        :font-size 8.5 :width 8.2 :color (space-echo-cream) :bg :transparent))))

;; ── Echo section ──

(def builtin-fx-space-echo-echo-box (fx intensity-p echo-p bass-p treble-p width-p)
  (box :width 10.4 :height 9.75 :padding 0.36
    :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "ECHO" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (builtin-fx-space-echo-percent-knob fx "intensity" intensity-p)
        (builtin-fx-space-echo-knob fx "echo vol" echo-p 2))
      (box :height 0.85)
      (label "TONE" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (v-stack :gap 0.30 :align :baseline
        (builtin-fx-filter-mini-number fx "bass" bass-p)
        (builtin-fx-filter-mini-number fx "treb" treble-p)
        (if width-p
          (builtin-fx-filter-mini-percent fx "wide" width-p)
          (box :height 0.1))))))

;; ── Tape + reverb section ──

;; Spring type selector (which physical tank the model is tuned to).
(def builtin-fx-space-echo-spring-button (fx p index short-label)
  (let ((selected (= (round (get p :value)) index)))
    (button short-label
      :width 3.35 :height 0.92 :padding 0 :font-size 8.0
      :background-color (if selected (space-echo-green) :mixer-control-bg)
      :color (if selected :black :dim)
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :on-click |x y r| (fx-set-effect-value fx p index))))

(def builtin-fx-space-echo-spring-row (fx p)
  (v-stack
    (h-stack :gap 0.14
      (builtin-fx-space-echo-spring-button fx p 0 "RE-201")
      (builtin-fx-space-echo-spring-button fx p 1 "Tubby")))
  )

(def builtin-fx-space-echo-tape-box (fx reverb-p tension-p spring-p wf-p age-p drive-p dry-p)
  (box :width 10.4 :height 9.75 :padding 0.36
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.16 :align :center
      (label "REVERB" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (builtin-fx-space-echo-knob fx "reverb vol" reverb-p 2)
        (if tension-p
          (builtin-fx-space-echo-percent-knob fx "tension" tension-p)
          (box :width 4.35 :height 2.45)))
      (if spring-p
        (builtin-fx-space-echo-spring-row fx spring-p)
        (box :height 0.1))
      (label "TAPE" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (builtin-fx-filter-mini-percent fx "w/f" wf-p)
      (builtin-fx-filter-mini-percent fx "age" age-p)
      (builtin-fx-filter-mini-number fx "drv" drive-p)
      (builtin-fx-filter-mini-percent fx "dry" dry-p))))

(def builtin-fx-space-echo-ui (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (builtin-fx-param params "mode"))
          (rate-p (builtin-fx-param params "repeat rate"))
          (sync-p (builtin-fx-param params "sync"))
          (div-p (builtin-fx-param params "sync div"))
          (offset-p (builtin-fx-param params "sync offset"))
          (intensity-p (builtin-fx-param params "intensity"))
          (bass-p (builtin-fx-param params "bass"))
          (treble-p (builtin-fx-param params "treble"))
          (echo-p (builtin-fx-param params "echo volume"))
          (reverb-p (builtin-fx-param params "reverb volume"))
          (tension-p (builtin-fx-param params "tension"))
          (spring-p (builtin-fx-param params "spring type"))
          (width-p (builtin-fx-param params "stereo width"))
          (dry-p (builtin-fx-param params "dry"))
          (drive-p (builtin-fx-param params "input drive"))
          (wf-p (builtin-fx-param params "wow/flutter"))
          (age-p (builtin-fx-param params "tape age")))
      (if (and mode-p rate-p sync-p intensity-p echo-p reverb-p)
        (h-stack :gap 0.35 :align :start
          (builtin-fx-space-echo-rate-box fx sync-p div-p offset-p rate-p)
          (builtin-fx-space-echo-mode-box fx mode-p)
          (builtin-fx-space-echo-echo-box fx intensity-p echo-p bass-p treble-p width-p)
          (builtin-fx-space-echo-tape-box fx reverb-p tension-p spring-p wf-p age-p drive-p dry-p))
        (fx-param-grid params fx)))))
