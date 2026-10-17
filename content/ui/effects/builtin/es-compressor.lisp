;; ES Compressor: four architectures, shared timing/gain controls and a
;; manifest-backed, p-lock-aware mode selector. Glue swaps in its own stepped
;; ratio/attack/release, makeup and detector high-pass. No private study assets.
(module eseq.effects.builtin.es-compressor)

(import eseq.effects.builtin.filter-core :refer (builtin-fx-param))
(import eseq.effects.param-controls :as pc)
(import eseq.effects.param-grid :refer (fx-param-grid))

(export es-compressor-ui)

(def es-compressor-accent () :blue)

(def es-compressor-caption (text)
  (label text :font-size 8.2 :color :dim :bg :transparent))

(def es-compressor-heading (text)
  (label text :font-size 8.2 :height 0.78 :color :dim :bg :transparent))

;; Large knob with full p-lock contract. `taper` is "log" for the ms pair so
;; the arc spends equal travel per doubling instead of crowding the fast end.
(def es-compressor-knob (fx label-text p decimals taper)
  (subtree :key (str "es-compressor-knob-" (get p :idx) (pc/param-control-key-mode fx p))
    (knob-number :label label-text
      :debug-name (str "es-compressor-control-" (get p :name))
      :taper taper
      :step (if (= decimals 0) 1 nil)
      :value (pc/fx-param-value-for fx p)
      :min (pc/param-control-min fx p) :max (pc/param-control-max fx p) :decimals decimals
      :font-size 10.8 :label-font-size 9.6
      :text-color :dim :label-color :dim
      :plock-active (pc/param-plock-active-prop fx p)
      :plock-default (pc/param-plock-default fx p)
      :plock-color-r (pc/param-plock-color-r)
      :plock-color-g (pc/param-plock-color-g)
      :plock-color-b (pc/param-plock-color-b)
      :track-color :widget-knob-track
      :arc-color (es-compressor-accent)
      :width 7.4 :height 4.5 :knob-size 5.4
      :on-change (lambda (v) (pc/param-set-control-value fx p v)))))

;; Percent knob: mix is stored 0..1 and shown 0..100.
(def es-compressor-percent-knob (fx label-text p)
  (subtree :key (str "es-compressor-knob-" (get p :idx) (pc/param-control-key-mode fx p))
    (knob-number :label label-text
      :debug-name (str "es-compressor-control-" (get p :name))
      :value (pc/fx-param-value-for fx p)
      :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
      :value-scale (eseq.effects.param-controls/percent-scale fx p) :decimals 0
      :font-size 10.8 :label-font-size 9.6
      :text-color :dim :label-color :dim
      :plock-active (pc/param-plock-active-prop fx p)
      :plock-default (pc/param-plock-default fx p)
      :plock-color-r (pc/param-plock-color-r)
      :plock-color-g (pc/param-plock-color-g)
      :plock-color-b (pc/param-plock-color-b)
      :track-color :widget-knob-track
      :arc-color (es-compressor-accent)
      :width 7.4 :height 4.5 :knob-size 5.4
      :on-change (lambda (v) (pc/param-set-control-value fx p v)))))

;; Slider-style dB trim for the gain strip.
(def es-compressor-trim (fx label-text p)
  (subtree :key (str "es-compressor-trim-" (get p :idx) (pc/param-control-key-mode fx p))
    (v-stack :width 7.2 :gap 0.06 :align :start
      (label label-text :font-size 9 :width 7.2 :height 0.78 :color :dim :bg :transparent)
      (number-picker
        :debug-name (str "es-compressor-control-" (get p :name))
        :value (pc/fx-param-value-for fx p)
        :min (pc/param-control-min fx p)
        :max (pc/param-control-max fx p)
        :decimals 1 :step nil :unit "dB"
        :mode :slider :fill-color :number-slider-fill :noui false
        :corner-radius 0 :border-color :black :background-color :mixer-strip-bg
        :font-size 9.5 :text-align :left :width 7.2 :height 0.8
        :text-color :dim
        :edit-color :yellow
        :plock-active (pc/param-plock-active-prop fx p)
        :plock-color-r (pc/param-plock-color-r)
        :plock-color-g (pc/param-plock-color-g)
        :plock-color-b (pc/param-plock-color-b)
        :on-change (lambda (v) (pc/param-set-control-value fx p v))))))

(def es-compressor-surface (width surface body)
  (box :width width :padding 0.24 :background-color surface :corner-radius 7 :border-width 1
    body))

;; Manifest-backed option dropdown with the full p-lock contract.
(def es-compressor-dropdown (fx p debug-name width height font-size)
  (subtree :key (str "es-compressor-option-" (get p :idx) (pc/param-control-key-mode fx p))
    (dropdown :debug-name debug-name
      :value (pc/fx-param-text-value-for fx p)
      :options (get p :options)
      :width width :height height :font-size font-size
      :bg-color :mixer-strip-bg :border-color :mixer-strip-border
      :plock-active (pc/param-plock-active-prop fx p)
      :plock-color-r (pc/param-plock-color-r)
      :plock-color-g (pc/param-plock-color-g)
      :plock-color-b (pc/param-plock-color-b)
      :on-change (lambda (v) (pc/param-set-option fx p v)))))

