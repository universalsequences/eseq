;; STR8 Delay built-in FX panel.
(defstate builtin-fx-str8-delay-live-slot -1)
(defstate builtin-fx-str8-delay-live-freq 0)
(defstate builtin-fx-str8-delay-live-q 0)
(defstate builtin-fx-str8-delay-live-active false)

(def builtin-fx-str8-delay-live? (fx)
  (and builtin-fx-str8-delay-live-active
       (not (get fx :bus-fx))
       (not (get fx :midi-fx))
       (= builtin-fx-str8-delay-live-slot (get fx :slot-idx))))

(def builtin-fx-str8-delay-freq-value (fx freq-p)
  (if (builtin-fx-str8-delay-live? fx) builtin-fx-str8-delay-live-freq (fx-param-value freq-p)))

(def builtin-fx-str8-delay-q-value (fx q-p)
  (if (builtin-fx-str8-delay-live? fx) builtin-fx-str8-delay-live-q (fx-param-value q-p)))

(def builtin-fx-str8-delay-band (fx freq-p q-p)
  (dict
    :id 0
    :type "passband"
    :freq (builtin-fx-str8-delay-freq-value fx freq-p)
    :freq-min (get freq-p :min)
    :freq-max (get freq-p :max)
    :gain 0
    :gain-min -12
    :gain-max 12
    :q (builtin-fx-str8-delay-q-value fx q-p)
    :q-min (get q-p :min)
    :q-max (get q-p :max)
    :enabled true
    :selected true))

(def builtin-fx-handle-str8-delay-curve-action (fx freq-p q-p event)
  (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
    (do
      (fx-clear-selected-effect)
      (set! builtin-fx-str8-delay-live-slot (get fx :slot-idx))
      (set! builtin-fx-str8-delay-live-freq (get event :freq))
      (set! builtin-fx-str8-delay-live-q (get event :q))
      (set! builtin-fx-str8-delay-live-active (= (get event :type) :change-band))
      (if (or (get fx :bus-fx) (get fx :midi-fx))
        (do
          (fx-set-effect-value fx freq-p (get event :freq))
          (fx-set-effect-value fx q-p (get event :q)))
        (if (seq-has-selection?)
          (seq-set-effect-plock-pair
            (get fx :slot-idx)
            (get freq-p :idx) (get event :freq)
            (get q-p :idx) (get event :q))
          (if (= (get event :type) :change-band)
            (seq-set-effect-param-pair-live
              (get fx :slot-idx)
              (get freq-p :idx) (get event :freq)
              (get q-p :idx) (get event :q))
            (seq-set-effect-param-pair
              (get fx :slot-idx)
              (get freq-p :idx) (get event :freq)
              (get q-p :idx) (get event :q))))))
    nil))

