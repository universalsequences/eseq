;; STR8 Delay built-in FX panel.
;; See filter-core.lisp: the curve drag deliberately keeps no `defstate` echo.
;; The widget draws its own in-flight band and the host's targeted SEQV param
;; fields repaint the bound readouts, so a mouse move never reruns this panel.

(def builtin-fx-str8-delay-freq-value (fx freq-p)
  (fx-param-value freq-p))

(def builtin-fx-str8-delay-q-value (fx q-p)
  (fx-param-value q-p))

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
      (if (or (get fx :rack-fx) (get fx :bus-fx) (get fx :midi-fx))
        (do
          (fx-set-effect-value fx freq-p (get event :freq))
          (fx-set-effect-value fx q-p (get event :q)))
        (host-command
          (if (seq-has-selection?) "set-effect-plock-batch" "set-effect-param-batch")
          (dict :slot-idx (get fx :slot-idx)
                :updates (list
                  (dict :param-idx (get freq-p :idx) :value (get event :freq))
                  (dict :param-idx (get q-p :idx) :value (get event :q)))
                :commit (= (get event :type) :commit-band)))))
    nil))

(def builtin-fx-str8-delay-sync-button (fx p)
  (subtree :key (builtin-fx-param-subtree-key fx p "s8-sync")
    (button "Sync"
      :width 4.95 :height 0.88 :padding 0 :font-size 8.5
      :background-color (if (fx-param-on-for? fx p) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
      :color (if (fx-param-on-for? fx p) :black :dim)
          :border-color :transparent
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :on-click |x y r| (fx-toggle-effect-value fx p))))

(def builtin-fx-str8-delay-div-button (fx p label-text)
  (button label-text
    :width 2.72 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
        :border-color :transparent
    :color (if (= (get p :text-value) label-text) :black :dim)
    :plock-active (if (param-plock-active? fx p) 1 0)
    :plock-color-r (param-plock-color-r)
    :plock-color-g (param-plock-color-g)
    :plock-color-b (param-plock-color-b)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def builtin-fx-str8-delay-div-grid (fx p)
  (subtree :key (builtin-fx-param-subtree-key fx p "s8-div")
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
        (box :width 2.72 :height 0.82)))))

(def builtin-fx-str8-delay-side (fx title sync-p div-p offset-p time-p)
  (box :width 6.15 :height 7.45 :padding 0.18
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (builtin-fx-str8-delay-sync-button fx sync-p)
      (if (fx-param-on-for? fx sync-p)
        (v-stack :gap 0.12 :align :center
          (builtin-fx-str8-delay-div-grid fx div-p)
          (builtin-fx-filter-mini-percent fx "ofs" offset-p))
        (builtin-fx-filter-mini-number fx "ms" time-p)))))

(def builtin-fx-str8-delay-knob (fx label-text p decimals)
  (param-mod-wrapper fx p (str "str8-delay-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "str8-delay-param-" (get p :idx) (param-control-key-mode fx p))
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
        :width 4.35 :height 2.45 :knob-size 1.55
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def builtin-fx-str8-delay-percent-knob (fx label-text p)
  (param-mod-wrapper fx p (str "str8-delay-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "str8-delay-param-" (get p :idx) (param-control-key-mode fx p))
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
        :font-size 9.5 :label-font-size 9.0
        :text-color (param-plock-text-color fx p) :label-color :dim
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.55
        :on-change (lambda (v) (param-set-control-value fx p v))))))

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
          (box :width 20.4 :height 7.5
            ;; See filter-panel.lisp: keep the curve in its own subtree.
            (subtree :key (builtin-fx-param-subtree-key fx filter-freq-p "curve")
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
              :corner-radius 16
              :grid-color (rgba 0.34 0.34 0.36 0.55)
              :stroke-color :blue
              :point-color (rgba 1.0 0.62 0.25 1.0)
              :on-action |event| (builtin-fx-handle-str8-delay-curve-action fx filter-freq-p filter-q-p event))))
          (box :width 20.4 :height 1.92 :padding 0.36
               :background-color :fx-inner-panel-bg :corner-radius 7
            (h-stack :gap 0.38 :align :baseline
              (label "Filter" :font-size 9.0 :width 3.8 :color :dim :bg :transparent)
              (builtin-fx-filter-mini-cutoff fx filter-freq-p)
              (builtin-fx-filter-mini-number fx "wid" filter-q-p))))
        (box :width 9.6 :height 7.45 :padding 0.36
             :background-color :fx-inner-panel-bg :corner-radius 7
          (v-stack :gap 0.16
            (h-stack :gap 0.18 :align :center
              (builtin-fx-str8-delay-percent-knob fx "wet" wet-p)
              (builtin-fx-str8-delay-knob fx "fb" feedback-p 2))
            (label "Mod" :font-size 9.0 :width 7.8 :color :dim :bg :transparent)
            (builtin-fx-filter-mini-number fx "rate" mod-rate-p)
            (builtin-fx-filter-mini-percent fx "amt" mod-amount-p)
            (builtin-fx-filter-mini-percent fx "phs" mod-phase-p)))))))
