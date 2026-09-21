;; Native feed-forward chorus. Cutoff curves always edit actual frequencies;
;; the adjacent knobs also expose the host's modulation-depth / p-lock modes.
(module eseq.effects.builtin.chorus)
(import eseq.effects.param-controls :as pc)
(import eseq.effects.builtin.filter-core :as fc)
(export panel select-filter)

(defstate filter-tabs '())
(def scope-key (fx)
  (fc/builtin-fx-param-subtree-key fx (fc/builtin-fx-param (get fx :params) "enabled") "chorus"))
(def tab-for (fx)
  (let ((row (nth (filter |row| (= (nth row 0) (scope-key fx)) filter-tabs) 0)))
    (if row (nth row 1) 0)))
(def select-filter (fx tab)
  (set! filter-tabs (cons (list (scope-key fx) tab)
    (filter |row| (not (= (nth row 0) (scope-key fx))) filter-tabs))))

(def knob (fx p)
  (let ((unit (pc/param-control-unit fx p))
        (frequency (and (= (get p :unit) "Hz") (not (= (get p :name) "rate")))))
    (box :debug-name (str "chorus-param-" (get p :idx))
      (pc/param-mod-wrapper fx p (fc/builtin-fx-param-subtree-key fx p "chorus-mod")
        (subtree :key (str (fc/builtin-fx-param-subtree-key fx p "chorus-knob") (pc/param-control-key-mode fx p))
          (knob-number :label (get p :name)
            :value (pc/fx-param-value-for fx p)
            :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
            :decimals (if (= (get p :name) "stereo phase") 0 2)
            :unit unit
            :value-scale (if (= unit "%") 100 1)
            :taper (if (and (not (pc/param-mods-open? fx))
              (or frequency (= (get p :name) "rate") (= (get p :name) "base delay"))) "log" "linear")
            :base-value (pc/param-base-value-prop fx p)
            :base-min (pc/param-base-min-prop fx p) :base-max (pc/param-base-max-prop fx p)
            :mod-offset (pc/param-mod-offset p) :mod-scale (pc/param-mod-scale p)
            :mod-range-0-slot (pc/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (pc/param-knob-mod-depth-prop fx p 0)
            :mod-range-1-slot (pc/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (pc/param-knob-mod-depth-prop fx p 1)
            :mod-range-2-slot (pc/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (pc/param-knob-mod-depth-prop fx p 2)
            :mod-range-3-slot (pc/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (pc/param-knob-mod-depth-prop fx p 3)
            :selected-mod-slot (pc/param-selected-mod-slot-prop fx p)
            :plock-active (if (pc/param-plock-active? fx p) 1 0)
            :plock-default (pc/param-plock-default fx p)
            :plock-color-r (pc/param-plock-color-r) :plock-color-g (pc/param-plock-color-g) :plock-color-b (pc/param-plock-color-b)
            :text-color (pc/param-plock-text-color fx p) :label-color :dim
            :color :blue :font-size 9.5 :label-font-size 9
            :width 7.0 :height 3.15 :knob-size 2.0
            :on-change |v| (pc/param-set-control-value fx p v)))))))

(def band (p id type)
  (dict :id id :type type :freq (pc/instrument-param-base-value p)
    :freq-min (get p :min) :freq-max (get p :max)
    :gain 0 :gain-min -18 :gain-max 6
    :q 0.70710678 :q-min 0.70710678 :q-max 0.70710678
    :lock-y true :enabled true :selected true))

(def curve (fx output)
  (let ((lp (fc/builtin-fx-param (get fx :params) (if output "wet lowpass" "input lowpass")))
        (hp (fc/builtin-fx-param (get fx :params) "input highpass")))
    (response-curve-editor :debug-name (if output "chorus-wet-curve" "chorus-input-curve")
      :mode :filter :bands (if output (list (band lp 0 "lowpass"))
        (list (band lp 0 "lowpass") (band hp 1 "highpass")))
      :freq-min 20 :freq-max 20000 :gain-min -18 :gain-max 6
      :q-min 0.70710678 :q-max 0.70710678
      :background-color :instrument-control-bg :corner-radius 4
      :grid-color :border-inactive :stroke-color :blue
      :point-color :blue :stroke-width 2
      :width 29.0 :height 3.2
      :on-action |event|
        (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
          (pc/fx-set-effect-value fx (if (= (get event :id) 1) hp lp) (get event :freq)) nil))))

(def tab-button (fx tab title)
  (button title :debug-name (str "chorus-filter-tab-" tab)
    :width 14.35 :height 0.9 :font-size 9 :padding 0 :corner-radius 2
    :background-color (if (= (tab-for fx) tab) :blue :instrument-control-bg)
    :color (if (= (tab-for fx) tab) :white :dim)
    :on-click (lambda (x y r) (select-filter fx tab))))

(def panel (fx)
  (let ((params (get fx :params)) (output (= (tab-for fx) 1)))
    (box :width 46.0 :padding 0.5
      (v-stack :gap 0.45
        (h-stack :gap 0.6
          (knob fx (fc/builtin-fx-param params "rate"))
          (knob fx (fc/builtin-fx-param params "delay modulation"))
          (knob fx (fc/builtin-fx-param params "base delay"))
          (knob fx (fc/builtin-fx-param params "stereo phase"))
          (knob fx (fc/builtin-fx-param params "mix"))
          (knob fx (fc/builtin-fx-param params "output")))
        (h-stack :gap 0.6
          (v-stack :gap 0.3
            (h-stack :gap 0.3 (tab-button fx 0 "Input filter") (tab-button fx 1 "Wet lowpass"))
            (curve fx output))
          (v-stack :gap 0.2
            (h-stack :gap 0.3
              (knob fx (fc/builtin-fx-param params (if output "wet lowpass" "input highpass")))
              (if output (box :width 7 :height 3.15)
                (knob fx (fc/builtin-fx-param params "input lowpass"))))
            (label "Triangle · no feedback"
              :width 14.3 :height 0.7 :font-size 9 :color :dim :bg :transparent)))))))
