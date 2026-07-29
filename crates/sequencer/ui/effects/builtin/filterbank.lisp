;; Filterbank (Sherman Filterbank 2 style) built-in FX panel.
;;
;; Layout mirrors the hardware left to right: the Input section (drive /
;; hi-eq / sense plus noise + feedback minis), the dual-filter section with
;; the Harmonics link column between the filters, the Modulation section
;; (ADSR + attenuverters, audio-rate LFO, FM/AM with source pickers), and
;; the Output section (AR envelope, stereo split, trim, dry/wet).
;; Palette: dark chassis with the famous school-bus Sherman yellow.

(def filterbank-yellow () (rgba 0.98 0.78 0.14 1.0))
(def filterbank-cream  () (rgba 0.93 0.88 0.72 1.0))

;; Effect-node selector for the live gate LED (same shape as Roar's meters).
(def builtin-fx-filterbank-source (fx)
  (if (get fx :bus-fx)
    (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
    (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx))))

;; Tiny gate LED (env meter) next to the sense knob — lights Sherman-yellow
;; while the envelope gate is open. Fed by the `filterbank-meter:` frames the
;; live-audio analyzer publishes from the effect's state meter tail.
(def builtin-fx-filterbank-gate-led (fx)
  (gate-led :width 0.9 :height 0.9
    :on-color (filterbank-yellow)
    :source (builtin-fx-filterbank-source fx)))

;; Mod-wrapped knob (same pattern as the Space Echo knobs, so freq / res /
;; mode / ser-par / crunch / fm / am pick up modulation rings and plocks).
(def builtin-fx-filterbank-knob (fx label-text p decimals)
  (param-mod-wrapper fx p (str "filterbank-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filterbank-param-" (get p :idx) (param-control-key-mode fx p))
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
        :width 4.35 :height 2.25 :knob-size 1.70
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def builtin-fx-filterbank-percent-knob (fx label-text p)
  (param-mod-wrapper fx p (str "filterbank-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filterbank-param-" (get p :idx) (param-control-key-mode fx p))
      (knob-number :label label-text
        :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals 0
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
        :width 4.35 :height 2.25 :knob-size 1.70
        :on-change (lambda (v) (param-set-control-value fx p v))))))

;; Large filter frequency knob (the hardware's front-and-center controls).
(def builtin-fx-filterbank-freq-knob (fx label-text p)
  (param-mod-wrapper fx p (str "filterbank-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filterbank-param-" (get p :idx) (param-control-key-mode fx p))
      (knob-number :label label-text
        :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals 0
        :base-value (param-base-value-prop fx p)
        :base-min (param-base-min-prop fx p) :base-max (param-base-max-prop fx p)
        :mod-range-0-slot (param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.0
        :text-color (param-plock-text-color fx p) :label-color (filterbank-yellow)
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.65 :height 2.60 :knob-size 2.05
        :on-change (lambda (v) (param-set-control-value fx p v))))))

;; On/off toggle button (yellow when lit).
(def builtin-fx-filterbank-toggle (fx p label-text w)
  (button label-text
    :width w :height 1.05 :padding 0 :font-size 8.5
    :background-color (if (fx-param-on-for? fx p) (filterbank-yellow) :mixer-control-bg)
    :border-color :transparent
    :color (if (fx-param-on-for? fx p) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

;; Latched enum button: highlights when the param's current option matches.
(def builtin-fx-filterbank-choice (fx p idx label-text w)
  (let ((active (= (get p :text-value) (nth (get p :options) idx))))
    (button label-text
      :width w :height 1.05 :padding 0 :font-size 8.5
      :background-color (if active (filterbank-yellow) :mixer-control-bg)
      :border-color :transparent
      :color (if active :black :dim)
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :on-click |x y r| (fx-set-effect-value fx p idx))))

;; Option dropdown (harmonics, lfo wave, and the FM/AM sidechain source
;; pickers — same control the Compressor uses for its sidechain source).
(def builtin-fx-filterbank-option (fx p w h fs)
  (dropdown :value (get p :text-value)
    :options (get p :options)
    :bg-color :mixer-strip-selected-bg
    :border-color :mixer-strip-border
    :badge-color :transparent
    :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :width w :height h :font-size fs))

;; ── Input section ──

(def builtin-fx-filterbank-input-box (fx input-p hieq-p sense-p noise-p feedback-p)
  (box :width 9.9 :height 9.75 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "INPUT" :font-size 8.0 :width 8.6 :color (filterbank-yellow) :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (builtin-fx-filterbank-knob fx "input" input-p 1)
        (v-stack :gap 0.10 :align :center
          (builtin-fx-filterbank-percent-knob fx "sense" sense-p)
          (builtin-fx-filterbank-gate-led fx)))
      (label "HI EQ" :font-size 8.0 :width 8.6 :color :dim :bg :transparent)
      (h-stack :gap 0.14
        (builtin-fx-filterbank-choice fx hieq-p 0 "Cut" 2.75)
        (builtin-fx-filterbank-choice fx hieq-p 1 "Flat" 2.75)
        (builtin-fx-filterbank-choice fx hieq-p 2 "Boost" 2.75))
      (box :height 0.35)
      (builtin-fx-filter-mini-number fx "noise" noise-p)
      (builtin-fx-filter-mini-number fx "fdbk" feedback-p))))

;; ── Filter columns + harmonics link ──

(def builtin-fx-filterbank-filter-column (fx title freq-p res-p mode-p)
  (v-stack :gap 0.16 :align :center
    (label title :font-size 8.0 :width 4.4 :color (filterbank-yellow) :bg :transparent)
    (builtin-fx-filterbank-freq-knob fx "freq" freq-p)
    (builtin-fx-filterbank-percent-knob fx "res" res-p)
    (builtin-fx-filterbank-percent-knob fx "mode" mode-p)
    (label "LP - BP - HP" :font-size 7.5 :width 4.4 :color :dim :bg :transparent)))

;; The harmonics link is the hardware's biggest knob — give it the visual
;; weight of the column between the two filters.
(def builtin-fx-filterbank-link-column (fx harmonics-p correction-p serpar-p crunch-p)
  (v-stack :gap 0.18 :align :center
    (label "HARMONICS" :font-size 8.5 :width 6.0 :color (filterbank-yellow) :bg :transparent)
    (builtin-fx-filterbank-option fx harmonics-p 6.0 1.45 11.0)
    (box :height 0.20)
    (builtin-fx-filter-mini-number fx "corr" correction-p)
    (builtin-fx-filterbank-percent-knob fx "ser/par" serpar-p)
    (builtin-fx-filterbank-percent-knob fx "crunch" crunch-p)))

(def builtin-fx-filterbank-filters-box (fx f1-freq-p f1-res-p f1-mode-p
                                        f2-freq-p f2-res-p f2-mode-p
                                        harmonics-p correction-p serpar-p crunch-p)
  (box :width 17.6 :height 9.75 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (h-stack :gap 0.35 :align :start
      (builtin-fx-filterbank-filter-column fx "FILTER 1" f1-freq-p f1-res-p f1-mode-p)
      (builtin-fx-filterbank-link-column fx harmonics-p correction-p serpar-p crunch-p)
      (builtin-fx-filterbank-filter-column fx "FILTER 2" f2-freq-p f2-res-p f2-mode-p))))

;; ── Modulation section ──

(def builtin-fx-filterbank-mod-box (fx envmode-p attack-p decay-p sustain-p release-p
                                    envf1-p envf2-p bleed-p
                                    rate-p wave-p depth-p trig-p sync-p div-p
                                    fm-p fmsrc-p am-p amsrc-p)
  (box :width 23.4 :height 9.75 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.16 :align :center
      (label "MODULATION" :font-size 8.0 :width 21.6 :color (filterbank-yellow) :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (v-stack :gap 0.14 :align :center
          (builtin-fx-filterbank-choice fx envmode-p 0 "ADSR" 3.1)
          (builtin-fx-filterbank-choice fx envmode-p 1 "Flw" 3.1))
        (builtin-fx-filterbank-knob fx "attack" attack-p 1)
        (builtin-fx-filterbank-knob fx "decay" decay-p 0)
        (builtin-fx-filterbank-percent-knob fx "sustain" sustain-p)
        (builtin-fx-filterbank-knob fx "release" release-p 0))
      (h-stack :gap 0.30 :align :baseline
        (builtin-fx-filter-mini-number fx "e-f1" envf1-p)
        (builtin-fx-filter-mini-number fx "e-f2" envf2-p)
        (builtin-fx-filter-mini-number fx "bleed" bleed-p))
      (h-stack :gap 0.22 :align :center
        ;; Free mode: rate knob. Synced: division dropdown in its place.
        (if (fx-param-on-for? fx sync-p)
          (v-stack :gap 0.14 :align :center
            (label "lfo div" :font-size 9.0 :width 4.35 :color :dim :bg :transparent)
            (builtin-fx-filterbank-option fx div-p 4.35 1.05 9.5))
          (builtin-fx-filterbank-knob fx "lfo rate" rate-p 2))
        (builtin-fx-filterbank-percent-knob fx "lfo depth" depth-p)
        (v-stack :gap 0.14 :align :center
          (builtin-fx-filterbank-option fx wave-p 5.0 1.05 9.5)
          (h-stack :gap 0.14
            (builtin-fx-filterbank-toggle fx trig-p "Trig" 2.4)
            (builtin-fx-filterbank-toggle fx sync-p "Sync" 2.4))))
      (h-stack :gap 0.22 :align :center
        (builtin-fx-filterbank-percent-knob fx "fm amt" fm-p)
        (builtin-fx-filterbank-option fx fmsrc-p 6.0 1.05 9.0)
        (builtin-fx-filterbank-percent-knob fx "am depth" am-p)
        (builtin-fx-filterbank-option fx amsrc-p 6.0 1.05 9.0)))))

;; ── Output section ──

(def builtin-fx-filterbank-output-box (fx ar-attack-p ar-release-p ar-depth-p
                                      split-p output-p mix-p)
  (box :width 9.9 :height 9.75 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.18 :align :center
      (label "OUTPUT" :font-size 8.0 :width 8.6 :color (filterbank-yellow) :bg :transparent)
      (h-stack :gap 0.22 :align :center
        (builtin-fx-filterbank-knob fx "ar atk" ar-attack-p 1)
        (builtin-fx-filterbank-knob fx "ar rel" ar-release-p 0))
      (h-stack :gap 0.22 :align :center
        (builtin-fx-filterbank-percent-knob fx "ar depth" ar-depth-p)
        (v-stack :gap 0.14 :align :center
          (label "SPLIT" :font-size 7.5 :width 3.4 :color :dim :bg :transparent)
          (builtin-fx-filterbank-toggle fx split-p "L|R" 3.4)))
      (h-stack :gap 0.22 :align :center
        (builtin-fx-filterbank-knob fx "output" output-p 1)
        (builtin-fx-filterbank-percent-knob fx "dry/wet" mix-p)))))

;; ── Panel body ──

(def builtin-fx-filterbank-ui (fx)
  (let ((params (get fx :params)))
    (let ((p (lambda (n) (builtin-fx-param params n))))
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
            (builtin-fx-filterbank-input-box fx input-p hieq-p sense-p noise-p feedback-p)
            (builtin-fx-filterbank-filters-box fx f1-freq-p f1-res-p f1-mode-p
              f2-freq-p f2-res-p f2-mode-p harmonics-p correction-p serpar-p crunch-p)
            (builtin-fx-filterbank-mod-box fx envmode-p attack-p decay-p sustain-p release-p
              envf1-p envf2-p bleed-p rate-p wave-p depth-p trig-p sync-p div-p
              fm-p fmsrc-p am-p amsrc-p)
            (builtin-fx-filterbank-output-box fx ar-attack-p ar-release-p ar-depth-p
              split-p output-p mix-p))
          (fx-param-grid params fx))))))
