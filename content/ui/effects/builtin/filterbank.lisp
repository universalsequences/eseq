;; Filterbank (Sherman Filterbank 2 style) built-in FX panel.
;;
;; Layout mirrors the hardware left to right: the Input section (drive /
;; hi-eq / sense plus noise + feedback minis), the dual-filter section with
;; the Harmonics link column between the filters, the Modulation section
;; (ADSR + attenuverters, audio-rate LFO, FM/AM with source pickers), and
;; the Output section (AR envelope, stereo split, trim, dry/wet).
;; Palette: dark chassis with the famous school-bus Sherman yellow.

(module eseq.effects.builtin.filterbank)

(import eseq.effects.param-controls :refer
  (fx-param-on-for?
   fx-param-value-for
   fx-set-effect-value
   fx-toggle-effect-value
   param-base-max-prop
   param-base-min-prop
   param-base-value-prop
   param-control-key-mode
   param-control-max
   param-control-min
   param-knob-mod-depth-prop
   param-knob-mod-slot-prop
   param-mod-wrapper
   param-plock-active?
   param-plock-color-b
   param-plock-color-g
   param-plock-color-r
   param-plock-default
   param-plock-text-color
   param-selected-mod-slot-prop
   param-set-control-value))
(import eseq.effects.builtin.filter-core :refer
  (builtin-fx-param
   builtin-fx-filter-mini-number
   builtin-fx-set-effect-option))
(import eseq.effects.param-grid :refer (fx-param-grid))

(export panel)

(def yellow () (rgba 0.98 0.78 0.14 1.0))
(def cream  () (rgba 0.93 0.88 0.72 1.0))

;; Effect-node selector for the live gate LED (same shape as Roar's meters).
(def effect-source (fx)
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx))))

;; Tiny gate LED (env meter) next to the sense knob — lights Sherman-yellow
;; while the envelope gate is open. Fed by the `filterbank-meter:` frames the
;; live-audio analyzer publishes from the effect's state meter tail.
(def gate-indicator (fx)
  (gate-led :width 0.9 :height 0.9
    :on-color (yellow)
    :source (effect-source fx)))

;; Mod-wrapped knob (same pattern as the Space Echo knobs, so freq / res /
;; mode / ser-par / crunch / fm / am pick up modulation rings and plocks).
(def parameter-knob (fx label-text p decimals)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "filterbank-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filterbank-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (eseq.effects.param-controls/fx-param-value-for fx p)
        :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :decimals decimals
        :base-value (eseq.effects.param-controls/param-base-value-prop fx p)
        :modulated-value (eseq.effects.param-controls/param-modulated-value p)
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
        :width 4.35 :height 2.25 :knob-size 1.70
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

(def percent-knob (fx label-text p)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "filterbank-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filterbank-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (eseq.effects.param-controls/fx-param-value-for fx p)
        :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :decimals 0
        :base-value (eseq.effects.param-controls/param-base-value-prop fx p)
        :modulated-value (eseq.effects.param-controls/param-modulated-value p)
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
        :width 4.35 :height 2.25 :knob-size 1.70
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

;; Large filter frequency knob (the hardware's front-and-center controls).
(def freq-knob (fx label-text p)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "filterbank-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filterbank-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (eseq.effects.param-controls/fx-param-value-for fx p)
        :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :decimals 0
        :base-value (eseq.effects.param-controls/param-base-value-prop fx p)
        :modulated-value (eseq.effects.param-controls/param-modulated-value p)
        :base-min (eseq.effects.param-controls/param-base-min-prop fx p) :base-max (eseq.effects.param-controls/param-base-max-prop fx p)
        :mod-range-0-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (eseq.effects.param-controls/param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.0
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color (yellow)
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.65 :height 2.60 :knob-size 2.05
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

;; On/off toggle button (yellow when lit).
(def parameter-toggle (fx p label-text w)
  (button label-text
    :width w :height 1.05 :padding 0 :font-size 8.5
    :background-color (if (eseq.effects.param-controls/fx-param-on-for? fx p) (yellow) :mixer-control-bg)
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
      :background-color (if active (yellow) :mixer-control-bg)
      :border-color :transparent
      :color (if active :black :dim)
      :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
      :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
      :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
      :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
      :on-click |x y r| (eseq.effects.param-controls/fx-set-effect-value fx p idx))))

;; Option dropdown (harmonics, lfo wave, and the FM/AM sidechain source
;; pickers — same control the Compressor uses for its sidechain source).
(def option (fx p w h fs)
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
    :width w :height h :font-size fs))

;; ── Input section ──

(def input-box (fx input-p hieq-p sense-p noise-p feedback-p)
  (box :width 9.9 :height 9.75 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "INPUT" :font-size 8.0 :width 8.6 :color (yellow) :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (parameter-knob fx "input" input-p 1)
        (v-stack :gap 0.10 :align :center
          (percent-knob fx "sense" sense-p)
          (gate-indicator fx)))
      (label "HI EQ" :font-size 8.0 :width 8.6 :color :dim :bg :transparent)
      (h-stack :gap 0.14
        (choice fx hieq-p 0 "Cut" 2.95)
        (choice fx hieq-p 1 "Flat" 2.95)
        (choice fx hieq-p 2 "Boost" 2.95))
      (box :height 0.35)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "noise" noise-p)
      (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "fdbk" feedback-p))))

