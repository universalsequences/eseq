;; Roar built-in FX panel (Ableton Live 12 inspired).
;;
;; Layout mirrors the device, left to right: Input (Drive / Tone), Routing
;; (topology picker + mode fields), the tabbed Stage box (shaper view +
;; filter view for the selected stage), the Feedback network, and Output
;; (Compress / Output / Dry-Wet). Tab selection is UI-only state, not a
;; param.

(module eseq.effects.builtin.roar)

(import eseq.effects.builtin.filter-core :refer
  (eseq.effects.builtin.filter-core/builtin-fx-param
   eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number
   eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent
   eseq.effects.builtin.filter-core/builtin-fx-set-effect-option))
(import eseq.effects.param-controls :refer
  (eseq.effects.param-controls/fx-param-numeric-value
   eseq.effects.param-controls/fx-param-on-for?
   eseq.effects.param-controls/fx-param-value-for
   eseq.effects.param-controls/fx-toggle-effect-value
   eseq.effects.param-controls/instrument-param-base-value
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
(import eseq.effects.param-grid :refer (eseq.effects.param-grid/fx-param-grid))

(def %orange () (rgba 1.00 0.62 0.25 1.0))
(def %cyan   () (rgba 0.45 0.78 0.95 1.0))
(def %pink   () (rgba 0.95 0.45 0.62 1.0))

(defstate %selected-stage 0)

(def %source (fx)
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx))))

(def %stage-param (params stage field)
  (eseq.effects.builtin.filter-core/builtin-fx-param params (str "s" (+ stage 1) " " field)))

(def %tab-count (routing)
  (if (= routing 0) 1 (if (= routing 3) 3 2)))

