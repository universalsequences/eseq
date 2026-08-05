;; Shared built-in FX helpers and Filter curve/control primitives.
;; Custom UI bodies for built-in audio effects.

;; NOTE: no drag "live echo" state here on purpose. The curve editor renders
;; its own in-flight drag from widget-local state (LIVE_BANDS in
;; response_curve_editor.rs) and the knob/readout values arrive through the
;; host's targeted SEQV param-value fields (`sync_effect_param_batch_display`),
;; which dirty exactly the bound widgets. Mirroring the drag into `defstate`
;; globals instead re-ran the whole effect panel on every mouse move.

(def builtin-fx-param (params name)
  (nth (filter |p| (= (get p :name) name) params) 0))

;; Chain-scoped subtree key for a single built-in FX param control. The
;; controls below read p-lock state (SEQ.track-plocks / track-plock-variants),
;; so each one lives in its own subtree — a p-lock change reruns only the
;; affected controls instead of the whole effect panel. Keys carry the chain
;; identity so two panels with the same param index can never collide.
(def builtin-fx-param-subtree-scope (fx)
  (if (get fx :rack-fx)
    (str "rack-" (get fx :track-idx) "-" (get fx :rack-slot) "-" (get fx :slot-idx))
    (if (get fx :bus-fx)
      (str "bus-" (get fx :bus-idx) "-" (get fx :slot-idx))
      (if (get fx :midi-fx)
        (str "midi-" (get fx :slot-idx))
        (str "audio-" (get fx :slot-idx))))))

(def builtin-fx-param-subtree-key (fx p tag)
  (str "builtin-fx-" tag "-" (builtin-fx-param-subtree-scope fx) "-param-" (get p :idx)))

(def builtin-fx-filter-mode-type (mode-label)
  (if (= mode-label "highpass")
    "highpass"
    (if (= mode-label "bandpass")
      "bandpass"
      (if (= mode-label "notch")
        "notch"
        "lowpass"))))

(def builtin-fx-filter-cutoff-value (fx cutoff-p)
  (fx-param-value-for fx cutoff-p))

(def builtin-fx-filter-resonance-value (fx resonance-p)
  (fx-param-value-for fx resonance-p))

(def builtin-fx-filter-band (fx mode-p cutoff-p resonance-p)
  (dict
    :id 0
    :type (builtin-fx-filter-mode-type (get mode-p :text-value))
    :freq (builtin-fx-filter-cutoff-value fx cutoff-p)
    :freq-min (param-control-min fx cutoff-p)
    :freq-max (param-control-max fx cutoff-p)
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
    (if (get fx :rack-fx)
      (fx-set-effect-value fx p (custom-ui-option-index (get p :options) label))
      (host-command
      (if (get fx :bus-fx)
        (if (seq-has-selection?) "set-bus-effect-plock-option" "set-bus-effect-param-option")
        (if (seq-has-selection?) "set-effect-plock-option" "set-effect-param-option"))
      (dict :bus (get fx :bus-idx) :slot-idx (get fx :slot-idx)
            :param-idx (get p :idx) :label label)))))

(def builtin-fx-handle-filter-curve-action (fx cutoff-p resonance-p event)
  (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
    (do
      (fx-clear-selected-effect)
      (if (or (get fx :rack-fx) (get fx :bus-fx) (get fx :midi-fx))
        (do
          (fx-set-effect-value fx cutoff-p (get event :freq))
          (fx-set-effect-value fx resonance-p (get event :q)))
        (host-command
          (if (seq-has-selection?) "set-effect-plock-batch" "set-effect-param-batch")
          (dict :slot-idx (get fx :slot-idx)
                :updates (list
                  (dict :param-idx (get cutoff-p :idx) :value (get event :freq))
                  (dict :param-idx (get resonance-p :idx) :value (get event :q)))
                :commit (= (get event :type) :commit-band)))))
    nil))

(def builtin-fx-filter-readout (fx label-text p value width)
  (subtree :key (builtin-fx-param-subtree-key fx p "readout")
    (h-stack :gap 0.18 :align :baseline
      (label label-text :font-size 8.5 :width 3.2 :color :dim :bg :transparent)
      (number-picker :value value
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals 2
        :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :on-change (lambda (v) (param-set-control-value fx p v))
        :width width :height 0.95))))

