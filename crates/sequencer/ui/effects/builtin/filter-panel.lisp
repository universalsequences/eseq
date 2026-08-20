;; Filter built-in FX panel.
(module eseq.effects.builtin.filter-panel)

(import eseq.effects.builtin.filter-core :refer
  (eseq.effects.builtin.filter-core/builtin-fx-param
   eseq.effects.builtin.filter-core/builtin-fx-param-subtree-key
   eseq.effects.builtin.filter-core/builtin-fx-filter-band
   eseq.effects.builtin.filter-core/builtin-fx-filter-cutoff-knob
   eseq.effects.builtin.filter-core/builtin-fx-filter-resonance-knob
   eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent
   eseq.effects.builtin.filter-core/builtin-fx-filter-mini-option
   eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number
   eseq.effects.builtin.filter-core/builtin-fx-filter-sync-label
   eseq.effects.builtin.filter-core/builtin-fx-handle-filter-curve-action
   eseq.effects.builtin.filter-core/builtin-fx-set-effect-option))
(import eseq.effects.param-controls :refer
  (eseq.effects.param-controls/fx-param-on-for?
   eseq.effects.param-controls/fx-set-effect-value
   eseq.effects.param-controls/param-plock-active?
   eseq.effects.param-controls/param-plock-color-r
   eseq.effects.param-controls/param-plock-color-g
   eseq.effects.param-controls/param-plock-color-b))
(import eseq.effects.param-grid :refer (eseq.effects.param-grid/fx-param-grid))

(def panel (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mode"))
        (cutoff-p (eseq.effects.builtin.filter-core/builtin-fx-param params "cutoff"))
        (resonance-p (eseq.effects.builtin.filter-core/builtin-fx-param params "resonance"))
        (drive-p (eseq.effects.builtin.filter-core/builtin-fx-param params "drive"))
        (wet-p (eseq.effects.builtin.filter-core/builtin-fx-param params "wet"))
        (slope-p (eseq.effects.builtin.filter-core/builtin-fx-param params "slope"))
        (lfo-amt-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lfo amt"))
        (lfo-rate-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lfo rate"))
        (lfo-sync-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lfo sync"))
        (lfo-div-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lfo div"))
        (lfo-wave-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lfo wave"))
        (lfo-phase-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lfo phase")))
      (if (and mode-p cutoff-p resonance-p)
        (v-stack :gap 0.15
          (h-stack :gap 0.34 :align :start
            (box :width 5.45 :height 6.60 :padding 0.28
              :background-color :fx-inner-panel-bg :corner-radius 7
              (v-stack :gap 0.3
                (eseq.effects.builtin.filter-core/builtin-fx-filter-cutoff-knob fx cutoff-p)
                (eseq.effects.builtin.filter-core/builtin-fx-filter-resonance-knob fx resonance-p)))
              (box :width 28.8 :height 8.00
                ;; Own subtree: the curve's band dict reads param state, so any
                ;; future body-level read reruns one widget instead of the whole
                ;; Filter panel (which is what made curve drags 12x a knob).
                (subtree :key (eseq.effects.builtin.filter-core/builtin-fx-param-subtree-key fx cutoff-p "curve")
                (response-curve-editor
                  :mode :filter
                  :bands (list (eseq.effects.builtin.filter-core/builtin-fx-filter-band fx mode-p cutoff-p resonance-p))
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
                  :on-action |event| (eseq.effects.builtin.filter-core/builtin-fx-handle-filter-curve-action fx cutoff-p resonance-p event))))
              (box :width 8.3 :height 6.30  :padding 0.28
                :background-color :fx-inner-panel-bg :corner-radius 7
                (v-stack :gap 0.2
                  (if drive-p (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "drive" drive-p) (box :width 0 :height 0))
                  (if wet-p (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "wet" wet-p) (box :width 0 :height 0))
                  (subtree :key (eseq.effects.builtin.filter-core/builtin-fx-param-subtree-key fx mode-p "mode")
                    (dropdown :value (get mode-p :text-value)
                      :options (get mode-p :options)
                      :on-change (lambda (v) (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx mode-p v))
                      :plock-active (if (eseq.effects.param-controls/param-plock-active? fx mode-p) 1 0)
                      :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
                      :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
                      :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
                      :width 7.7 :height 1.05 :font-size 9.5))
                  (if slope-p (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-option fx slope-p) (box :width 0 :height 0)))))
          (box :width 43.2 :height 1.4 :padding 0.2
            :background-color :fx-inner-panel-bg :corner-radius 7
            (h-stack :gap 0.5 :align :baseline
              (label "LFO" :font-size 9.0 :width 2.4 :color :dim :bg :transparent)
              (if lfo-amt-p (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "amt" lfo-amt-p) (box :width 0 :height 0))
              (if lfo-sync-p
                (subtree :key (eseq.effects.builtin.filter-core/builtin-fx-param-subtree-key fx lfo-sync-p "lfo-sync")
                  (dropdown :value (eseq.effects.builtin.filter-core/builtin-fx-filter-sync-label fx lfo-sync-p)
                    :options '("free" "sync")
                    :on-change (lambda (v) (eseq.effects.param-controls/fx-set-effect-value fx lfo-sync-p (if (= v "sync") 1 0)))
                    :plock-active (if (eseq.effects.param-controls/param-plock-active? fx lfo-sync-p) 1 0)
                    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
                    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
                    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
                    :width 4.8 :height 1.05 :font-size 9.5))
                (box :width 0 :height 0))
              (if (and lfo-sync-p (eseq.effects.param-controls/fx-param-on-for? fx lfo-sync-p) lfo-div-p)
                (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-option fx lfo-div-p)
                (if lfo-rate-p (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-number fx "rate" lfo-rate-p) (box :width 0 :height 0)))
              (if lfo-wave-p (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-option fx lfo-wave-p) (box :width 0 :height 0))
              (if lfo-phase-p (eseq.effects.builtin.filter-core/builtin-fx-filter-mini-percent fx "phs" lfo-phase-p) (box :width 0 :height 0)))))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
