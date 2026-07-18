;; Roar built-in FX panel (Ableton Live 12 inspired).
;;
;; Layout mirrors the device, left to right: Input (Drive / Tone), Routing
;; (topology picker + mode fields), the tabbed Stage box (shaper view +
;; filter view for the selected stage), the Feedback network, and Output
;; (Compress / Output / Dry-Wet). Tab selection is UI-only state, not a
;; param.

(def roar-orange () (rgba 1.00 0.62 0.25 1.0))
(def roar-cyan   () (rgba 0.45 0.78 0.95 1.0))
(def roar-pink   () (rgba 0.95 0.45 0.62 1.0))

(defstate roar-selected-stage 0)

(def builtin-fx-roar-source (fx)
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx))))

(def roar-stage-param (params stage field)
  (builtin-fx-param params (str "s" (+ stage 1) " " field)))

(def roar-tab-count (routing)
  (if (= routing 0) 1 (if (= routing 3) 3 2)))

(def roar-tab-label (routing idx)
  (if (= routing 3)
    (nth '("Low" "Mid" "High") idx)
    (if (= routing 4)
      (nth '("Mid" "Side") idx)
      (nth '("Stage 1" "Stage 2" "Stage 3") idx))))

(def roar-stage-color (idx)
  (if (= idx 0) (roar-orange) (if (= idx 1) (roar-cyan) (roar-pink))))

;; Mod-wrapped knobs (same pattern as Phaser-Flanger so drive / tone /
;; fb amount / dry-wet pick up modulation rings and plock handling).
(def builtin-fx-roar-knob (fx label-text p decimals)
  (param-mod-wrapper fx p (str "roar-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "roar-param-" (get p :idx) (param-control-key-mode fx p))
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

(def builtin-fx-roar-percent-knob (fx label-text p)
  (param-mod-wrapper fx p (str "roar-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "roar-param-" (get p :idx) (param-control-key-mode fx p))
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
        :font-size 9.5 :label-font-size 7.5
        :text-color (param-plock-text-color fx p) :label-color :dim
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.85
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def builtin-fx-roar-toggle (fx p label-text w)
  (button label-text
    :width w :height 1.05 :padding 0 :font-size 8.5
    :background-color (if (fx-param-on-for? fx p) (roar-orange) :mixer-control-bg)
    :color (if (fx-param-on-for? fx p) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def builtin-fx-roar-option (fx p w)
  (dropdown :value (get p :text-value)
    :options (get p :options)
    :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width w :height 1.05 :font-size 9.0))

;; ── Input box (Drive / Tone) ──

(def builtin-fx-roar-input-box (fx drive-p tone-p tone-freq-p tone-mode-p)
  (box :width 5.6 :height 9.70 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (label "INPUT" :font-size 8.0 :width 4.6 :color :dim :bg :transparent)
      (builtin-fx-roar-knob fx "drive" drive-p 1)
      (builtin-fx-roar-percent-knob fx "tone" tone-p)
      (subtree :key "roar-tone-mode-control"
        (builtin-fx-roar-option fx tone-mode-p 4.7))
      (builtin-fx-filter-mini-number fx "freq" tone-freq-p))))

;; ── Routing box (topology + mode-dependent fields) ──

(def builtin-fx-roar-routing-fields (fx routing blend-p xlow-p xhigh-p)
  (if (= routing 3)
    (v-stack :gap 0.18 :align :baseline
      (builtin-fx-filter-mini-number fx "low" xlow-p)
      (builtin-fx-filter-mini-number fx "high" xhigh-p))
    (if (or (= routing 2) (= routing 4))
      (builtin-fx-filter-mini-percent fx "blnd" blend-p)
      (box :height 1.0))))

(def builtin-fx-roar-routing-box (fx routing-p blend-p xlow-p xhigh-p)
  (box :width 7.4 :height 9.70 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.22 :align :center
      (label "ROUTING" :font-size 8.0 :width 6.2 :color :dim :bg :transparent)
      (subtree :key "roar-routing-control"
        (builtin-fx-roar-option fx routing-p 6.4))
      (builtin-fx-roar-routing-fields fx (round (fx-param-numeric-value routing-p)) blend-p xlow-p xhigh-p))))

;; ── Stage box (tab row + shaper/filter views for the selected stage) ──

(def builtin-fx-roar-stage-tab (fx routing idx selected)
  (button (roar-tab-label routing idx)
    :width 5.0 :height 1.15 :padding 0 :font-size 8.5
    :background-color (if selected (roar-stage-color idx) :mixer-control-bg)
    :color (if selected :black :dim)
    :on-click |x y r| (set! roar-selected-stage idx)))

(def builtin-fx-roar-stage-tabs (fx routing stage)
  (let ((count (roar-tab-count routing)))
    (h-stack :gap 0.16
      (each (range count) |idx i|
        (builtin-fx-roar-stage-tab fx routing idx (= idx stage))))))

(def builtin-fx-roar-shaper-view (fx stage shaper-p amount-p bias-p level-p)
  (v-stack :gap 0.16 :align :center
    (roar-shaper
      :width 9.4 :height 4.6
      :source (builtin-fx-roar-source fx)
      :stage stage
      ;; Base-value bindings, not snapshot values: knob drags update the
      ;; value field in place and do not rebuild the panel.
      :shaper (instrument-param-base-value shaper-p)
      :amount (instrument-param-base-value amount-p)
      :bias (instrument-param-base-value bias-p))
    (subtree :key (str "roar-shaper-option-" stage)
      (builtin-fx-roar-option fx shaper-p 8.4))
    (builtin-fx-filter-mini-number fx "levl" level-p)))

(def builtin-fx-roar-filter-view (fx stage filter-p freq-p res-p pre-p)
  (v-stack :gap 0.16 :align :center
    (roar-filter
      :width 9.4 :height 4.6
      :source (builtin-fx-roar-source fx)
      :stage stage
      :filter (instrument-param-base-value filter-p)
      :freq (instrument-param-base-value freq-p)
      :res (instrument-param-base-value res-p))
    (subtree :key (str "roar-filter-option-" stage)
      (builtin-fx-roar-option fx filter-p 8.4))
    (h-stack :gap 0.30 :align :baseline
      (builtin-fx-filter-mini-number fx "res" res-p)
      (builtin-fx-roar-toggle fx pre-p "Pre" 2.4))))

(def builtin-fx-roar-stage-box (fx params routing)
  (let ((stage (min roar-selected-stage (- (roar-tab-count routing) 1))))
    (let ((shaper-p (roar-stage-param params stage "shaper"))
          (amount-p (roar-stage-param params stage "amount"))
          (bias-p (roar-stage-param params stage "bias"))
          (level-p (roar-stage-param params stage "level"))
          (filter-p (roar-stage-param params stage "filter"))
          (freq-p (roar-stage-param params stage "freq"))
          (res-p (roar-stage-param params stage "res"))
          (pre-p (roar-stage-param params stage "pre")))
      (box :width 26.0 :height 9.70 :padding 0.36
           :background-color :fx-inner-panel-bg :corner-radius 7
        (v-stack :gap 0.20 :align :center
          (builtin-fx-roar-stage-tabs fx routing stage)
          (h-stack :gap 0.40 :align :start
            (v-stack :gap 0.14 :align :center
              (builtin-fx-roar-percent-knob fx "amount" amount-p)
              (builtin-fx-roar-knob fx "bias" bias-p 2)
              (builtin-fx-filter-mini-number fx "freq" freq-p))
            (builtin-fx-roar-shaper-view fx stage shaper-p amount-p bias-p level-p)
            (builtin-fx-roar-filter-view fx stage filter-p freq-p res-p pre-p)))))))

;; ── Feedback box ──

(def builtin-fx-roar-feedback-box (fx fbmode-p fbtime-p fbdiv-p fbamount-p fbinvert-p fbduck-p fbfreq-p fbwidth-p)
  (box :width 7.6 :height 9.70 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.16 :align :center
      (label "FEEDBACK" :font-size 8.0 :width 6.4 :color :dim :bg :transparent)
      (subtree :key "roar-fb-mode-control"
        (builtin-fx-roar-option fx fbmode-p 5.4))
      (if (= (get fbmode-p :text-value) "note")
        (subtree :key "roar-fb-div-control"
          (builtin-fx-roar-option fx fbdiv-p 5.4))
        (builtin-fx-filter-mini-number fx "time" fbtime-p))
      (builtin-fx-roar-percent-knob fx "amount" fbamount-p)
      (h-stack :gap 0.26 :align :center
        (builtin-fx-roar-toggle fx fbinvert-p "Ø" 1.6)
        (builtin-fx-roar-toggle fx fbduck-p "Duck" 3.0))
      (builtin-fx-filter-mini-number fx "freq" fbfreq-p)
      (builtin-fx-filter-mini-number fx "wdth" fbwidth-p))))

;; ── Output box ──

(def builtin-fx-roar-out-box (fx compress-p schpf-p output-p mix-p)
  (box :width 5.6 :height 9.70 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.10 :align :center
      (label "OUT" :font-size 8.0 :width 4.4 :color :dim :bg :transparent)
      (builtin-fx-roar-percent-knob fx "compress" compress-p)
      (builtin-fx-roar-toggle fx schpf-p "SC HPF" 4.0)
      (builtin-fx-roar-knob fx "output" output-p 1)
      (builtin-fx-roar-percent-knob fx "dry/wet" mix-p))))

(def builtin-fx-roar-ui (fx)
  (let ((params (get fx :params)))
    (let ((drive-p (builtin-fx-param params "drive"))
          (tone-p (builtin-fx-param params "tone"))
          (tone-freq-p (builtin-fx-param params "tone freq"))
          (tone-mode-p (builtin-fx-param params "tone mode"))
          (routing-p (builtin-fx-param params "routing"))
          (blend-p (builtin-fx-param params "blend"))
          (xlow-p (builtin-fx-param params "xover low"))
          (xhigh-p (builtin-fx-param params "xover high"))
          (fbmode-p (builtin-fx-param params "fb mode"))
          (fbtime-p (builtin-fx-param params "fb time"))
          (fbdiv-p (builtin-fx-param params "fb div"))
          (fbamount-p (builtin-fx-param params "fb amount"))
          (fbinvert-p (builtin-fx-param params "fb invert"))
          (fbduck-p (builtin-fx-param params "fb duck"))
          (fbfreq-p (builtin-fx-param params "fb freq"))
          (fbwidth-p (builtin-fx-param params "fb width"))
          (compress-p (builtin-fx-param params "compress"))
          (schpf-p (builtin-fx-param params "sc hpf"))
          (output-p (builtin-fx-param params "output"))
          (mix-p (builtin-fx-param params "dry/wet")))
      (if (and drive-p tone-p tone-freq-p tone-mode-p routing-p blend-p
               xlow-p xhigh-p fbmode-p fbtime-p fbdiv-p fbamount-p
               fbinvert-p fbduck-p fbfreq-p fbwidth-p compress-p schpf-p
               output-p mix-p
               (roar-stage-param params 0 "shaper"))
        (h-stack :gap 0.35 :align :start
          (builtin-fx-roar-input-box fx drive-p tone-p tone-freq-p tone-mode-p)
          (builtin-fx-roar-routing-box fx routing-p blend-p xlow-p xhigh-p)
          (builtin-fx-roar-stage-box fx params (round (fx-param-numeric-value routing-p)))
          (builtin-fx-roar-feedback-box fx fbmode-p fbtime-p fbdiv-p fbamount-p fbinvert-p fbduck-p fbfreq-p fbwidth-p)
          (builtin-fx-roar-out-box fx compress-p schpf-p output-p mix-p))
        (fx-param-grid params fx)))))
