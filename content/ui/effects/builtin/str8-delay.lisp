;; STR8 Delay built-in FX panel.
;; See filter-core.lisp: the curve drag deliberately keeps no `defstate` echo.
;; The widget draws its own in-flight band and the host's targeted SEQV param
;; fields repaint the bound readouts, so a mouse move never reruns this panel.
(module eseq.effects.builtin.str8-delay)

(import eseq.effects.builtin.filter-core :refer
  (builtin-fx-param
   builtin-fx-param-subtree-key
   builtin-fx-set-effect-option
   builtin-fx-filter-mini-cutoff
   builtin-fx-filter-mini-number
   builtin-fx-filter-mini-percent))

(import eseq.effects.param-controls :refer
  (fx-param-value
   fx-param-on-for?
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

(import eseq.effects.panel-frame :refer (fx-clear-selected-effect))

(export builtin-fx-str8-delay-ui)

;; NOTE: `seq-has-selection?` and `host-command` are Rust natives.

(def freq-value (fx freq-p)
  (eseq.effects.param-controls/fx-param-value freq-p))

(def q-value (fx q-p)
  (eseq.effects.param-controls/fx-param-value q-p))

(def band (fx freq-p q-p)
  (dict
    :id 0
    :type "passband"
    :freq (freq-value fx freq-p)
    :freq-min (get freq-p :min)
    :freq-max (get freq-p :max)
    :gain 0
    :gain-min -12
    :gain-max 12
    :q (q-value fx q-p)
    :q-min (get q-p :min)
    :q-max (get q-p :max)
    :enabled true
    :selected true))

(def handle-curve-action (fx freq-p q-p event)
  (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
    (do
      (fx-clear-selected-effect)
      (if (or (get fx :rack-fx) (get fx :bus-fx) (get fx :midi-fx))
        (do
          (eseq.effects.param-controls/fx-set-effect-value fx freq-p (get event :freq))
          (eseq.effects.param-controls/fx-set-effect-value fx q-p (get event :q)))
        (host-command
          (if (seq-has-selection?) "set-effect-plock-batch" "set-effect-param-batch")
          (dict :slot-idx (get fx :slot-idx)
                :updates (list
                  (dict :param-idx (get freq-p :idx) :value (get event :freq))
                  (dict :param-idx (get q-p :idx) :value (get event :q)))
                :commit (= (get event :type) :commit-band)))))
    nil))

(def sync-button (fx p)
  (subtree :key (eseq.effects.builtin.filter-core/builtin-fx-param-subtree-key fx p "s8-sync")
    (button "Sync"
      :corner-radius 1
      :width 4.95 :height 0.88 :padding 0 :font-size 8.5
      :background-color (if (eseq.effects.param-controls/fx-param-on-for? fx p) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
      :color (if (eseq.effects.param-controls/fx-param-on-for? fx p) :black :dim)
      :border-color :transparent
      :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
      :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
      :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
      :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
      :on-click |x y r| (eseq.effects.param-controls/fx-toggle-effect-value fx p))))

(def div-button (fx p label-text)
  (button label-text
    :width 2.72 :height 1.12 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
        :border-color :transparent
    :color (if (= (get p :text-value) label-text) :black :dim)
    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
    :on-click |x y r| (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx p label-text)))

(def div-grid (fx p)
  (subtree :key (eseq.effects.builtin.filter-core/builtin-fx-param-subtree-key fx p "s8-div")
    (v-stack :gap 0.11
      (h-stack :gap 0.12
        (div-button fx p "1/32")
        (div-button fx p "1/16"))
      (h-stack :gap 0.12
        (div-button fx p "1/16t")
        (div-button fx p "1/8"))
      (h-stack :gap 0.12
        (div-button fx p "1/8t")
        (div-button fx p "1/8."))
      (h-stack :gap 0.12
        (div-button fx p "1/4")
        (div-button fx p "1/4t"))
      (h-stack :gap 0.12
        (div-button fx p "1/4.")
        (div-button fx p "1/2"))
      (h-stack :gap 0.12
        (div-button fx p "1")
        (box :width 2.72 :height 0.82)))))

(def side (fx title sync-p div-p offset-p time-p)
  (box :width 6.15 :height 9.45 :padding 0.18
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.14 :align :center
      (sync-button fx sync-p)
      (if (eseq.effects.param-controls/fx-param-on-for? fx sync-p)
        (v-stack :gap 0.12 :align :center
          (div-grid fx div-p)
          (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "ofs" offset-p))
        (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "ms" time-p)))))

(def parameter-knob (fx label-text p decimals)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "str8-delay-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "str8-delay-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
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
        :width 4.35 :height 2.45 :knob-size 1.55
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

(def percent-knob (fx label-text p)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "str8-delay-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "str8-delay-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
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
        :font-size 9.5 :label-font-size 9.0
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p) :label-color :dim
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.35 :height 2.45 :knob-size 1.55
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

(def builtin-fx-str8-delay-ui (fx)
  (let ((params (get fx :params)))
    (let ((wet-p (eseq.effects.builtin.filter-core/builtin-fx-param params "wet"))
          (feedback-p (eseq.effects.builtin.filter-core/builtin-fx-param params "feedback"))
          (left-sync-p (eseq.effects.builtin.filter-core/builtin-fx-param params "left sync"))
          (left-div-p (eseq.effects.builtin.filter-core/builtin-fx-param params "left div"))
          (left-offset-p (eseq.effects.builtin.filter-core/builtin-fx-param params "left offset"))
          (left-time-p (eseq.effects.builtin.filter-core/builtin-fx-param params "left time"))
          (right-sync-p (eseq.effects.builtin.filter-core/builtin-fx-param params "right sync"))
          (right-div-p (eseq.effects.builtin.filter-core/builtin-fx-param params "right div"))
          (right-offset-p (eseq.effects.builtin.filter-core/builtin-fx-param params "right offset"))
          (right-time-p (eseq.effects.builtin.filter-core/builtin-fx-param params "right time"))
          (filter-freq-p (eseq.effects.builtin.filter-core/builtin-fx-param params "filter freq"))
          (filter-q-p (eseq.effects.builtin.filter-core/builtin-fx-param params "filter width"))
          (mod-rate-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mod rate"))
          (mod-amount-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mod amount"))
          (mod-phase-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mod phase")))
      (h-stack :gap 0.35 :align :start
        (side fx "Left" left-sync-p left-div-p left-offset-p left-time-p)
        (side fx "Right" right-sync-p right-div-p right-offset-p right-time-p)
        (v-stack :gap 0.18
          (box :width 20.4 :height 7.5
            ;; See filter-panel.lisp: keep the curve in its own subtree.
            (subtree :key (eseq.effects.builtin.filter-core/builtin-fx-param-subtree-key fx filter-freq-p "curve")
            (response-curve-editor
              :mode :filter
              :bands (list (band fx filter-freq-p filter-q-p))
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
              :on-action |event| (handle-curve-action fx filter-freq-p filter-q-p event))))
          (box :width 20.4 :height 1.92 :padding 0.36
               :background-color :fx-inner-panel-bg :corner-radius 7
            (h-stack :gap 0.38 :align :baseline
              (label "Filter" :font-size 9.0 :width 3.8 :color :dim :bg :transparent)
              (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-cutoff fx filter-freq-p)
              (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "wid" filter-q-p))))
        (box :width 9.6 :height 7.45 :padding 0.36
             :background-color :fx-inner-panel-bg :corner-radius 7
          (v-stack :gap 0.16
            (h-stack :gap 0.18 :align :center
              (percent-knob fx "wet" wet-p)
              (parameter-knob fx "fb" feedback-p 2))
            (label "Mod" :font-size 9.0 :width 7.8 :color :dim :bg :transparent)
            (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "rate" mod-rate-p)
            (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "amt" mod-amount-p)
            (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "phs" mod-phase-p)))))))
