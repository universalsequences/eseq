;; Compressor built-in FX panel (Ableton Live style clone).
;;
;; Left to right, mirroring the device: the Sidechain section (source
;; routing, gain, listen, SC filter with type/freq/res), the Ratio /
;; Attack / Release column, the activity display (output envelope, GR
;; trace, threshold line) with the Thresh / Out header and Knee / Look /
;; Env footer, then Makeup, the detection model, and Dry/Wet.

(module eseq.effects.builtin.compressor)

(import eseq.effects.param-controls :refer
  (fx-param-on-for?
   fx-param-value
   fx-param-value-for
   fx-set-effect-value
   fx-toggle-effect-value
   instrument-param-base-value
   param-control-max
   param-control-min
   param-plock-active?
   param-plock-color-b
   param-plock-color-g
   param-plock-color-r
   param-plock-default
   param-plock-text-color
   param-set-control-value))
(import eseq.effects.builtin.filter-core :refer
  (builtin-fx-param
   builtin-fx-set-effect-option))
(import eseq.effects.param-grid :refer (fx-param-grid))

(export builtin-fx-compressor-ui)

(def orange () (rgba 1.00 0.62 0.25 1.0))
(def cyan   () (rgba 0.45 0.78 0.95 1.0))

(def effect-source (fx)
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx))))

(def parameter-knob (fx label-text p decimals)
  (knob-number :label label-text
    :value (eseq.effects.param-controls/fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals decimals
    :mod-offset (eseq.effects.param-controls/param-mod-offset p)
    :mod-scale (eseq.effects.param-controls/param-mod-scale p)
    :unit (eseq.effects.param-controls/param-control-unit fx p)
    :font-size 9.5 :label-font-size 9.5
    :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-default (eseq.effects.param-controls/param-plock-default fx p)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :width 6.4 :height 2.6 :knob-size 2.0
    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (eseq.effects.param-controls/fx-set-effect-value fx p v))))

(def percent-knob (fx label-text p)
  (knob-number :label label-text
    :value (eseq.effects.param-controls/fx-param-value p)
    :min (get p :min) :max (get p :max) :value-scale 100 :decimals 0
    :font-size 9.5 :label-font-size 9.5
    :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-default (eseq.effects.param-controls/param-plock-default fx p)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :width 6.4 :height 3.2 :knob-size 2.0
    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (eseq.effects.param-controls/fx-set-effect-value fx p v))))

(def mini-number (fx label-text p decimals w)
  (h-stack :gap 0.18 :align :baseline
    (label label-text :font-size 8.5 :color :dim :bg :transparent)
    (number-picker :value (eseq.effects.param-controls/fx-param-value-for fx p)
      :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :decimals decimals
      :noui true :font-size 9.5 :text-color (eseq.effects.param-controls/param-plock-text-color fx p)
      :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
      :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
      :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
      :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
      :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))
      :width w :height 1.0)))

(def parameter-toggle (fx p label-text w)
  (button label-text
    :width w :height 1.05 :padding 0 :font-size 8.5
    :background-color (if (eseq.effects.param-controls/fx-param-on-for? fx p) (orange) :mixer-control-bg)
    :border-color :transparent
    :color (if (eseq.effects.param-controls/fx-param-on-for? fx p) :black :dim)
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :on-click |x y r| (eseq.effects.param-controls/fx-toggle-effect-value fx p)))

;; Latched enum button: highlights when the param's current option matches.
(def choice (fx p idx label-text w)
  (let ((active (= (get p :text-value) (nth (get p :options) idx))))
    (button label-text
      :width w :height 1.05 :padding 0 :font-size 8.5
      :background-color (if active (orange) :mixer-control-bg)
      :border-color :transparent
      :color (if active :black :dim)
      :on-click |x y r| (eseq.effects.param-controls/fx-set-effect-value fx p idx))))

(def option (fx p w)
  (dropdown :value (get p :text-value)
    :options (get p :options)
    :bg-color :mixer-strip-selected-bg
    :border-color :mixer-strip-border
    :badge-color :transparent
    :on-change (lambda (v) (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx p v))
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :width w :height 0.8 :font-size 9.5))

;; ── Sidechain section ──

