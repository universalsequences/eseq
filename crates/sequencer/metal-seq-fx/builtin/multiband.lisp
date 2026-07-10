;; OTT (Multiband Dynamics) built-in FX panel — 1:1 with the Ableton device.
;;
;; Columns left to right: Split Freq (High/Low split buttons + crossover
;; frequencies, per-band activator + solo, Soft Knee), per-band Input gain
;; knobs (+ RMS), the live band display flanked by the Below and Above
;; threshold/ratio fields and per-band Att/Rel, per-band Output gain knobs,
;; and the global Output / Time / Amount column.
;;
;; Band rows read high / mid / low from top to bottom everywhere.

(def ott-band-yellow () (rgba 0.93 0.85 0.36 1.0))
(def ott-band-cyan   () (rgba 0.45 0.78 0.95 1.0))
(def ott-band-orange () (rgba 1.00 0.62 0.25 1.0))
(def ott-button-on   () (rgba 0.94 0.69 0.32 1.0))

(def builtin-fx-ott-source (fx)
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx))))

;; Row height shared by every column so the three bands stay aligned with the
;; display rows.
(def ott-band-row-h () 2.2)

(def builtin-fx-ott-knob (fx label-text p)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals 2 :unit "dB"
    :font-size 9.0 :label-font-size 8.0
    :text-color (param-plock-text-color fx p) :label-color :dim
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-default (param-plock-default fx p)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width 5.6 :height (ott-band-row-h) :knob-size 1.5
    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-ott-percent-knob (fx label-text p)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :value-scale 100 :decimals 0 :unit "%"
    :font-size 9.0 :label-font-size 8.0
    :text-color (param-plock-text-color fx p) :label-color :dim
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-default (param-plock-default fx p)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width 5.6 :height (ott-band-row-h) :knob-size 1.5
    :track-color '(rgba 0.4, 0.4, 0.4, 1)
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-ott-field (fx p decimals unit-text color width)
  (number-picker :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals decimals :unit unit-text
    :noui true :font-size 9.0 :text-color color
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-change (lambda (v) (fx-set-effect-value fx p v))
    :width width :height 1.0))

;; Threshold above ratio, in the band color, one per band row.
(def builtin-fx-ott-thr-ratio-cell (fx thr-p ratio-p color)
  (box :height (ott-band-row-h) :padding 0
    (v-stack :gap 0.1 :align :start
      (builtin-fx-ott-field fx thr-p 1 "dB" color 4.6)
      (h-stack :gap 0.14 :align :baseline
        (label "1 :" :font-size 9.0 :width 1.3 :color :dim :bg :transparent)
        (builtin-fx-ott-field fx ratio-p 2 "" color 3.2)))))

(def builtin-fx-ott-att-rel-cell (fx attack-p release-p color)
  (box :height (ott-band-row-h) :padding 0
    (v-stack :gap 0.1 :align :start
      (builtin-fx-ott-field fx attack-p 1 "ms" color 4.4)
      (builtin-fx-ott-field fx release-p 0 "ms" color 4.4))))

(def builtin-fx-ott-toggle (fx p label-text w)
  (button label-text
    :width w :height 1.0 :padding 0 :font-size 8.0
    :background-color (if (> (get p :value) 0.5) (ott-button-on) :mixer-control-bg)
    :color (if (> (get p :value) 0.5) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

;; One Split Freq row: split button + crossover field (high/low bands) or a
;; plain label (mid), with the band activator and solo stacked beside it.
(def builtin-fx-ott-split-cell (fx split-control on-p solo-p)
  (box :height (ott-band-row-h) :padding 0
    (h-stack :gap 0.22 :align :start
      (box :width 5.2 split-control)
      (v-stack :gap 0.12 :align :start
        (builtin-fx-ott-toggle fx on-p "O" 1.4)
        (builtin-fx-ott-toggle fx solo-p "S" 1.4)))))

(def builtin-fx-ott-split-box (fx high-split-p low-split-p xh-p xl-p
                               high-on-p high-solo-p mid-on-p mid-solo-p low-on-p low-solo-p
                               knee-p)
  (box :width 8.6 :height 9.4 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :start
      (label "Split Freq" :font-size 8.5 :width 7.4 :color :dim :bg :transparent)
      (builtin-fx-ott-split-cell fx
        (v-stack :gap 0.12 :align :start
          (builtin-fx-ott-toggle fx high-split-p "High" 5.0)
          (builtin-fx-ott-field fx xh-p 0 "Hz" (rgba 0.75 0.75 0.75 1.0) 5.0))
        high-on-p high-solo-p)
      (builtin-fx-ott-split-cell fx
        (label "Mid" :font-size 9.0 :width 5.0 :color :dim :bg :transparent)
        mid-on-p mid-solo-p)
      (builtin-fx-ott-split-cell fx
        (v-stack :gap 0.12 :align :start
          (builtin-fx-ott-toggle fx low-split-p "Low" 5.0)
          (builtin-fx-ott-field fx xl-p 0 "Hz" (rgba 0.75 0.75 0.75 1.0) 5.0))
        low-on-p low-solo-p)
      (builtin-fx-ott-toggle fx knee-p "Soft Knee" 6.4))))

(def builtin-fx-ott-input-box (fx high-in-p mid-in-p low-in-p rms-p)
  (box :width 6.4 :height 9.4 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "Input" :font-size 8.5 :width 4.2 :color :dim :bg :transparent)
      (builtin-fx-ott-knob fx "" high-in-p)
      (builtin-fx-ott-knob fx "" mid-in-p)
      (builtin-fx-ott-knob fx "" low-in-p)
      (builtin-fx-ott-toggle fx rms-p "RMS" 3.6))))

