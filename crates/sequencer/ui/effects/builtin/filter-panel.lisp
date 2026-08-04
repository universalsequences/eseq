;; Filter built-in FX panel.
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
                  (subtree :key (builtin-fx-param-subtree-key fx mode-p "mode")
                    (dropdown :value (get mode-p :text-value)
                      :options (get mode-p :options)
                      :on-change (lambda (v) (builtin-fx-set-effect-option fx mode-p v))
                      :plock-active (if (param-plock-active? fx mode-p) 1 0)
                      :plock-color-r (param-plock-color-r)
                      :plock-color-g (param-plock-color-g)
                      :plock-color-b (param-plock-color-b)
                      :width 7.7 :height 1.05 :font-size 9.5))
                  (if slope-p (builtin-fx-filter-mini-option fx slope-p) (box :width 0 :height 0)))))
          (box :width 43.2 :height 1.4 :padding 0.2
            :background-color :fx-inner-panel-bg :corner-radius 7
            (h-stack :gap 0.5 :align :baseline
              (label "LFO" :font-size 9.0 :width 2.4 :color :dim :bg :transparent)
              (if lfo-amt-p (builtin-fx-filter-mini-percent fx "amt" lfo-amt-p) (box :width 0 :height 0))
              (if lfo-sync-p
                (subtree :key (builtin-fx-param-subtree-key fx lfo-sync-p "lfo-sync")
                  (dropdown :value (builtin-fx-filter-sync-label fx lfo-sync-p)
                    :options '("free" "sync")
                    :on-change (lambda (v) (fx-set-effect-value fx lfo-sync-p (if (= v "sync") 1 0)))
                    :plock-active (if (param-plock-active? fx lfo-sync-p) 1 0)
                    :plock-color-r (param-plock-color-r)
                    :plock-color-g (param-plock-color-g)
                    :plock-color-b (param-plock-color-b)
                    :width 4.8 :height 1.05 :font-size 9.5))
                (box :width 0 :height 0))
              (if (and lfo-sync-p (fx-param-on-for? fx lfo-sync-p) lfo-div-p)
                (builtin-fx-filter-mini-option fx lfo-div-p)
                (if lfo-rate-p (builtin-fx-filter-mini-number fx "rate" lfo-rate-p) (box :width 0 :height 0)))
              (if lfo-wave-p (builtin-fx-filter-mini-option fx lfo-wave-p) (box :width 0 :height 0))
              (if lfo-phase-p (builtin-fx-filter-mini-percent fx "phs" lfo-phase-p) (box :width 0 :height 0)))))
        (fx-param-grid params fx)))))
