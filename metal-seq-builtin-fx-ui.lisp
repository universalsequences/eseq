;; Custom UI bodies for built-in audio effects.

(defstate builtin-fx-filter-live-slot -1)
(defstate builtin-fx-filter-live-cutoff 0)
(defstate builtin-fx-filter-live-resonance 0)
(defstate builtin-fx-filter-live-active false)

(def builtin-fx-param (params name)
  (nth (filter |p| (= (get p :name) name) params) 0))

(def builtin-fx-filter-mode-type (mode-label)
  (if (= mode-label "highpass")
    "highpass"
    (if (= mode-label "bandpass")
      "bandpass"
      (if (= mode-label "notch")
        "notch"
        "lowpass"))))

(def builtin-fx-filter-live? (fx)
  (and builtin-fx-filter-live-active
       (not (get fx :bus-fx))
       (not (get fx :midi-fx))
       (= builtin-fx-filter-live-slot (get fx :slot-idx))))

(def builtin-fx-filter-cutoff-value (fx cutoff-p)
  (if (builtin-fx-filter-live? fx) builtin-fx-filter-live-cutoff (get cutoff-p :value)))

(def builtin-fx-filter-resonance-value (fx resonance-p)
  (if (builtin-fx-filter-live? fx) builtin-fx-filter-live-resonance (get resonance-p :value)))

(def builtin-fx-filter-band (fx mode-p cutoff-p resonance-p)
  (dict
    :id 0
    :type (builtin-fx-filter-mode-type (get mode-p :text-value))
    :freq (builtin-fx-filter-cutoff-value fx cutoff-p)
    :freq-min (get cutoff-p :min)
    :freq-max (get cutoff-p :max)
    :gain 0
    :gain-min -12
    :gain-max 12
    :q (builtin-fx-filter-resonance-value fx resonance-p)
    :q-min (get resonance-p :min)
    :q-max (get resonance-p :max)
    :enabled true
    :selected true))

(def builtin-fx-set-effect-option (fx p label)
  (do
    (fx-clear-selected-effect)
    (host-command
      (if (get fx :bus-fx)
        (if (seq-has-selection?) "set-bus-effect-plock-option" "set-bus-effect-param-option")
        (if (seq-has-selection?) "set-effect-plock-option" "set-effect-param-option"))
      (dict :bus (get fx :bus-idx) :slot-idx (get fx :slot-idx)
            :param-idx (get p :idx) :label label))))

(def builtin-fx-handle-filter-curve-action (fx cutoff-p resonance-p event)
  (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
    (do
      (fx-clear-selected-effect)
      (set! builtin-fx-filter-live-slot (get fx :slot-idx))
      (set! builtin-fx-filter-live-cutoff (get event :freq))
      (set! builtin-fx-filter-live-resonance (get event :q))
      (set! builtin-fx-filter-live-active (= (get event :type) :change-band))
      (if (or (get fx :bus-fx) (get fx :midi-fx))
        (do
          (fx-set-effect-value fx cutoff-p (get event :freq))
          (fx-set-effect-value fx resonance-p (get event :q)))
        (if (seq-has-selection?)
          (seq-set-effect-plock-pair
            (get fx :slot-idx)
            (get cutoff-p :idx) (get event :freq)
            (get resonance-p :idx) (get event :q))
          (if (= (get event :type) :change-band)
            (seq-set-effect-param-pair-live
              (get fx :slot-idx)
              (get cutoff-p :idx) (get event :freq)
              (get resonance-p :idx) (get event :q))
            (seq-set-effect-param-pair
              (get fx :slot-idx)
              (get cutoff-p :idx) (get event :freq)
              (get resonance-p :idx) (get event :q))))))
    nil))

(def builtin-fx-filter-readout (fx label-text p value width)
  (h-stack :gap 0.18 :align :baseline
    (label label-text :font-size 8.5 :width 3.2 :color :dim :bg :transparent)
    (number-picker :value value
      :min (get p :min) :max (get p :max) :decimals 2
      :noui true :font-size 9.5 :text-color :dim
      :on-change (lambda (v) (fx-set-effect-value fx p v))
      :width width :height 0.95)))