(def builtin-fx-filter-number (fx label-text p width decimals)
  (subtree :key (builtin-fx-param-subtree-key fx p "num")
    (h-stack :gap 0.22 :align :baseline
      (label label-text :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
      (number-picker :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals decimals
        :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :on-change (lambda (v) (param-set-control-value fx p v))
        :width width :height 1.05))))

(def builtin-fx-filter-percent (fx label-text p width)
  (subtree :key (builtin-fx-param-subtree-key fx p "pct")
    (h-stack :gap 0.22 :align :baseline
      (label label-text :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
      (number-picker :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :value-scale 100 :decimals 0
        :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :on-change (lambda (v) (param-set-control-value fx p v))
        :width width :height 1.05))))

(def builtin-fx-filter-option (fx label-text p width)
  (subtree :key (builtin-fx-param-subtree-key fx p "opt")
    (h-stack :gap 0.22 :align :center
      (label label-text :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
      (dropdown :value (get p :text-value)
        :options (get p :options)
        :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width width :height 1.05 :font-size 9.5))))

(def builtin-fx-filter-sync-label (fx p)
  (if (fx-param-on-for? fx p) "sync" "free"))

(def builtin-fx-filter-sync-control (fx p)
  (subtree :key (builtin-fx-param-subtree-key fx p "sync")
    (h-stack :gap 0.22 :align :center
      (label "sync" :font-size 8.5 :width 4.8 :color :dim :bg :transparent)
      (dropdown :value (builtin-fx-filter-sync-label fx p)
        :options '("free" "sync")
        :on-change (lambda (v) (fx-set-effect-value fx p (if (= v "sync") 1 0)))
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.8 :height 1.05 :font-size 9.5))))

(def builtin-fx-filter-mini-number (fx label-text p)
  (param-mod-wrapper fx p (str "builtin-fx-mini-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (builtin-fx-param-subtree-key fx p "mini-num")
      (h-stack :gap 0.18 :align :baseline
        (label label-text :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
        (number-picker :value (fx-param-value-for fx p)
          :min (param-control-min fx p) :max (param-control-max fx p) :decimals 2
          :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
          :plock-active (if (param-plock-active? fx p) 1 0)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :on-change (lambda (v) (param-set-control-value fx p v))
          :width 4.6 :height 1.0)))))

(def builtin-fx-filter-mini-cutoff (fx p)
  (param-mod-wrapper fx p (str "builtin-fx-mini-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (builtin-fx-param-subtree-key fx p "mini-cut")
      (h-stack :gap 0.18 :align :baseline
        (label "cut" :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
        (number-picker :value (builtin-fx-filter-cutoff-value fx p)
          :min (param-control-min fx p) :max (param-control-max fx p) :decimals 2
          :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
          :plock-active (if (param-plock-active? fx p) 1 0)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :on-change (lambda (v) (param-set-control-value fx p v))
          :width 4.6 :height 1.0)))))

(def builtin-fx-filter-mini-resonance (fx p)
  (subtree :key (builtin-fx-param-subtree-key fx p "mini-res")
    (h-stack :gap 0.18 :align :baseline
      (label "res" :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
      (number-picker :value (builtin-fx-filter-resonance-value fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals 2
        :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :on-change (lambda (v) (param-set-control-value fx p v))
        :width 4.6 :height 1.0))))

(def builtin-fx-filter-cutoff-knob (fx p)
  (param-mod-wrapper fx p (str "fx-slot-" (get fx :slot-idx) "-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (builtin-fx-param-subtree-key fx p "cut-knob")
      (knob-number :label "cut"
        :value (builtin-fx-filter-cutoff-value fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals 0
        :font-size 9.5 :label-font-size 9.5
        :text-color (param-plock-text-color fx p) :label-color :dim
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.65 :height 2.55 :knob-size 1.65
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def builtin-fx-filter-resonance-knob (fx p)
  (subtree :key (builtin-fx-param-subtree-key fx p "res-knob")
    (knob-number :label "res"
      :value (builtin-fx-filter-resonance-value fx p)
      :min (param-control-min fx p) :max (param-control-max fx p) :decimals 2
      :font-size 9.5 :label-font-size 9.5
      :text-color (param-plock-text-color fx p) :label-color :dim
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-default (param-plock-default fx p)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :width 4.65 :height 2.55 :knob-size 1.65
      :on-change (lambda (v) (param-set-control-value fx p v)))))

(def builtin-fx-filter-mini-percent (fx label-text p)
  (param-mod-wrapper fx p (str "builtin-fx-mini-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (builtin-fx-param-subtree-key fx p "mini-pct")
      (h-stack :gap 0.18 :align :baseline
        (label label-text :font-size 8.5 :width 2.35 :color :dim :bg :transparent)
        (number-picker :value (fx-param-value-for fx p)
          :min (param-control-min fx p) :max (param-control-max fx p) :value-scale 100 :decimals 0
          :noui true :font-size 9.5 :text-color (param-plock-text-color fx p)
          :plock-active (if (param-plock-active? fx p) 1 0)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :on-change (lambda (v) (param-set-control-value fx p v))
          :width 4.6 :height 1.0)))))

(def builtin-fx-filter-mini-option (fx p)
  (subtree :key (builtin-fx-param-subtree-key fx p "mini-opt")
    (dropdown :value (get p :text-value)
      :options (get p :options)
      :on-change (lambda (v) (builtin-fx-set-effect-option fx p v))
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :width 5.4 :height 1.05 :font-size 9.5)))