;; ── Filter columns + harmonics link ──

(def filter-column (fx title freq-p res-p mode-p)
  (v-stack :gap 0.16 :align :center
    (label title :font-size 8.0 :width 4.4 :color (yellow) :bg :transparent)
    (freq-knob fx "freq" freq-p)
    (percent-knob fx "res" res-p)
    (percent-knob fx "mode" mode-p)
    (label "LP - BP - HP" :font-size 7.5 :width 4.4 :color :dim :bg :transparent)))

;; The harmonics link is the hardware's biggest knob — give it the visual
;; weight of the column between the two filters.
(def link-column (fx harmonics-p correction-p serpar-p crunch-p)
  (v-stack :gap 0.18 :align :center
    (label "HARMONICS" :font-size 8.5 :width 6.0 :color (yellow) :bg :transparent)
    (option fx harmonics-p 6.0 1.45 11.0)
    (box :height 0.20)
    (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "corr" correction-p)
    (percent-knob fx "ser/par" serpar-p)
    (percent-knob fx "crunch" crunch-p)))

(def filters-box (fx f1-freq-p f1-res-p f1-mode-p
                   f2-freq-p f2-res-p f2-mode-p
                   harmonics-p correction-p serpar-p crunch-p)
  (box :width 17.6 :height 9.75 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (h-stack :gap 0.35 :align :start
      (filter-column fx "FILTER 1" f1-freq-p f1-res-p f1-mode-p)
      (link-column fx harmonics-p correction-p serpar-p crunch-p)
      (filter-column fx "FILTER 2" f2-freq-p f2-res-p f2-mode-p))))

;; ── Modulation section ──

(def mod-box (fx envmode-p attack-p decay-p sustain-p release-p
               envf1-p envf2-p bleed-p
               rate-p wave-p depth-p trig-p sync-p div-p
               fm-p fmsrc-p am-p amsrc-p)
  (box :width 23.4 :height 9.75 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.16 :align :center
      (label "MODULATION" :font-size 8.0 :width 21.6 :color (yellow) :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (v-stack :gap 0.14 :align :center
          (choice fx envmode-p 0 "ADSR" 3.1)
          (choice fx envmode-p 1 "Flw" 3.1))
        (parameter-knob fx "attack" attack-p 1)
        (parameter-knob fx "decay" decay-p 0)
        (percent-knob fx "sustain" sustain-p)
        (parameter-knob fx "release" release-p 0))
      (h-stack :gap 0.30 :align :baseline
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "e-f1" envf1-p)
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "e-f2" envf2-p)
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "bleed" bleed-p))
      (h-stack :gap 0.22 :align :center
        ;; Free mode: rate knob. Synced: division dropdown in its place.
        (if (eseq.effects.param-controls/fx-param-on-for? fx sync-p)
          (v-stack :gap 0.14 :align :center
            (label "lfo div" :font-size 9.0 :width 4.35 :color :dim :bg :transparent)
            (option fx div-p 4.35 1.05 9.5))
          (parameter-knob fx "lfo rate" rate-p 2))
        (percent-knob fx "lfo depth" depth-p)
        (v-stack :gap 0.14 :align :center
          (option fx wave-p 5.0 1.05 9.5)
          (h-stack :gap 0.14
            (parameter-toggle fx trig-p "Trig" 2.4)
            (parameter-toggle fx sync-p "Sync" 2.4))))
      (h-stack :gap 0.22 :align :center
        (percent-knob fx "fm amt" fm-p)
        (option fx fmsrc-p 6.0 1.05 9.0)
        (percent-knob fx "am depth" am-p)
        (option fx amsrc-p 6.0 1.05 9.0)))))

