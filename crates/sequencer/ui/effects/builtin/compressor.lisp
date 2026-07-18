;; Compressor built-in FX panel (Ableton Live style clone).
;;
;; Left to right, mirroring the device: the Sidechain section (source
;; routing, gain, listen, SC filter with type/freq/res), the Ratio /
;; Attack / Release column, the activity display (output envelope, GR
;; trace, threshold line) with the Thresh / Out header and Knee / Look /
;; Env footer, then Makeup, the detection model, and Dry/Wet.

(def comp-orange () (rgba 1.00 0.62 0.25 1.0))
(def comp-cyan   () (rgba 0.45 0.78 0.95 1.0))

(def builtin-fx-comp-source (fx)
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx))))

(def builtin-fx-comp-knob (fx label-text p decimals)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals decimals
    :font-size 9.5 :label-font-size 9.5
    :text-color (param-plock-text-color fx p) :label-color :dim
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-default (param-plock-default fx p)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width 6.4 :height 3.2 :knob-size 2.0
    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-comp-percent-knob (fx label-text p)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :value-scale 100 :decimals 0
    :font-size 9.5 :label-font-size 9.5
    :text-color (param-plock-text-color fx p) :label-color :dim
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-default (param-plock-default fx p)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width 6.4 :height 3.2 :knob-size 2.0
    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-comp-mini-number (fx label-text p decimals w)
  (h-stack :gap 0.18 :align :baseline
    (label label-text :font-size 8.5 :color :dim :bg :transparent)
    (number-picker :value (fx-param-value-for fx p)
      :min (param-control-min fx p) :max (param-control-max fx p) :decimals decimals
      :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :on-change (lambda (v) (param-set-control-value fx p v))
      :width w :height 1.0)))

(def builtin-fx-comp-toggle (fx p label-text w)
  (button label-text
    :width w :height 1.05 :padding 0 :font-size 8.5
    :background-color (if (fx-param-on-for? fx p) (comp-orange) :mixer-control-bg)
    :color (if (fx-param-on-for? fx p) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

;; Latched enum button: highlights when the param's current option matches.
(def builtin-fx-comp-choice (fx p idx label-text w)
  (let ((active (= (get p :text-value) (nth (get p :options) idx))))
    (button label-text
      :width w :height 1.05 :padding 0 :font-size 8.5
      :background-color (if active (comp-orange) :mixer-control-bg)
      :color (if active :black :dim)
      :on-click |x y r| (fx-set-effect-value fx p idx))))

(def builtin-fx-comp-option (fx p w)
  (dropdown :value (get p :text-value)
    :options (get p :options)
    :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width w :height 1.05 :font-size 9.5))

;; ── Sidechain section ──

(def builtin-fx-comp-sidechain-box (fx sc-source-p sc-on-p sc-gain-p listen-p
                                    filter-on-p type-p freq-p res-p)
  (box :width 8.6 :height 9.7 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (h-stack :gap 0.22 :align :center
        (label "Sidechain" :font-size 8.0 :width 4.4 :color :dim :bg :transparent)
        (builtin-fx-comp-toggle fx sc-on-p "SC" 2.0)
        (builtin-fx-comp-toggle fx listen-p "Ear" 2.0))
      (builtin-fx-comp-option fx sc-source-p 7.6)
      (builtin-fx-comp-mini-number fx "Gain" sc-gain-p 1 4.2)
      (builtin-fx-comp-toggle fx filter-on-p "SC Filter" 5.4)
      (builtin-fx-comp-option fx type-p 7.6)
      (builtin-fx-comp-knob fx "Freq" freq-p 0)
      (builtin-fx-comp-mini-number fx "Res" res-p 2 4.2))))

;; ── Ratio / Attack / Release column ──

(def builtin-fx-comp-dynamics-box (fx ratio-p attack-p release-p auto-rel-p)
  (box :width 7.4 :height 9.7 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (builtin-fx-comp-knob fx "Ratio" ratio-p 2)
      (builtin-fx-comp-knob fx "Attack" attack-p 2)
      (builtin-fx-comp-knob fx "Release" release-p 0)
      (builtin-fx-comp-toggle fx auto-rel-p "Auto" 3.4))))

;; ── Activity display ──

(def builtin-fx-comp-display-box (fx thr-p out-p knee-p look-p env-p)
  (box :width 21.6 :height 9.7 :padding 0.30
       :background-color :black :corner-radius 7
    (v-stack :gap 0.16 :align :start
      (h-stack :gap 0.6 :align :baseline
        (builtin-fx-comp-mini-number fx "Thresh" thr-p 1 4.4)
        (label "GR" :font-size 8.5 :width 1.6 :color (comp-orange) :bg :transparent)
        (label "Output" :font-size 8.5 :width 3.2 :color :dim :bg :transparent)
        (builtin-fx-comp-mini-number fx "Out" out-p 1 4.4))
      (compressor-display
        :width 20.8 :height 6.0
        :source (builtin-fx-comp-source fx)
        :threshold (instrument-param-base-value thr-p))
      (h-stack :gap 0.55 :align :center
        (builtin-fx-comp-mini-number fx "Knee" knee-p 1 3.6)
        (label "Look." :font-size 8.5 :width 2.5 :color :dim :bg :transparent)
        (builtin-fx-comp-option fx look-p 4.4)
        (label "Env" :font-size 8.5 :width 1.8 :color :dim :bg :transparent)
        (builtin-fx-comp-choice fx env-p 0 "Lin" 2.2)
        (builtin-fx-comp-choice fx env-p 1 "Log" 2.2)))))

;; ── Makeup / model / Dry-Wet column ──

(def builtin-fx-comp-output-box (fx makeup-p model-p mix-p)
  (box :width 7.0 :height 9.7 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.20 :align :center
      (builtin-fx-comp-toggle fx makeup-p "Makeup" 5.2)
      (builtin-fx-comp-choice fx model-p 0 "Peak" 5.2)
      (builtin-fx-comp-choice fx model-p 1 "RMS" 5.2)
      (builtin-fx-comp-choice fx model-p 2 "Expand" 5.2)
      (builtin-fx-comp-percent-knob fx "Dry/Wet" mix-p))))

(def builtin-fx-compressor-ui (fx)
  (let ((params (get fx :params)))
    (let ((p (lambda (n) (builtin-fx-param params n))))
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
            (builtin-fx-comp-sidechain-box fx sc-source-p sc-on-p sc-gain-p
              listen-p filter-on-p type-p freq-p res-p)
            (builtin-fx-comp-dynamics-box fx ratio-p attack-p release-p auto-rel-p)
            (builtin-fx-comp-display-box fx thr-p out-p knee-p look-p env-p)
            (builtin-fx-comp-output-box fx makeup-p model-p mix-p))
          (fx-param-grid params fx))))))