(def builtin-fx-str8-delay-sync-button (fx p)
  (button "Sync"
    :width 4.95 :height 0.88 :padding 0 :font-size 8.5
    :background-color (if (> (get p :value) 0.5) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
    :color (if (> (get p :value) 0.5) :black :dim)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def builtin-fx-str8-delay-div-button (fx p label-text)
  (button label-text
    :width 2.72 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
    :color (if (= (get p :text-value) label-text) :black :dim)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def builtin-fx-str8-delay-div-grid (fx p)
  (v-stack :gap 0.11
    (h-stack :gap 0.12
      (builtin-fx-str8-delay-div-button fx p "1/32")
      (builtin-fx-str8-delay-div-button fx p "1/16"))
    (h-stack :gap 0.12
      (builtin-fx-str8-delay-div-button fx p "1/16t")
      (builtin-fx-str8-delay-div-button fx p "1/8"))
    (h-stack :gap 0.12
      (builtin-fx-str8-delay-div-button fx p "1/8t")
      (builtin-fx-str8-delay-div-button fx p "1/8."))
    (h-stack :gap 0.12
      (builtin-fx-str8-delay-div-button fx p "1/4")
      (builtin-fx-str8-delay-div-button fx p "1/4t"))
    (h-stack :gap 0.12
      (builtin-fx-str8-delay-div-button fx p "1/4.")
      (builtin-fx-str8-delay-div-button fx p "1/2"))
    (h-stack :gap 0.12
      (builtin-fx-str8-delay-div-button fx p "1")
      (box :width 2.72 :height 0.82))))

(def builtin-fx-str8-delay-side (fx title sync-p div-p offset-p time-p)
  (box :width 6.15 :height 7.45 :padding 0.18
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (builtin-fx-str8-delay-sync-button fx sync-p)
      (if (> (get sync-p :value) 0.5)
        (v-stack :gap 0.12 :align :center
          (builtin-fx-str8-delay-div-grid fx div-p)
          (builtin-fx-filter-mini-percent fx "ofs" offset-p))
        (builtin-fx-filter-mini-number fx "ms" time-p)))))

(def builtin-fx-str8-delay-knob (fx label-text p decimals)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :decimals decimals
    :font-size 9.5 :label-font-size 9.0
    :text-color :fg :label-color :dim
    :width 4.35 :height 2.45 :knob-size 1.55
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-str8-delay-percent-knob (fx label-text p)
  (knob-number :label label-text
    :value (fx-param-value p)
    :min (get p :min) :max (get p :max) :value-scale 100 :decimals 0
    :font-size 9.5 :label-font-size 9.0
    :text-color :fg :label-color :dim
    :width 4.35 :height 2.45 :knob-size 1.55
    :on-change (lambda (v) (fx-set-effect-value fx p v))))

(def builtin-fx-str8-delay-ui (fx)
  (let ((params (get fx :params)))
    (let ((wet-p (builtin-fx-param params "wet"))
          (feedback-p (builtin-fx-param params "feedback"))
          (left-sync-p (builtin-fx-param params "left sync"))
          (left-div-p (builtin-fx-param params "left div"))
          (left-offset-p (builtin-fx-param params "left offset"))
          (left-time-p (builtin-fx-param params "left time"))
          (right-sync-p (builtin-fx-param params "right sync"))
          (right-div-p (builtin-fx-param params "right div"))
          (right-offset-p (builtin-fx-param params "right offset"))
          (right-time-p (builtin-fx-param params "right time"))
          (filter-freq-p (builtin-fx-param params "filter freq"))
          (filter-q-p (builtin-fx-param params "filter width"))
          (mod-rate-p (builtin-fx-param params "mod rate"))
          (mod-amount-p (builtin-fx-param params "mod amount"))
          (mod-phase-p (builtin-fx-param params "mod phase")))
      (h-stack :gap 0.35 :align :start
        (builtin-fx-str8-delay-side fx "Left" left-sync-p left-div-p left-offset-p left-time-p)
        (builtin-fx-str8-delay-side fx "Right" right-sync-p right-div-p right-offset-p right-time-p)
        (v-stack :gap 0.18
          (box :width 20.4 :height 5.35
            (response-curve-editor
              :mode :filter
              :bands (list (builtin-fx-str8-delay-band fx filter-freq-p filter-q-p))
              :freq-min (get filter-freq-p :min)
              :freq-max (get filter-freq-p :max)
              :gain-min -12
              :gain-max 12
              :q-min (get filter-q-p :min)
              :q-max (get filter-q-p :max)
              :background-color (rgba 0.055 0.058 0.06 1.0)
              :corner-radius 6
              :grid-color (rgba 0.34 0.34 0.36 0.55)
              :stroke-color :blue
              :point-color (rgba 1.0 0.62 0.25 1.0)
              :on-action |event| (builtin-fx-handle-str8-delay-curve-action fx filter-freq-p filter-q-p event)))
          (box :width 20.4 :height 1.92 :padding 0.36
               :background-color :fx-inner-panel-bg :corner-radius 7
            (h-stack :gap 0.38 :align :baseline
              (label "Filter" :font-size 9.0 :width 3.8 :color :dim :bg :transparent)
              (builtin-fx-filter-mini-cutoff fx filter-freq-p)
              (builtin-fx-filter-mini-number fx "wid" filter-q-p))))
        (box :width 9.2 :height 7.45 :padding 0.36
             :background-color :fx-inner-panel-bg :corner-radius 7
          (v-stack :gap 0.16
            (h-stack :gap 0.18 :align :center
              (builtin-fx-str8-delay-percent-knob fx "wet" wet-p)
              (builtin-fx-str8-delay-knob fx "fb" feedback-p 2))
            (label "Mod" :font-size 9.0 :width 7.8 :color :dim :bg :transparent)
            (builtin-fx-filter-mini-number fx "rate" mod-rate-p)
            (builtin-fx-filter-mini-percent fx "amt" mod-amount-p)
            (builtin-fx-filter-mini-percent fx "phs" mod-phase-p)))))))
