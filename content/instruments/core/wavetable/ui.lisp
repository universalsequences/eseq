;; Wavetable — tabbed oscillators, live wave display, filter, envelopes, and
;; global controls, all expressed through semantic colors from the active theme.

(def wt-orange () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def wt-blue   () (eseq.effects.custom-ui-lego/ui-accent-blue))
(def wt-cream  () :fg)
(def wt-grey   () :dim)

(def wt-surf () :instrument-group-bg)
(def wt-surf-cool () :instrument-group-bg)
(def wt-bord () :border-inactive)
(def wt-bord-cool () :border-inactive)

(def wt-osc-h () (+ (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-small-h)
                    (eseq.effects.custom-ui-lego/ui-lego-gap) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def wt-env-w () (* (eseq.effects.custom-ui-lego/ui-lego-col-w) 1.75))
(def wt-viewer-h () 3.5)

;; Oscillator visibility is independent from the globally selected modulation
;; section. Selecting Filter must not silently switch the visible oscillator.
(defstate wt-selected-oscillators '())

(def wt-selected-oscillator-for-scope (scope-name)
  (let ((entry
          (nth
            (filter |item| (= (get item :scope) scope-name)
              wt-selected-oscillators)
            0)))
    (if entry (get entry :oscillator) 0)))

(def wt-selected-oscillator ()
  (wt-selected-oscillator-for-scope (eseq.effects.custom-ui-runtime/custom-ui-scope-name)))

(def wt-set-selected-oscillator-for-scope (scope-name oscillator)
  (set! wt-selected-oscillators
    (cons
      (dict :scope scope-name :oscillator oscillator)
      (filter |item| (not (= (get item :scope) scope-name))
        wt-selected-oscillators))))

(def wt-osc-tab-callback (section)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (lambda (info)
      (do
        (wt-set-selected-oscillator-for-scope (get scope :name) section)
        (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)))))

(def wt-bank-file () "instruments/core/wavetable/waves/bank.json")

(def wt-bank-metadata ()
  (asset-metadata (wt-bank-file)))

(def wt-set-options (metadata)
  (let ((sets (if metadata (get metadata :sets) nil)))
    (if (and sets (nth sets 0)) sets '("Bank"))))

(def wt-waves-per-set (metadata)
  (let ((waves-per-set (if metadata (get metadata :waves-per-set) nil)))
    (if waves-per-set waves-per-set 1)))

(def wt-fmode-options () '("LP" "BP" "HP"))

(def wt-filter-type (mode)
  (if (< mode 0.5)
    "lowpass"
    (if (< mode 1.5) "bandpass" "highpass")))

;; :freq / :q pass the params' raw bindings, not `eseq.effects.custom-ui-runtime/custom-ui-param-value`.
;; `eseq.effects.custom-ui-runtime/custom-ui-param-value` unwraps with `reactive-value`, an eager read that
;; subscribes this whole custom-UI subtree to the per-param value fields the
;; host rewrites on every drag event — one full subtree rerun per mouse move.
;; response-curve-editor declares `bands` bindable and resolves ReactiveRefs
;; inside each band dict, so the binding repaints just this widget.
;; :type must stay a value read: it feeds a numeric comparison, and the mode
;; field only changes when the mode control does, never during a curve drag.
(def wt-filter-curve-band (mode-p cutoff-p resonance-p)
  (dict
    :id 0
    :type (wt-filter-type (eseq.effects.custom-ui-runtime/custom-ui-param-value mode-p))
    :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding cutoff-p)
    :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min cutoff-p)
    :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max cutoff-p)
    :gain 0
    :gain-min -12
    :gain-max 12
    :q (eseq.effects.custom-ui-runtime/custom-ui-param-binding resonance-p)
    :q-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min resonance-p)
    :q-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max resonance-p)
    :enabled true
    :selected true))

(def wt-filter-curve-callback (cutoff-p resonance-p)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (lambda (event)
      (if (or (= (get event :type) :change-band)
              (= (get event :type) :commit-band))
        (do
          (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 3)
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope cutoff-p (get event :freq))
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope resonance-p (get event :q)))
        nil))))

(def wt-filter-curve ()
  (let ((mode-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "filter_mode"))
        (cutoff-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "cutoff"))
        (resonance-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "resonance")))
    (if (and mode-p cutoff-p resonance-p)
      (response-curve-editor
        :mode :filter
        :bands (list (wt-filter-curve-band mode-p cutoff-p resonance-p))
        :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min cutoff-p)
        :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max cutoff-p)
        :gain-min -12
        :gain-max 12
        :q-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min resonance-p)
        :q-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max resonance-p)
        :background-color :instrument-control-bg
        :corner-radius 5
        :grid-color :border-inactive
        :stroke-color (wt-blue)
        :point-color (wt-orange)
        :width :fill
        :height 5.0
        :on-action (wt-filter-curve-callback cutoff-p resonance-p))
      (label "missing filter params" :font-size 8 :color :red :bg :transparent))))