(def builtin-fx-filter-number (fx label-text p width decimals)
  (h-stack :gap 0.22 :align :baseline
    (label label-text :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
    (number-picker :value (get p :value)
      :min (get p :min) :max (get p :max) :decimals decimals
      :noui true :font-size 9.5 :text-color :fg
      :on-change (lambda (v) (fx-set-effect-value fx p v))
      :width width :height 1.05)))

(def builtin-fx-filter-percent (fx label-text p width)
  (h-stack :gap 0.22 :align :baseline
    (label label-text :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
    (number-picker :value (* (get p :value) 100)
      :min (* (get p :min) 100) :max (* (get p :max) 100) :decimals 0
      :noui true :font-size 9.5 :text-color :fg
      :on-change (lambda (v) (fx-set-effect-value fx p (/ v 100)))
      :width width :height 1.05)))

(def builtin-fx-filter-option (fx label-text p width)
  (h-stack :gap 0.22 :align :center
    (label label-text :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
    (dropdown :value (get p :text-value)
      :options (get p :options)
      :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
      :width width :height 1.05 :font-size 9.5)))

(def builtin-fx-filter-sync-label (p)
  (if (> (get p :value) 0.5) "sync" "free"))

(def builtin-fx-filter-sync-control (fx p)
  (h-stack :gap 0.22 :align :center
    (label "sync" :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
    (dropdown :value (builtin-fx-filter-sync-label p)
      :options '("free" "sync")
      :on-change (lambda (v) (fx-set-effect-value fx p (if (= v "sync") 1 0)))
      :width 4.8 :height 1.05 :font-size 9.5)))

(def builtin-fx-filter-mini-number (fx label-text p)
  (h-stack :gap 0.18 :align :baseline
    (label label-text :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
    (number-picker :value (get p :value)
      :min (get p :min) :max (get p :max) :decimals 2
      :noui true :font-size 9.5 :text-color :fg
      :on-change (lambda (v) (fx-set-effect-value fx p v))
      :width 4.6 :height 1.0)))

(def builtin-fx-filter-mini-cutoff (fx p)
  (h-stack :gap 0.18 :align :baseline
    (label "cut" :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
    (number-picker :value (builtin-fx-filter-cutoff-value fx p)
      :min (get p :min) :max (get p :max) :decimals 2
      :noui true :font-size 9.5 :text-color :fg
      :on-change (lambda (v) (fx-set-effect-value fx p v))
      :width 4.6 :height 1.0)))

(def builtin-fx-filter-mini-resonance (fx p)
  (h-stack :gap 0.18 :align :baseline
    (label "res" :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
    (number-picker :value (builtin-fx-filter-resonance-value fx p)
      :min (get p :min) :max (get p :max) :decimals 2
      :noui true :font-size 9.5 :text-color :fg
      :on-change (lambda (v) (fx-set-effect-value fx p v))
      :width 4.6 :height 1.0)))

(def builtin-fx-filter-cutoff-knob (fx p)
  (knob-number :label "cut"
    :value (builtin-fx-filter-cutoff-value fx p)
    :min (get p :min) :max (get p :max) :decimals 0
    :font-size 9.5 :label-font-size 9.5
    :text-color :dim :label-color :dim
    :width 4.65 :height 2.55 :knob-size 1.65
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-filter-resonance-knob (fx p)
  (knob-number :label "res"
    :value (builtin-fx-filter-resonance-value fx p)
    :min (get p :min) :max (get p :max) :decimals 2
    :font-size 9.5 :label-font-size 9.5
    :text-color :dim :label-color :dim
    :width 4.65 :height 2.55 :knob-size 1.65
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-filter-mini-percent (fx label-text p)
  (h-stack :gap 0.18 :align :baseline
    (label label-text :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
    (number-picker :value (* (get p :value) 100)
      :min (* (get p :min) 100) :max (* (get p :max) 100) :decimals 0
      :noui true :font-size 9.5 :text-color :fg
      :on-change (lambda (v) (fx-set-effect-value fx p (/ v 100)))
      :width 4.6 :height 1.0)))

(def builtin-fx-filter-mini-option (fx p)
  (dropdown :value (get p :text-value)
    :options (get p :options)
    :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
    :width 5.4 :height 1.05 :font-size 9.5))

(def builtin-fx-filter-ui (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (builtin-fx-param params "mode"))
        (cutoff-p (builtin-fx-param params "cutoff"))
        (resonance-p (builtin-fx-param params "resonance"))
        (drive-p (builtin-fx-param params "drive"))
        (wet-p (builtin-fx-param params "wet"))
        (slope-p (builtin-fx-param params "slope"))
        (lfo-amt-p (builtin-fx-param params "lfo amt"))
        (lfo-rate-p (builtin-fx-param params "lfo rate"))
        (lfo-sync-p (builtin-fx-param params "lfo sync"))
        (lfo-div-p (builtin-fx-param params "lfo div"))
        (lfo-wave-p (builtin-fx-param params "lfo wave"))
        (lfo-phase-p (builtin-fx-param params "lfo phase")))
      (if (and mode-p cutoff-p resonance-p)
        (v-stack :gap 0.15
          (h-stack :gap 0.34 :align :start
            (box :width 5.45 :height 6.30 :padding 0.28
              :background-color :fx-inner-panel-bg :corner-radius 7
              (v-stack :gap 0.3
                (builtin-fx-filter-cutoff-knob fx cutoff-p)
                (builtin-fx-filter-resonance-knob fx resonance-p)))
              (box :width 28.8 :height 6.30
                (response-curve-editor
                  :mode :filter
                  :bands (list (builtin-fx-filter-band fx mode-p cutoff-p resonance-p))
                  :freq-min (get cutoff-p :min)
                  :freq-max (get cutoff-p :max)
                  :gain-min -12
                  :gain-max 12
                  :q-min (get resonance-p :min)
                  :q-max (get resonance-p :max)
                  :background-color (rgba 0.055 0.058 0.06 1.0)
                  :corner-radius 6
                  :grid-color (rgba 0.34 0.34 0.36 0.55)
                  :stroke-color :blue
                  :point-color (rgba 1.0 0.62 0.25 1.0)
                  :on-action |event| (builtin-fx-handle-filter-curve-action fx cutoff-p resonance-p event)))
              (box :width 8.3 :height 6.30  :padding 0.28
                :background-color :fx-inner-panel-bg :corner-radius 7
                (v-stack :gap 0.2
                  (if drive-p (builtin-fx-filter-mini-percent fx "drive" drive-p) (box :width 0 :height 0))
                  (if wet-p (builtin-fx-filter-mini-percent fx "wet" wet-p) (box :width 0 :height 0))
                  (dropdown :value (get mode-p :text-value)
                    :options (get mode-p :options)
                    :on-change (lambda (v) (builtin-fx-set-effect-option fx mode-p v))
                    :width 7.7 :height 1.05 :font-size 9.5)
                  (if slope-p (builtin-fx-filter-mini-option fx slope-p) (box :width 0 :height 0)))))
          (box :width 43.2 :height 1.4 :padding 0.2
            :background-color :fx-inner-panel-bg :corner-radius 7
            (h-stack :gap 0.5 :align :baseline
              (label "LFO" :font-size 9.0 :width 2.4 :color :dim :bg :transparent)
              (if lfo-amt-p (builtin-fx-filter-mini-percent fx "amt" lfo-amt-p) (box :width 0 :height 0))
              (if lfo-sync-p
                (dropdown :value (builtin-fx-filter-sync-label lfo-sync-p)
                  :options '("free" "sync")
                  :on-change (lambda (v) (fx-set-effect-value fx lfo-sync-p (if (= v "sync") 1 0)))
                  :width 4.8 :height 1.05 :font-size 9.5)
                (box :width 0 :height 0))
              (if (and lfo-sync-p (> (get lfo-sync-p :value) 0.5) lfo-div-p)
                (builtin-fx-filter-mini-option fx lfo-div-p)
                (if lfo-rate-p (builtin-fx-filter-mini-number fx "rate" lfo-rate-p) (box :width 0 :height 0)))
              (if lfo-wave-p (builtin-fx-filter-mini-option fx lfo-wave-p) (box :width 0 :height 0))
              (if lfo-phase-p (builtin-fx-filter-mini-percent fx "phs" lfo-phase-p) (box :width 0 :height 0)))))
        (fx-param-grid params fx)))))

(def builtin-fx-dynamics-percent-knob (fx label-text p)
  (knob-number :label label-text
    :value (* (get p :value) 100)
    :min (* (get p :min) 100) :max (* (get p :max) 100) :decimals 0
    :font-size 9.5 :label-font-size 9.5
    :text-color :fg :label-color :dim
    :width 6.4 :height 3.2 :knob-size 2.0
    :on-change (lambda (v) (fx-set-effect-value fx p (/ v 100)))))

(def builtin-fx-dynamics-number-knob (fx label-text p decimals)
  (knob-number :label label-text
    :value (get p :value)
    :min (get p :min) :max (get p :max) :decimals decimals
    :font-size 9.5 :label-font-size 9.5
    :text-color :fg :label-color :dim
    :width 6.8 :height 3.2 :knob-size 2.0
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-dynamics-option (fx label-text p width)
  (h-stack :gap 0.22 :align :center
    (label label-text :font-size 8.5 :width 4.7 :color :dim :bg :transparent)
    (dropdown :value (get p :text-value)
      :options (get p :options)
      :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
      :width width :height 1.05 :font-size 9.5)))

(def builtin-fx-dynamics-ui (fx)
  (let ((params (get fx :params)))
    (let ((amount-p (builtin-fx-param params "amount"))
          (attack-p (builtin-fx-param params "attack"))
          (release-p (builtin-fx-param params "release"))
          (low-cut-p (builtin-fx-param params "low cut"))
          (drive-p (builtin-fx-param params "drive"))
          (output-p (builtin-fx-param params "output"))
          (mix-p (builtin-fx-param params "mix")))
      (if (and amount-p attack-p release-p low-cut-p drive-p output-p mix-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.45 :align :center
            (builtin-fx-dynamics-option fx "atk" attack-p 5.5)
            (builtin-fx-dynamics-option fx "rel" release-p 5.9))
          (h-stack :gap 0.5 :align :center
            (builtin-fx-dynamics-percent-knob fx "amt" amount-p)
            (builtin-fx-dynamics-number-knob fx "low" low-cut-p 0)
            (builtin-fx-dynamics-percent-knob fx "drive" drive-p)
            (builtin-fx-dynamics-number-knob fx "out" output-p 1)
            (builtin-fx-dynamics-percent-knob fx "mix" mix-p)))
        (fx-param-grid params fx)))))

(def builtin-fx-compressor-ui (fx)
  (let ((params (get fx :params)))
    (let ((threshold-p (builtin-fx-param params "threshold"))
          (ratio-p (builtin-fx-param params "ratio"))
          (attack-p (builtin-fx-param params "attack"))
          (release-p (builtin-fx-param params "release"))
          (makeup-p (builtin-fx-param params "makeup"))
          (mix-p (builtin-fx-param params "mix")))
      (if (and threshold-p ratio-p attack-p release-p makeup-p mix-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.5 :align :center
            (builtin-fx-dynamics-number-knob fx "thr" threshold-p 1)
            (builtin-fx-dynamics-number-knob fx "ratio" ratio-p 1)
            (builtin-fx-dynamics-number-knob fx "atk" attack-p 1)
            (builtin-fx-dynamics-number-knob fx "rel" release-p 0)
            (builtin-fx-dynamics-number-knob fx "mkup" makeup-p 1)
            (builtin-fx-dynamics-percent-knob fx "mix" mix-p)))
        (fx-param-grid params fx)))))

(def builtin-fx-limiter-ui (fx)
  (let ((params (get fx :params)))
    (let ((input-p (builtin-fx-param params "input"))
          (ceiling-p (builtin-fx-param params "ceiling"))
          (release-p (builtin-fx-param params "release"))
          (lookahead-p (builtin-fx-param params "lookahead")))
      (if (and input-p ceiling-p release-p lookahead-p)
        (v-stack :gap 0.34
          (h-stack :gap 0.65 :align :center
            (builtin-fx-dynamics-number-knob fx "input" input-p 1)
            (builtin-fx-dynamics-number-knob fx "ceil" ceiling-p 1)
            (builtin-fx-dynamics-number-knob fx "rel" release-p 0)
            (builtin-fx-dynamics-number-knob fx "look" lookahead-p 1)))
        (fx-param-grid params fx)))))

(def builtin-audio-fx-ui (fx)
  (if (= (get fx :name) "Filter")
    (builtin-fx-filter-ui fx)
    (if (or (= (get fx :name) "444 Compressor")
            (= (get fx :name) "Glue Compressor"))
      (builtin-fx-dynamics-ui fx)
      (if (= (get fx :name) "Compressor")
        (builtin-fx-compressor-ui fx)
        (if (= (get fx :name) "Limiter")
          (builtin-fx-limiter-ui fx)
          false)))))