(def es-compressor-mode (fx p)
  (es-compressor-dropdown fx p "es-compressor-mode" 9 1.1 10))

;; Labelled selector for a Glue-only enum param.
(def es-compressor-glue-option (fx label-text p width)
  (v-stack :width width :gap 0.06 :align :start
    (label label-text :font-size 9 :width width :height 0.78 :color :dim :bg :transparent)
    (es-compressor-dropdown fx p (str "es-compressor-control-" (get p :name)) width 1.0 9.5)))

;; Glue's response surface: the three switches above makeup, sidechain HP
;; and tone. Shown in place of the attack/release knobs only in Glue mode.
(def es-compressor-glue-response (fx glue-ratio-p attack-p release-p glue-makeup-p hp-p tone-p)
  (v-stack :gap 0.18 :align :start
    (h-stack :gap 0.3 :align :start
      (es-compressor-glue-option fx "ratio" glue-ratio-p 7.4)
      (es-compressor-glue-option fx "attack" attack-p 7.4)
      (es-compressor-glue-option fx "release" release-p 7.4))
    (h-stack :gap 0.3 :align :start
      (es-compressor-knob fx "makeup / dB" glue-makeup-p 1 "linear")
      (es-compressor-knob fx "sc hp / Hz" hp-p 0 "log")
      (es-compressor-knob fx "tone / %" tone-p 0 "linear"))
    (es-compressor-caption
      ;; Release option 4 is Auto (manifest order 0.1/0.3/0.6/1.2 s/Auto).
      (if (= (pc/fx-param-numeric-value-for fx release-p) 4)
        "feedback glue · auto release · sc hp 20 = off"
        "feedback glue · sc hp 20 = off"))))

(def es-compressor-ui (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mode"))
          (tone-p (eseq.effects.builtin.filter-core/builtin-fx-param params "tone"))
          (amount-p (eseq.effects.builtin.filter-core/builtin-fx-param params "amount"))
          (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
          (attack-p (eseq.effects.builtin.filter-core/builtin-fx-param params "attack"))
          (release-p (eseq.effects.builtin.filter-core/builtin-fx-param params "release"))
          (input-p (eseq.effects.builtin.filter-core/builtin-fx-param params "input-db"))
          (drive-p (eseq.effects.builtin.filter-core/builtin-fx-param params "drive"))
          (detector-p (eseq.effects.builtin.filter-core/builtin-fx-param params "detector-db"))
          (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output-db"))
          (glue-ratio-p (eseq.effects.builtin.filter-core/builtin-fx-param params "glue-ratio"))
          (glue-attack-p (eseq.effects.builtin.filter-core/builtin-fx-param params "glue-attack"))
          (glue-release-p (eseq.effects.builtin.filter-core/builtin-fx-param params "glue-release"))
          (detector-hp-p (eseq.effects.builtin.filter-core/builtin-fx-param params "detector-hp"))
          (glue-makeup-p (eseq.effects.builtin.filter-core/builtin-fx-param params "glue-makeup")))
      (if (and mode-p tone-p amount-p mix-p attack-p release-p input-p drive-p detector-p output-p
               glue-ratio-p glue-attack-p glue-release-p detector-hp-p glue-makeup-p)
        (h-stack :gap 0.35 :align :stretch :debug-name "es-compressor-panel"
          (es-compressor-surface 16 :instrument-group-bg
            (v-stack :gap 0.18 :align :start
              (es-compressor-mode fx mode-p)
              (h-stack :gap 0.3 :align :start
                (es-compressor-knob fx "amount / %" amount-p 0 "linear")
                (es-compressor-percent-knob fx "mix" mix-p))
              (es-compressor-caption "stereo linked · parallel mix")))
          (es-compressor-surface 23.8 :instrument-group-bg
            (v-stack :gap 0.18 :align :start
              (box :height 1.1 (es-compressor-heading "RESPONSE / COLOR"))
              (let ((mode-value (pc/fx-param-numeric-value-for fx mode-p)))
                (if (= mode-value 3)
                  (es-compressor-glue-response fx glue-ratio-p glue-attack-p glue-release-p
                    glue-makeup-p detector-hp-p tone-p)
                  (v-stack :gap 0.18 :align :start
                    (h-stack :gap 0.3 :align :start
                      (es-compressor-knob fx "attack / ms" attack-p 1 "log")
                      (es-compressor-knob fx "release / ms" release-p 0 "log")
                      (es-compressor-knob fx "tone / %" tone-p 0 "linear"))
                    (es-compressor-caption
                      (if (= mode-value 1) "leveling · +8.5 dB nominal gain"
                        (if (= mode-value 2) "grab-and-settle · automatic makeup"
                          "broad-knee punch · manual output makeup"))))))))
          (es-compressor-surface 8 :instrument-control-bg
            (v-stack :gap 0.18 :align :start
              (es-compressor-heading "GAIN")
              (es-compressor-trim fx "input" input-p)
              (es-compressor-trim fx "drive" drive-p)
              (es-compressor-trim fx "detector" detector-p)
              (es-compressor-trim fx "output" output-p))))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
