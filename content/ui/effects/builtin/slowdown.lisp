;; Compact native Slowdown controls, using the shared mapping/p-lock contract.
(module eseq.effects.builtin.slowdown)

(import eseq.effects.param-controls :as pc)
(import eseq.effects.builtin.filter-core :as fc)

(export panel)

(def knob (fx p decimals)
  (let ((unit (pc/param-control-unit fx p)))
  (box :debug-name (str "slowdown-param-" (get p :idx))
    (pc/param-mod-wrapper fx p (fc/builtin-fx-param-subtree-key fx p "slowdown-mod")
      (subtree :key (str (fc/builtin-fx-param-subtree-key fx p "slowdown-knob")
                         (pc/param-control-key-mode fx p))
        (knob-number :label (get p :name)
          :value (pc/fx-param-value-for fx p)
          :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
          :decimals (if (or (= unit "Hz") (pc/param-mods-open? fx)) 2 decimals)
          :unit (if (= unit "Hz") "kHz" unit)
          :value-scale (if (= unit "Hz") 0.001 (if (= unit "%") 100 1))
          :taper (if (and (not (pc/param-mods-open? fx))
                          (or (= unit "Hz") (= unit "ms") (= unit "beats"))) "log" "linear")
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
          :font-size 9.5 :label-font-size 9.0
          :width 5.4 :height 3.15 :knob-size 2.0
          :on-change |v| (pc/param-set-control-value fx p v)))))))

(def clock-switch (fx p)
  (subtree :key (fc/builtin-fx-param-subtree-key fx p "slowdown-clock")
    (dropdown :value (if (pc/fx-param-on-for? fx p) "Beat sync" "Free time")
      :options '("Beat sync" "Free time")
      :debug-name "slowdown-clock" :width 8.0 :height 1.15 :font-size 10
      :plock-active (if (pc/param-plock-active? fx p) 1 0)
      :plock-color-r (pc/param-plock-color-r) :plock-color-g (pc/param-plock-color-g) :plock-color-b (pc/param-plock-color-b)
      :on-change |value| (pc/fx-set-effect-value fx p (if (= value "Beat sync") 1 0)))))

;; Head mode: varispeed (pitch follows speed), pitch-preserving slice-repeat
;; stretch, or a fixed pitch ratio with slices making up the rest.
(def modes '(("Varispeed" 0) ("Stretch" 1) ("Stretch+Pitch" 2)))
(def mode-picker (fx p)
  (subtree :key (fc/builtin-fx-param-subtree-key fx p "slowdown-mode")
    (let ((value (reactive-value (pc/instrument-param-base-value p))))
      (let ((selected (nth (filter |row| (= (nth row 1) value) modes) 0)))
        (dropdown :debug-name "slowdown-mode"
          :value (if selected (nth selected 0) "Varispeed")
          :options (map |row| (nth row 0) modes)
          :width 9.6 :height 1.15 :font-size 10
          :plock-active (if (pc/param-plock-active? fx p) 1 0)
          :plock-color-r (pc/param-plock-color-r) :plock-color-g (pc/param-plock-color-g) :plock-color-b (pc/param-plock-color-b)
          :on-change |label|
            (let ((row (nth (filter |row| (= (nth row 0) label) modes) 0)))
              (if row (pc/fx-set-effect-value fx p (nth row 1)) false)))))))

;; Presets complement the continuous beats knob: saved/mapped custom lengths
;; remain exact rather than being silently rounded to a musical division.
(def divisions '(("1/32" 0.125) ("1/16" 0.25) ("1/8" 0.5)
                 ("1/4" 1) ("1/2" 2) ("1 bar" 4)))
(def division-picker (fx p)
  (subtree :key (fc/builtin-fx-param-subtree-key fx p "slowdown-division")
    (let ((value (reactive-value (pc/instrument-param-base-value p))))
      (let ((selected (nth (filter |row| (= (nth row 1) value) divisions) 0)))
        (dropdown :debug-name "slowdown-division"
          :value (if selected (nth selected 0) "Custom")
          :options (if selected (map |row| (nth row 0) divisions)
                     (cons "Custom" (map |row| (nth row 0) divisions)))
          :width 8.0 :height 1.15 :font-size 10
          :plock-active (if (pc/param-plock-active? fx p) 1 0)
          :plock-color-r (pc/param-plock-color-r) :plock-color-g (pc/param-plock-color-g) :plock-color-b (pc/param-plock-color-b)
          :on-change |label|
            (let ((row (nth (filter |row| (= (nth row 0) label) divisions) 0)))
              (if row (pc/fx-set-effect-value fx p (nth row 1)) false)))))))

(def panel (fx)
  (let ((params (get fx :params)))
    (box :width 29.3 :padding 0.45
      (v-stack :gap 0.4 :align :center
        (h-stack :gap 0.6
          (clock-switch fx (fc/builtin-fx-param params "sync"))
          (division-picker fx (fc/builtin-fx-param params "beats"))
          (mode-picker fx (fc/builtin-fx-param params "mode")))
        (h-stack :gap 0.35
          (knob fx (fc/builtin-fx-param params "speed") 2)
          (knob fx (fc/builtin-fx-param params "beats") 3)
          (knob fx (fc/builtin-fx-param params "time") 0)
          (knob fx (fc/builtin-fx-param params "slice") 0)
          (knob fx (fc/builtin-fx-param params "xfade") 1))
        (h-stack :gap 0.35
          (knob fx (fc/builtin-fx-param params "smooth") 1)
          (knob fx (fc/builtin-fx-param params "tone") 0)
          (knob fx (fc/builtin-fx-param params "mix") 2)
          (knob fx (fc/builtin-fx-param params "pitch") 0))))))