(def wt-viewer (section set-name wave-name warp-name fold-name accent metadata)
  (let ((pset (eseq.effects.custom-ui-runtime/custom-ui-current-param set-name))
        (pwave (eseq.effects.custom-ui-runtime/custom-ui-current-param wave-name))
        (pwarp (eseq.effects.custom-ui-runtime/custom-ui-current-param warp-name))
        (pfold (eseq.effects.custom-ui-runtime/custom-ui-current-param fold-name)))
    (if (and pset pwave)
      (wavetable-viewer
        :file (wt-bank-file)
        :waves-per-set (wt-waves-per-set metadata)
        :set (eseq.effects.custom-ui-runtime/custom-ui-param-value pset)
        :wave (eseq.effects.custom-ui-runtime/custom-ui-param-value pwave)
        :warp (eseq.effects.custom-ui-runtime/custom-ui-param-value pwarp)
        :fold (eseq.effects.custom-ui-runtime/custom-ui-param-value pfold)
        :wave-color accent
        :inactive-color (wt-grey)
        :background-color :instrument-control-bg
        :width :fill
        :height (wt-viewer-h))
      (label "missing wavetable params" :font-size 8 :color :red :bg :transparent))))

(def wt-osc-content (section accent set-name wave-name warp-name fold-name
                     semi-name det-name gain-name extra)
  (let ((metadata (wt-bank-metadata)))
    (v-stack :width :fill :height :fill :gap 0.16 :align :start
        (h-stack :gap 0.18 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s section set-name "table" 7.6 (wt-set-options metadata) accent)
          extra
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s section semi-name "semi" 2.8 0 "st" (wt-cream))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s section det-name "det" 2.8 0 "ct" (wt-cream)))
        (wt-viewer section set-name wave-name warp-name fold-name accent metadata)
        (h-stack :gap 0.30 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-s section wave-name "wave" 3.7 accent 1)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s section warp-name "warp" 3.7 accent 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-s section fold-name "fold" 3.7 accent 2)
          (box :width 0.8)
          (eseq.effects.custom-ui-lego/ui-lego-fader-s section gain-name 3.0 1.55 accent 1 "dB")))))

(def wt-osc-block ()
  (let ((show-2 (= (wt-selected-oscillator) 1))
        (tab-width (/ (- (eseq.effects.custom-ui-lego/ui-lego-col-w) 1.0) 2.0)))
    (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :height (wt-osc-h)
         :background-color (wt-surf) :corner-radius 7 :border-width 1 :padding 0.18
      (box :width :fill :height :fill :padding 0.04
        (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
          (h-stack :width :fill :height 1.02 :gap 0.0 :align :stretch
            (eseq.effects.custom-ui-lego/ui-lego-underline-tab
              "Osc 1" tab-width (not show-2) (wt-orange)
              (wt-osc-tab-callback 0) "wt-osc-tab-1")
            (eseq.effects.custom-ui-lego/ui-lego-underline-tab
              "Osc 2" tab-width show-2 (wt-blue)
              (wt-osc-tab-callback 1) "wt-osc-tab-2"))
          (eseq.effects.custom-ui-lego/ui-detail-adsr-divider "wt-osc-tabs-divider")
          (box :width :fill :flex 1 :padding 0.12
            (if show-2
              (wt-osc-content 1 (wt-blue)
                "osc2_set" "osc2_wave" "osc2_warp" "osc2_fold"
                "osc2_semi" "osc2_detune" "osc2_gain_db"
                (eseq.effects.custom-ui-lego/ui-lego-micro-toggle-s 1 "osc2_on" 4.0 (wt-blue)))
              (wt-osc-content 0 (wt-orange)
                "osc1_set" "osc1_wave" "osc1_warp" "osc1_fold"
                "osc1_semi" "osc1_detune" "osc1_gain_db"
                (box :width 0.02 :height 0.1)))))))))

;; The filter surface is section 3 so using its curve or controls selects the
;; filter envelope. The curve and large knobs own the visual hierarchy; mode,
;; tracking, envelope amount, base note, and output volume stay compact.
(def wt-filter-small-controls ()
  (v-stack :width 8.4 :height 4.30 :gap 0.12 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 3 "filter_mode" "mode" 8.4 (wt-fmode-options) (wt-blue))
    (h-stack :width :fill :height 1.0 :gap 0.20 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "keytrack" "key" 3.6 2 false (wt-cream))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "filter_env_amt" "env" 4.4 0 "Hz" (wt-cream)))
    (eseq.effects.custom-ui-lego/ui-lego-divider)
    (h-stack :width :fill :height 1.18 :gap 0.40 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 2 3.2 (wt-cream))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "volume_db" "vol" 4.8 1 "dB" (wt-orange)))))

(def wt-filter-block ()
  (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :height (wt-osc-h)
       :background-color (wt-surf-cool) :corner-radius 7
       :border-width 1 :padding 0.18
    (box :width :fill :height :fill :padding 0.04
      (v-stack :width :fill :height :fill :gap 0.15 :align :stretch
        (wt-filter-curve)
        (h-stack :width :fill :height 4.30 :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-big-knob-s 3 "cutoff" "cut" 5.0 (wt-blue) 0)
          (eseq.effects.custom-ui-lego/ui-lego-big-knob-s 3 "resonance" "res" 5.0 (wt-blue) 2)
          (wt-filter-small-controls))))))

(def wt-env-detail ()
  (eseq.effects.custom-ui-lego/ui-detail-adsr-wide-switch-s
    (wt-env-w) (wt-osc-h)
    2 "Amp" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
    3 "Filter" "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms"))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (wt-osc-block)
    (wt-filter-block)
    (wt-env-detail)))