(def sidechain-box (fx sc-source-p sc-on-p sc-gain-p listen-p
                     filter-on-p type-p freq-p res-p)
  (box :width 9.6 :height 9.7 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (h-stack :gap 0.22 :align :center
        (label "Sidechain" :font-size 8.0 :width 4.4 :color :dim :bg :transparent)
        (parameter-toggle fx sc-on-p "SC" 2.0)
        (parameter-toggle fx listen-p "Ear" 2.0))
      (option fx sc-source-p 7.6)
      (mini-number fx "Gain" sc-gain-p 1 4.2)
      (parameter-toggle fx filter-on-p "SC Filter" 5.4)
      (option fx type-p 7.6)
      (parameter-knob fx "Freq" freq-p 0)
      (mini-number fx "Res" res-p 2 4.2))))

;; ── Ratio / Attack / Release column ──

(def dynamics-box (fx ratio-p attack-p release-p auto-rel-p)
  (box :width 7.4 :height 9.7 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (parameter-knob fx "Ratio" ratio-p 2)
      (parameter-knob fx "Attack" attack-p 2)
      (parameter-knob fx "Release" release-p 0)
      (parameter-toggle fx auto-rel-p "Auto" 3.4))))

;; ── Activity display ──

(def display-box (fx thr-p out-p knee-p look-p env-p)
  (box :width 23.6 :height 9.7 :padding 0.30
       :background-color :black :corner-radius 7
    (v-stack :gap 0.16 :align :start
      (h-stack :gap 0.6 :align :baseline
        (mini-number fx "Thresh" thr-p 1 4.4)
        (label "GR" :font-size 8.5 :width 1.6 :color (orange) :bg :transparent)
        (label "Output" :font-size 8.5 :width 3.2 :color :dim :bg :transparent)
        (mini-number fx "Out" out-p 1 4.4))
      (compressor-display
        :width 23.6 :height 6.0
        :source (effect-source fx)
        :threshold (eseq.effects.param-controls/param-effective-value thr-p))
      (h-stack :gap 0.55 :align :baseline
        (mini-number fx "Knee" knee-p 1 3.6)
        (label "Look." :font-size 8.5 :width 2.5 :color :dim :bg :transparent)
        (option fx look-p 4.4)
        (label "Env" :font-size 8.5 :width 1.8 :color :dim :bg :transparent)
        (choice fx env-p 0 "Lin" 2.2)
        (choice fx env-p 1 "Log" 2.2)))))

;; ── Makeup / model / Dry-Wet column ──

(def output-box (fx makeup-p model-p mix-p)
  (box :width 7.0 :height 9.7 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.20 :align :center
      (parameter-toggle fx makeup-p "Makeup" 5.2)
      (choice fx model-p 0 "Peak" 5.2)
      (choice fx model-p 1 "RMS" 5.2)
      (choice fx model-p 2 "Expand" 5.2)
      (percent-knob fx "Dry/Wet" mix-p))))

(def builtin-fx-compressor-ui (fx)
  (let ((params (get fx :params)))
    (let ((p (lambda (n) (eseq.effects.builtin.filter-core/builtin-fx-param params n))))
      (let ((thr-p (p "threshold")) (ratio-p (p "ratio"))
            (attack-p (p "attack")) (release-p (p "release"))
            (auto-rel-p (p "auto release")) (model-p (p "model"))
            (knee-p (p "knee")) (look-p (p "lookahead")) (env-p (p "env"))
            (out-p (p "out")) (makeup-p (p "makeup")) (mix-p (p "dry/wet"))
            (sc-on-p (p "sc on")) (sc-gain-p (p "sc gain"))
            (filter-on-p (p "sc filter")) (type-p (p "sc type"))
            (freq-p (p "sc freq")) (res-p (p "sc res"))
            (listen-p (p "sc listen")) (sc-source-p (p "sidechain")))
        (if (and thr-p ratio-p attack-p release-p model-p knee-p out-p mix-p)
          (h-stack :gap 0.30 :align :start :padding 0.1
            (sidechain-box fx sc-source-p sc-on-p sc-gain-p
              listen-p filter-on-p type-p freq-p res-p)
            (dynamics-box fx ratio-p attack-p release-p auto-rel-p)
            (display-box fx thr-p out-p knee-p look-p env-p)
            (output-box fx makeup-p model-p mix-p))
          (eseq.effects.param-grid/fx-param-grid params fx))))))