;; ── Output section ──

(def output-box (fx ar-attack-p ar-release-p ar-depth-p
                  split-p output-p mix-p)
  (box :width 9.9 :height 9.75 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "OUTPUT" :font-size 8.0 :width 8.6 :color (yellow) :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (parameter-knob fx "ar atk" ar-attack-p 1)
        (parameter-knob fx "ar rel" ar-release-p 0))
      (h-stack :gap 0.22 :align :center
        (percent-knob fx "ar depth" ar-depth-p)
        (v-stack :gap 0.14 :align :center
          (label "SPLIT" :font-size 7.5 :width 3.4 :color :dim :bg :transparent)
          (parameter-toggle fx split-p "L|R" 3.4)))
      (h-stack :gap 0.22 :align :center
        (parameter-knob fx "output" output-p 1)
        (percent-knob fx "dry/wet" mix-p)))))

;; ── Panel body ──

(def panel (fx)
  (let ((params (get fx :params)))
    (let ((p (lambda (n) (eseq.effects.builtin.filter-core/builtin-fx-param params n))))
      (let ((input-p (p "input")) (hieq-p (p "hi eq")) (sense-p (p "sense"))
            (noise-p (p "noise")) (feedback-p (p "feedback"))
            (crunch-p (p "crunch")) (correction-p (p "correction"))
            (serpar-p (p "ser/par")) (harmonics-p (p "harmonics"))
            (fm-p (p "fm amount")) (fmsrc-p (p "fm source"))
            (am-p (p "am depth")) (amsrc-p (p "am source"))
            (envmode-p (p "env mode")) (attack-p (p "attack"))
            (decay-p (p "decay")) (sustain-p (p "sustain"))
            (release-p (p "release")) (envf1-p (p "env f1"))
            (envf2-p (p "env f2")) (bleed-p (p "res bleed"))
            (rate-p (p "lfo rate")) (wave-p (p "lfo wave"))
            (depth-p (p "lfo depth")) (trig-p (p "lfo trig"))
            (sync-p (p "lfo sync")) (div-p (p "lfo div"))
            (ar-attack-p (p "ar attack")) (ar-release-p (p "ar release"))
            (ar-depth-p (p "ar depth")) (split-p (p "stereo split"))
            (output-p (p "output")) (mix-p (p "dry/wet"))
            (f1-freq-p (p "f1 freq")) (f1-res-p (p "f1 res")) (f1-mode-p (p "f1 mode"))
            (f2-freq-p (p "f2 freq")) (f2-res-p (p "f2 res")) (f2-mode-p (p "f2 mode")))
        (if (and input-p hieq-p sense-p noise-p feedback-p harmonics-p
                 serpar-p crunch-p correction-p envmode-p attack-p decay-p
                 sustain-p release-p envf1-p envf2-p bleed-p
                 rate-p wave-p depth-p trig-p sync-p div-p fm-p fmsrc-p am-p amsrc-p
                 ar-attack-p ar-release-p ar-depth-p split-p output-p mix-p
                 f1-freq-p f1-res-p f1-mode-p f2-freq-p f2-res-p f2-mode-p)
          (h-stack :gap 0.35 :align :start
            (input-box fx input-p hieq-p sense-p noise-p feedback-p)
            (filters-box fx f1-freq-p f1-res-p f1-mode-p
              f2-freq-p f2-res-p f2-mode-p harmonics-p correction-p serpar-p crunch-p)
            (mod-box fx envmode-p attack-p decay-p sustain-p release-p
              envf1-p envf2-p bleed-p rate-p wave-p depth-p trig-p sync-p div-p
              fm-p fmsrc-p am-p amsrc-p)
            (output-box fx ar-attack-p ar-release-p ar-depth-p
              split-p output-p mix-p))
          (eseq.effects.param-grid/fx-param-grid params fx))))))