(def %tab-label (routing idx)
  (if (= routing 3)
    (nth '("Low" "Mid" "High") idx)
    (if (= routing 4)
      (nth '("Mid" "Side") idx)
      (nth '("Stage 1" "Stage 2" "Stage 3") idx))))

(def %stage-color (idx)
  (if (= idx 0) (%orange) (if (= idx 1) (%cyan) (%pink))))

;; Mod-wrapped knobs (same pattern as Phaser-Flanger so drive / tone /
;; fb amount / dry-wet pick up modulation rings and plock handling).
(def %knob (fx label-text p decimals)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "roar-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "roar-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
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

(def %percent-knob (fx label-text p)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "roar-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "roar-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (eseq.effects.param-controls/fx-param-value-for fx p)
        :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :value-scale 100 :decimals 0
        :base-value (eseq.effects.param-controls/param-base-value-prop fx p)
        :base-min (eseq.effects.param-controls/param-base-min-prop fx p) :base-max (eseq.effects.param-controls/param-base-max-prop fx p)
        :mod-range-0-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (eseq.effects.param-controls/param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 7.5
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.85
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

(def %toggle (fx p label-text w)
  (button label-text
    :width w :height 1.05 :padding 0 :font-size 8.5
    :background-color (if (eseq.effects.param-controls/fx-param-on-for? fx p) (%orange) :mixer-control-bg)
    :color (if (eseq.effects.param-controls/fx-param-on-for? fx p) :black :dim)
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :on-click |x y r| (eseq.effects.param-controls/fx-toggle-effect-value fx p)))

(def %option (fx p w)
  (dropdown :value (get p :text-value)
    :options (get p :options)
    :on-change (lambda (v) (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx p v))
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :width w :height 1.05 :font-size 9.0))

;; ── Input box (Drive / Tone) ──

(def %input-box (fx drive-p tone-p tone-freq-p tone-mode-p)
  (box :width 5.6 :height 9.70 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (label "INPUT" :font-size 8.0 :width 4.6 :color :dim :bg :transparent)
      (%knob fx "drive" drive-p 1)
      (%percent-knob fx "tone" tone-p)
      (subtree :key "roar-tone-mode-control"
        (%option fx tone-mode-p 4.7))
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "freq" tone-freq-p))))

;; ── Routing box (topology + mode-dependent fields) ──

(def %routing-fields (fx routing blend-p xlow-p xhigh-p)
  (if (= routing 3)
    (v-stack :gap 0.18 :align :baseline
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "low" xlow-p)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "high" xhigh-p))
    (if (or (= routing 2) (= routing 4))
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "blnd" blend-p)
      (box :height 1.0))))

(def %routing-box (fx routing-p blend-p xlow-p xhigh-p)
  (box :width 7.4 :height 9.70 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.22 :align :center
      (label "ROUTING" :font-size 8.0 :width 6.2 :color :dim :bg :transparent)
      (subtree :key "roar-routing-control"
        (%option fx routing-p 6.4))
      (%routing-fields fx (round (eseq.effects.param-controls/fx-param-numeric-value routing-p)) blend-p xlow-p xhigh-p))))

;; ── Stage box (tab row + shaper/filter views for the selected stage) ──

(def %stage-tab (fx routing idx selected)
  (button (%tab-label routing idx)
    :width 5.0 :height 1.15 :padding 0 :font-size 8.5
    :background-color (if selected (%stage-color idx) :mixer-control-bg)
    :color (if selected :black :dim)
    :on-click |x y r| (set! %selected-stage idx)))

(def %stage-tabs (fx routing stage)
  (let ((count (%tab-count routing)))
    (h-stack :gap 0.16
      (each (range count) |idx i|
        (%stage-tab fx routing idx (= idx stage))))))

(def %shaper-view (fx stage shaper-p amount-p bias-p level-p)
  (v-stack :gap 0.16 :align :center
    (roar-shaper
      :width 9.4 :height 4.6
      :source (%source fx)
      :stage stage
      ;; Base-value bindings, not snapshot values: knob drags update the
      ;; value field in place and do not rebuild the panel.
      :shaper (eseq.effects.param-controls/instrument-param-base-value shaper-p)
      :amount (eseq.effects.param-controls/instrument-param-base-value amount-p)
      :bias (eseq.effects.param-controls/instrument-param-base-value bias-p))
    (subtree :key (str "roar-shaper-option-" stage)
      (%option fx shaper-p 8.4))
    (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "levl" level-p)))

(def %filter-view (fx stage filter-p freq-p res-p pre-p)
  (v-stack :gap 0.16 :align :center
    (roar-filter
      :width 9.4 :height 4.6
      :source (%source fx)
      :stage stage
      :filter (eseq.effects.param-controls/instrument-param-base-value filter-p)
      :freq (eseq.effects.param-controls/instrument-param-base-value freq-p)
      :res (eseq.effects.param-controls/instrument-param-base-value res-p))
    (subtree :key (str "roar-filter-option-" stage)
      (%option fx filter-p 8.4))
    (h-stack :gap 0.30 :align :baseline
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "res" res-p)
      (%toggle fx pre-p "Pre" 2.4))))

(def %stage-box (fx params routing)
  (let ((stage (min %selected-stage (- (%tab-count routing) 1))))
    (let ((shaper-p (%stage-param params stage "shaper"))
          (amount-p (%stage-param params stage "amount"))
          (bias-p (%stage-param params stage "bias"))
          (level-p (%stage-param params stage "level"))
          (filter-p (%stage-param params stage "filter"))
          (freq-p (%stage-param params stage "freq"))
          (res-p (%stage-param params stage "res"))
          (pre-p (%stage-param params stage "pre")))
      (box :width 26.0 :height 9.70 :padding 0.36
           :background-color :fx-inner-panel-bg :corner-radius 7
        (v-stack :gap 0.20 :align :center
          (%stage-tabs fx routing stage)
          (h-stack :gap 0.40 :align :start
            (v-stack :gap 0.14 :align :center
              (%percent-knob fx "amount" amount-p)
              (%knob fx "bias" bias-p 2)
              (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "freq" freq-p))
            (%shaper-view fx stage shaper-p amount-p bias-p level-p)
            (%filter-view fx stage filter-p freq-p res-p pre-p)))))))

;; ── Feedback box ──

(def %feedback-box (fx fbmode-p fbtime-p fbdiv-p fbamount-p fbinvert-p fbduck-p fbfreq-p fbwidth-p)
  (box :width 7.6 :height 9.70 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.16 :align :center
      (label "FEEDBACK" :font-size 8.0 :width 6.4 :color :dim :bg :transparent)
      (subtree :key "roar-fb-mode-control"
        (%option fx fbmode-p 5.4))
      (if (= (get fbmode-p :text-value) "note")
        (subtree :key "roar-fb-div-control"
          (%option fx fbdiv-p 5.4))
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "time" fbtime-p))
      (%percent-knob fx "amount" fbamount-p)
      (h-stack :gap 0.26 :align :center
        (%toggle fx fbinvert-p "Ø" 1.6)
        (%toggle fx fbduck-p "Duck" 3.0))
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "freq" fbfreq-p)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "wdth" fbwidth-p))))

