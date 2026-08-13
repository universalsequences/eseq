;; Dimension (Roland SDD-320) built-in FX panel.
;;
;; Layout mirrors the hardware: the five DIMENSION MODE buttons (red "0"
;; clears, 1-4 latch and combine), then the character section (DYNAMIC COLOR
;; compander voicing + LFO SHAPE override), then the control knobs.
;; Palette: dark chassis, cream mode buttons, red off button.

(module eseq.effects.builtin.dimension)

(import eseq.effects.param-controls :refer
  (eseq.effects.param-controls/fx-param-on-for?
   eseq.effects.param-controls/fx-param-value-for
   eseq.effects.param-controls/fx-set-effect-value
   eseq.effects.param-controls/fx-toggle-effect-value
   eseq.effects.param-controls/param-base-max-prop
   eseq.effects.param-controls/param-base-min-prop
   eseq.effects.param-controls/param-base-value-prop
   eseq.effects.param-controls/param-control-key-mode
   eseq.effects.param-controls/param-control-max
   eseq.effects.param-controls/param-control-min
   eseq.effects.param-controls/param-knob-mod-depth-prop
   eseq.effects.param-controls/param-knob-mod-slot-prop
   eseq.effects.param-controls/param-mod-wrapper
   eseq.effects.param-controls/param-plock-active?
   eseq.effects.param-controls/param-plock-color-b
   eseq.effects.param-controls/param-plock-color-g
   eseq.effects.param-controls/param-plock-color-r
   eseq.effects.param-controls/param-plock-default
   eseq.effects.param-controls/param-plock-text-color
   eseq.effects.param-controls/param-selected-mod-slot-prop
   eseq.effects.param-controls/param-set-control-value))
(import eseq.effects.builtin.filter-core :refer
  (eseq.effects.builtin.filter-core/builtin-fx-param
   eseq.effects.builtin.filter-core/builtin-fx-set-effect-option
   eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number
   eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent))
(import eseq.effects.param-grid :refer (eseq.effects.param-grid/fx-param-grid))

(def %cream () (rgba 0.93 0.89 0.80 1.0))
(def %red   () (rgba 0.86 0.24 0.20 1.0))

;; Mod-wrapped knob (same pattern as the Space Echo knobs, so depth / mix
;; pick up modulation rings and plock handling).
(def %knob (fx label-text p decimals)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "dimension-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "dimension-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
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

;; ── Dimension mode buttons ──

(def %mode-button (fx p label-text)
  (button label-text
    :width 1.95 :height 1.55 :padding 0 :font-size 9.5
    :background-color (if (eseq.effects.param-controls/fx-param-on-for? fx p) (%cream) :mixer-control-bg)
    :color (if (eseq.effects.param-controls/fx-param-on-for? fx p) :black :dim)
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :on-click |x y r| (eseq.effects.param-controls/fx-toggle-effect-value fx p)))

(def %off-button (fx b1 b2 b3 b4)
  (let ((all-off (and (not (eseq.effects.param-controls/fx-param-on-for? fx b1)) (not (eseq.effects.param-controls/fx-param-on-for? fx b2))
                      (not (eseq.effects.param-controls/fx-param-on-for? fx b3)) (not (eseq.effects.param-controls/fx-param-on-for? fx b4)))))
    (button "0"
      :width 1.95 :height 1.55 :padding 0 :font-size 9.5
      :background-color (if all-off (%red) :mixer-control-bg)
      :color (if all-off :white :dim)
      :on-click |x y r| (do (eseq.effects.param-controls/fx-set-effect-value fx b1 0)
                            (eseq.effects.param-controls/fx-set-effect-value fx b2 0)
                            (eseq.effects.param-controls/fx-set-effect-value fx b3 0)
                            (eseq.effects.param-controls/fx-set-effect-value fx b4 0)))))

(def %mode-box (fx b1 b2 b3 b4)
  (box :width 11.4 :height 9 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.50 :align :center
      (label "DIMENSION MODE" :font-size 8.0 :width 10.4 :color :dim :bg :transparent)
      (h-stack :gap 0.16
        (%off-button fx b1 b2 b3 b4)
        (%mode-button fx b1 "1")
        (%mode-button fx b2 "2")
        (%mode-button fx b3 "3")
        (%mode-button fx b4 "4"))
      )))

;; ── Character section (dynamic color + lfo shape) ──

(def %option-button (fx p label-text)
  (button label-text
    :width 4.4 :height 0.95 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (%cream) :mixer-control-bg)
    :color (if (= (get p :text-value) label-text) :black :dim)
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :on-click |x y r| (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx p label-text)))

(def %character-box (fx color-p shape-p)
  (box :width 11.2 :height :fill :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (h-stack :width 5 :gap 0.40 :align :start
      (v-stack :gap 0.54 :align :center
        (label "COLOR" :font-size 8.0 :width 4.6 :color :dim :bg :transparent)
        (%option-button fx color-p "smooth")
        (%option-button fx color-p "default")
        (%option-button fx color-p "lf sat 1")
        (%option-button fx color-p "lf sat 2"))
      (v-stack :gap 0.54 :align :center
        (label "LFO SHAPE" :font-size 8.0 :width 4.6 :color :dim :bg :transparent)
        (%option-button fx shape-p "default")
        (%option-button fx shape-p "sine")
        (%option-button fx shape-p "ramp")
        (%option-button fx shape-p "square")
        (%option-button fx shape-p "triangle")))))

;; ── Controls ──

(def %controls-box (fx rate-p depth-p width-p tone-p mix-p)
  (box :width 10.4 :height :fill :padding 0.36
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "CONTROLS" :font-size 8.0 :width 9.2 :color :dim :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (%knob fx "depth" depth-p 2)
        (%knob fx "mix" mix-p 2))
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "rate" rate-p)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "width" width-p)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "tone" tone-p))))

(def builtin-fx-dimension-ui (fx)
  (let ((params (get fx :params)))
    (let ((b1-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mode 1"))
          (b2-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mode 2"))
          (b3-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mode 3"))
          (b4-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mode 4"))
          (color-p (eseq.effects.builtin.filter-core/builtin-fx-param params "dynamic color"))
          (shape-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lfo shape"))
          (rate-p (eseq.effects.builtin.filter-core/builtin-fx-param params "rate"))
          (depth-p (eseq.effects.builtin.filter-core/builtin-fx-param params "depth"))
          (width-p (eseq.effects.builtin.filter-core/builtin-fx-param params "width"))
          (tone-p (eseq.effects.builtin.filter-core/builtin-fx-param params "tone"))
          (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix")))
      (if (and b1-p b2-p b3-p b4-p color-p shape-p depth-p mix-p)
        (h-stack :gap 0.35 :align :start
          (%mode-box fx b1-p b2-p b3-p b4-p)
          (%character-box fx color-p shape-p)
          (%controls-box fx rate-p depth-p width-p tone-p mix-p))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