(def builtin-fx-ott-display-box (fx params)
  (let ((p (lambda (name) (builtin-fx-param params name))))
    (let ((hb-thr (p "high below thr")) (hb-ratio (p "high below ratio"))
          (ha-thr (p "high above thr")) (ha-ratio (p "high above ratio"))
          (mb-thr (p "mid below thr"))  (mb-ratio (p "mid below ratio"))
          (ma-thr (p "mid above thr"))  (ma-ratio (p "mid above ratio"))
          (lb-thr (p "low below thr"))  (lb-ratio (p "low below ratio"))
          (la-thr (p "low above thr"))  (la-ratio (p "low above ratio"))
          (h-att (p "high attack")) (h-rel (p "high release"))
          (m-att (p "mid attack"))  (m-rel (p "mid release"))
          (l-att (p "low attack"))  (l-rel (p "low release"))
          (h-on (p "high on")) (m-on (p "mid on")) (l-on (p "low on"))
          (low-split (p "low split")) (high-split (p "high split")))
      (box :width 34.4 :height 9.4 :padding 0.36
           :background-color :black :corner-radius 7
        (h-stack :gap 0.4 :align :start
          (v-stack :gap 0.18 :align :start
            (label "Below" :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
            (builtin-fx-ott-thr-ratio-cell fx hb-thr hb-ratio (ott-band-yellow))
            (builtin-fx-ott-thr-ratio-cell fx mb-thr mb-ratio (ott-band-cyan))
            (builtin-fx-ott-thr-ratio-cell fx lb-thr lb-ratio (ott-band-orange)))
          (v-stack :gap 0.18 :align :start
            (box :height 0.8)
            (multiband-meter
              :width 17.0 :height 7.0
              :source (builtin-fx-ott-source fx)
              :low-below-thr (instrument-param-base-value lb-thr)
              :mid-below-thr (instrument-param-base-value mb-thr)
              :high-below-thr (instrument-param-base-value hb-thr)
              :low-above-thr (instrument-param-base-value la-thr)
              :mid-above-thr (instrument-param-base-value ma-thr)
              :high-above-thr (instrument-param-base-value ha-thr)
              :low-on (instrument-param-base-value l-on)
              :mid-on (instrument-param-base-value m-on)
              :high-on (instrument-param-base-value h-on)
              :low-split (instrument-param-base-value low-split)
              :high-split (instrument-param-base-value high-split)))
          (v-stack :gap 0.18 :align :start
            (label "Above" :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
            (builtin-fx-ott-thr-ratio-cell fx ha-thr ha-ratio (ott-band-yellow))
            (builtin-fx-ott-thr-ratio-cell fx ma-thr ma-ratio (ott-band-cyan))
            (builtin-fx-ott-thr-ratio-cell fx la-thr la-ratio (ott-band-orange)))
          (v-stack :gap 0.18 :align :start
            (label "Att/Rel" :font-size 8.5 :width 4.6 :color :dim :bg :transparent)
            (builtin-fx-ott-att-rel-cell fx h-att h-rel (ott-band-yellow))
            (builtin-fx-ott-att-rel-cell fx m-att m-rel (ott-band-cyan))
            (builtin-fx-ott-att-rel-cell fx l-att l-rel (ott-band-orange))))))))

(def builtin-fx-ott-output-box (fx high-out-p mid-out-p low-out-p)
  (box :width 6.4 :height 9.4 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "Output" :font-size 8.5 :width 4.2 :color :dim :bg :transparent)
      (builtin-fx-ott-knob fx "" high-out-p)
      (builtin-fx-ott-knob fx "" mid-out-p)
      (builtin-fx-ott-knob fx "" low-out-p))))

(def builtin-fx-ott-global-box (fx output-p time-p amount-p)
  (box :width 6.4 :height 9.4 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "Output" :font-size 8.5 :width 4.2 :color :dim :bg :transparent)
      (builtin-fx-ott-knob fx "" output-p)
      (builtin-fx-ott-percent-knob fx "Time" time-p)
      (builtin-fx-ott-percent-knob fx "Amount" amount-p))))

(def builtin-fx-ott-ui (fx)
  (let ((params (get fx :params)))
    (let ((p (lambda (name) (builtin-fx-param params name))))
      (let ((high-split (p "high split")) (low-split (p "low split"))
            (xh (p "xover high")) (xl (p "xover low"))
            (h-on (p "high on")) (h-solo (p "high solo"))
            (m-on (p "mid on")) (m-solo (p "mid solo"))
            (l-on (p "low on")) (l-solo (p "low solo"))
            (knee (p "soft knee")) (rms (p "rms"))
            (h-in (p "high input")) (m-in (p "mid input")) (l-in (p "low input"))
            (h-out (p "high output")) (m-out (p "mid output")) (l-out (p "low output"))
            (output (p "output")) (time-p (p "time")) (amount (p "amount")))
        (if (and high-split low-split xh xl h-on h-solo m-on m-solo l-on l-solo
                 knee rms h-in m-in l-in h-out m-out l-out output time-p amount)
          (h-stack :gap 0.35 :align :start
            (builtin-fx-ott-split-box fx high-split low-split xh xl
              h-on h-solo m-on m-solo l-on l-solo knee)
            (builtin-fx-ott-input-box fx h-in m-in l-in rms)
            (builtin-fx-ott-display-box fx params)
            (builtin-fx-ott-output-box fx h-out m-out l-out)
            (builtin-fx-ott-global-box fx output time-p amount))
          (fx-param-grid params fx))))))