;; ── Output box ──

(def %out-box (fx compress-p schpf-p output-p mix-p)
  (box :width 5.6 :height 9.70 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.10 :align :center
      (label "OUT" :font-size 8.0 :width 4.4 :color :dim :bg :transparent)
      (%percent-knob fx "compress" compress-p)
      (%toggle fx schpf-p "SC HPF" 4.0)
      (%knob fx "output" output-p 1)
      (%percent-knob fx "dry/wet" mix-p))))

(def panel (fx)
  (let ((params (get fx :params)))
    (let ((drive-p (eseq.effects.builtin.filter-core/builtin-fx-param params "drive"))
          (tone-p (eseq.effects.builtin.filter-core/builtin-fx-param params "tone"))
          (tone-freq-p (eseq.effects.builtin.filter-core/builtin-fx-param params "tone freq"))
          (tone-mode-p (eseq.effects.builtin.filter-core/builtin-fx-param params "tone mode"))
          (routing-p (eseq.effects.builtin.filter-core/builtin-fx-param params "routing"))
          (blend-p (eseq.effects.builtin.filter-core/builtin-fx-param params "blend"))
          (xlow-p (eseq.effects.builtin.filter-core/builtin-fx-param params "xover low"))
          (xhigh-p (eseq.effects.builtin.filter-core/builtin-fx-param params "xover high"))
          (fbmode-p (eseq.effects.builtin.filter-core/builtin-fx-param params "fb mode"))
          (fbtime-p (eseq.effects.builtin.filter-core/builtin-fx-param params "fb time"))
          (fbdiv-p (eseq.effects.builtin.filter-core/builtin-fx-param params "fb div"))
          (fbamount-p (eseq.effects.builtin.filter-core/builtin-fx-param params "fb amount"))
          (fbinvert-p (eseq.effects.builtin.filter-core/builtin-fx-param params "fb invert"))
          (fbduck-p (eseq.effects.builtin.filter-core/builtin-fx-param params "fb duck"))
          (fbfreq-p (eseq.effects.builtin.filter-core/builtin-fx-param params "fb freq"))
          (fbwidth-p (eseq.effects.builtin.filter-core/builtin-fx-param params "fb width"))
          (compress-p (eseq.effects.builtin.filter-core/builtin-fx-param params "compress"))
          (schpf-p (eseq.effects.builtin.filter-core/builtin-fx-param params "sc hpf"))
          (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output"))
          (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "dry/wet")))
      (if (and drive-p tone-p tone-freq-p tone-mode-p routing-p blend-p
               xlow-p xhigh-p fbmode-p fbtime-p fbdiv-p fbamount-p
               fbinvert-p fbduck-p fbfreq-p fbwidth-p compress-p schpf-p
               output-p mix-p
               (%stage-param params 0 "shaper"))
        (h-stack :gap 0.35 :align :start
          (%input-box fx drive-p tone-p tone-freq-p tone-mode-p)
          (%routing-box fx routing-p blend-p xlow-p xhigh-p)
          (%stage-box fx params (round (eseq.effects.param-controls/fx-param-numeric-value routing-p)))
          (%feedback-box fx fbmode-p fbtime-p fbdiv-p fbamount-p fbinvert-p fbduck-p fbfreq-p fbwidth-p)
          (%out-box fx compress-p schpf-p output-p mix-p))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
