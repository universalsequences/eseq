;; Dimension (Roland SDD-320) built-in FX panel.
;;
;; Layout mirrors the hardware: the five DIMENSION MODE buttons (red "0"
;; clears, 1-4 latch and combine), then the character section (DYNAMIC COLOR
;; compander voicing + LFO SHAPE override), then the control knobs.
;; Palette: dark chassis, cream mode buttons, red off button.

(def dimension-cream () (rgba 0.93 0.89 0.80 1.0))
(def dimension-red   () (rgba 0.86 0.24 0.20 1.0))

;; Mod-wrapped knob (same pattern as the Space Echo knobs, so depth / mix
;; pick up modulation rings and plock handling).
(def builtin-fx-dimension-knob (fx label-text p decimals)
  (param-mod-wrapper fx p (str "dimension-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "dimension-param-" (get p :idx) (param-control-key-mode fx p))
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

;; ── Dimension mode buttons ──

(def builtin-fx-dimension-mode-button (fx p label-text)
  (button label-text
    :width 1.95 :height 1.55 :padding 0 :font-size 9.5
    :background-color (if (> (get p :value) 0.5) (dimension-cream) :mixer-control-bg)
    :color (if (> (get p :value) 0.5) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def builtin-fx-dimension-off-button (fx b1 b2 b3 b4)
  (let ((all-off (and (< (get b1 :value) 0.5) (< (get b2 :value) 0.5)
                      (< (get b3 :value) 0.5) (< (get b4 :value) 0.5))))
    (button "0"
      :width 1.95 :height 1.55 :padding 0 :font-size 9.5
      :background-color (if all-off (dimension-red) :mixer-control-bg)
      :color (if all-off :white :dim)
      :on-click |x y r| (do (fx-set-effect-value fx b1 0)
                            (fx-set-effect-value fx b2 0)
                            (fx-set-effect-value fx b3 0)
                            (fx-set-effect-value fx b4 0)))))

(def builtin-fx-dimension-mode-box (fx b1 b2 b3 b4)
  (box :width 11.4 :height 9 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.50 :align :center
      (label "DIMENSION MODE" :font-size 8.0 :width 10.4 :color :dim :bg :transparent)
      (h-stack :gap 0.16
        (builtin-fx-dimension-off-button fx b1 b2 b3 b4)
        (builtin-fx-dimension-mode-button fx b1 "1")
        (builtin-fx-dimension-mode-button fx b2 "2")
        (builtin-fx-dimension-mode-button fx b3 "3")
        (builtin-fx-dimension-mode-button fx b4 "4"))
      )))

;; ── Character section (dynamic color + lfo shape) ──

(def builtin-fx-dimension-option-button (fx p label-text)
  (button label-text
    :width 4.4 :height 0.95 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (dimension-cream) :mixer-control-bg)
    :color (if (= (get p :text-value) label-text) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def builtin-fx-dimension-character-box (fx color-p shape-p)
  (box :width 11.2 :height :fill :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (h-stack :width 5 :gap 0.40 :align :start
      (v-stack :gap 0.54 :align :center
        (label "COLOR" :font-size 8.0 :width 4.6 :color :dim :bg :transparent)
        (builtin-fx-dimension-option-button fx color-p "smooth")
        (builtin-fx-dimension-option-button fx color-p "default")
        (builtin-fx-dimension-option-button fx color-p "lf sat 1")
        (builtin-fx-dimension-option-button fx color-p "lf sat 2"))
      (v-stack :gap 0.54 :align :center
        (label "LFO SHAPE" :font-size 8.0 :width 4.6 :color :dim :bg :transparent)
        (builtin-fx-dimension-option-button fx shape-p "default")
        (builtin-fx-dimension-option-button fx shape-p "sine")
        (builtin-fx-dimension-option-button fx shape-p "ramp")
        (builtin-fx-dimension-option-button fx shape-p "square")
        (builtin-fx-dimension-option-button fx shape-p "triangle")))))

;; ── Controls ──

(def builtin-fx-dimension-controls-box (fx rate-p depth-p width-p tone-p mix-p)
  (box :width 10.4 :height :fill :padding 0.36
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "CONTROLS" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (builtin-fx-dimension-knob fx "depth" depth-p 2)
        (builtin-fx-dimension-knob fx "mix" mix-p 2))
      (builtin-fx-filter-mini-number fx "rate" rate-p)
      (builtin-fx-filter-mini-percent fx "width" width-p)
      (builtin-fx-filter-mini-number fx "tone" tone-p))))

(def builtin-fx-dimension-ui (fx)
  (let ((params (get fx :params)))
    (let ((b1-p (builtin-fx-param params "mode 1"))
          (b2-p (builtin-fx-param params "mode 2"))
          (b3-p (builtin-fx-param params "mode 3"))
          (b4-p (builtin-fx-param params "mode 4"))
          (color-p (builtin-fx-param params "dynamic color"))
          (shape-p (builtin-fx-param params "lfo shape"))
          (rate-p (builtin-fx-param params "rate"))
          (depth-p (builtin-fx-param params "depth"))
          (width-p (builtin-fx-param params "width"))
          (tone-p (builtin-fx-param params "tone"))
          (mix-p (builtin-fx-param params "mix")))
      (if (and b1-p b2-p b3-p b4-p color-p shape-p depth-p mix-p)
        (h-stack :gap 0.35 :align :start
          (builtin-fx-dimension-mode-box fx b1-p b2-p b3-p b4-p)
          (builtin-fx-dimension-character-box fx color-p shape-p)
          (builtin-fx-dimension-controls-box fx rate-p depth-p width-p tone-p mix-p))
        (fx-param-grid params fx)))))
