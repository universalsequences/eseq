;; Digiwave — Monomachine DDRW doubledraw character laid out the poseidon way:
;; one tabbed oscillator panel with a live bank viewer, a detail column whose
;; plot is the tabbed AMP/FLT envelope or the filter response (while the
;; FILTER panel is selected), and a filter / character / EQ column. Every
;; control appears exactly once.

;; One accent: knobs, viewer, headers, and tabs all share the DDRW yellow.
;; Text stays neutral.
(def dw-knob () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def dw-head () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def dw-text () :fg)

(def dw-surf () :instrument-group-bg)
(def dw-bord () :border-inactive)

(def dw-panel-dense (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (dw-surf) (dw-bord) false body))
(def dw-panel-small (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) (dw-surf) (dw-bord) false body))

(def dw-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

(def dw-bank-file () "instruments/Synths/Digi Wave/waves/bank.json")

(def dw-fmode-options ()
  '("LP24" "LP+HP"))

(def dw-bank-options ()
  (let ((metadata (asset-metadata (dw-bank-file))))
    (let ((sets (if metadata (get metadata :sets) nil)))
      (if (and sets (nth sets 0)) sets '("DDRW")))))

(def dw-waves-per-set ()
  (let ((metadata (asset-metadata (dw-bank-file))))
    (let ((n (if metadata (get metadata :waves-per-set) nil)))
      (if n n 64))))

;; Oscillator visibility is independent from the selected modulation section
;; (both oscillators are section 0): a per-scope tab state, like poseidon.
(defstate dw-selected-oscillators '())

(def dw-selected-oscillator-for-scope (scope-name)
  (let ((entry
          (nth
            (filter |item| (= (get item :scope) scope-name)
              dw-selected-oscillators)
            0)))
    (if entry (get entry :oscillator) 0)))

(def dw-selected-oscillator ()
  (dw-selected-oscillator-for-scope (eseq.effects.custom-ui-runtime/custom-ui-scope-name)))

(def dw-set-selected-oscillator-for-scope (scope-name oscillator)
  (set! dw-selected-oscillators
    (cons
      (dict :scope scope-name :oscillator oscillator)
      (filter |item| (not (= (get item :scope) scope-name))
        dw-selected-oscillators))))

(def dw-osc-tab-callback (oscillator)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (lambda (info)
      (do
        (dw-set-selected-oscillator-for-scope (get scope :name) oscillator)
        (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 0)))))

;; Two oscillator panels' worth of height, one tabbed panel.
(def dw-osc-h () (+ (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def dw-osc-w () (eseq.effects.custom-ui-lego/ui-lego-wide-col-w))
(def dw-viewer-w () 14.4)
(def dw-viewer-h () 4.6)

;; Live bank display for the visible oscillator: its selected 64-wave bank
;; (the DDRW sweep is one modulatable range), with the wave the DSP is
;; scanning toward highlighted. :wave binds the param's effective value
;; (base + published modulation offset) so it follows modulation of the wave
;; position, not just the knob.
(def dw-viewer (bank-name wave-name)
  (let ((pbank (eseq.effects.custom-ui-runtime/custom-ui-current-param bank-name))
      (pwave (eseq.effects.custom-ui-runtime/custom-ui-current-param wave-name)))
    (if (and pbank pwave)
      (wavetable-viewer
        :file (dw-bank-file)
        :waves-per-set (dw-waves-per-set)
        :set (eseq.effects.custom-ui-runtime/custom-ui-param-value pbank)
        :wave (eseq.effects.param-controls/param-effective-value pwave)
        :wave-color (dw-knob)
        :inactive-color :dim
        :background-color :instrument-group-bg
        :width (dw-viewer-w)
        :height (dw-viewer-h))
      (label "missing wavetable params" :font-size 8 :color :red :bg :transparent))))

(def dw-osc1-content ()
  (v-stack :width :fill :height :fill :gap 0.16 :align :start
    (h-stack :gap 0.20 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "bank1" "bank" 9.0 (dw-bank-options) (dw-text)))
    (h-stack :width :fill :gap 0.40 :align :center
      (dw-viewer "bank1" "wave1")
      (h-stack :gap 0.30 :align :center
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "wave1" "wave" 4.4 4.2 3.8 (dw-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "crush1" "crush" 4.4 4.2 3.8 (dw-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "mix" "mix" 4.4 4.2 3.8 (dw-knob) 2)))))

(def dw-osc2-content ()
  (v-stack :width :fill :height :fill :gap 0.16 :align :start
    (h-stack :gap 0.20 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "bank2" "bank" 9.0 (dw-bank-options) (dw-text)))
    (h-stack :width :fill :gap 0.40 :align :center
      (dw-viewer "bank2" "wave2")
      (h-stack :gap 0.30 :align :center
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "wave2" "wave" 4.4 4.2 3.8 (dw-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "crush2" "crush" 4.4 4.2 3.8 (dw-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "width_st" "width" 4.4 4.2 3.8 (dw-knob) 1)))))

(def dw-osc-block ()
  (let ((show-2 (= (dw-selected-oscillator) 1))
        (tab-width (/ (- (dw-osc-w) 1.0) 2.0)))
    (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 0 (dw-osc-w) (dw-osc-h) (dw-surf) (dw-bord) false
      (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
        (h-stack :width :fill :height 1.02 :gap 0.0 :align :stretch
          (eseq.effects.custom-ui-lego/ui-lego-underline-tab
            "DRAW 1" tab-width (not show-2) (dw-head)
            (dw-osc-tab-callback 0) "dw-osc-tab-1")
          (eseq.effects.custom-ui-lego/ui-lego-underline-tab
            "DRAW 2" tab-width show-2 (dw-head)
            (dw-osc-tab-callback 1) "dw-osc-tab-2"))
        (eseq.effects.custom-ui-lego/ui-detail-adsr-divider "dw-osc-tabs-divider")
        (box :width :fill :flex 1 :padding 0.12
          (if show-2 (dw-osc2-content) (dw-osc1-content)))))))

(def dw-panel-small-wide (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (dw-osc-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) (dw-surf) (dw-bord) false body))

;; Shared doubledraw controls: how fast both draws glide to a new wave, and
;; the voice basics.
(def dw-draw-block ()
  (dw-panel-small-wide 0
    (dw-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "DRAW" 3.2 (dw-head))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "time" "time" 4.6 0 false (dw-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tune_cents" "tune" 4.6 0 "ct" (dw-text))))))

(def dw-env-detail ()
  (eseq.effects.custom-ui-lego/ui-detail-adsr-tabs-s 2.4 (dw-head)
    0 "AMP" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
    1 "FLT" "filter_attack_ms" "filter_decay_ms" "filter_sustain" "filter_release_ms"))

;; filter_mode: 0 = "LP24 res" (no highpass in the DSP), 1 = "LP12+HP".
(def dw-hp-enabled? ()
  (let ((mode-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "filter_mode")))
    (if mode-p
      (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value mode-p)) 0.5)
      true)))

;; Filter response in place of the envelope plot while the FILTER panel
;; (section 3) is selected. Band 0 is the resonant lowpass (draggable); band
;; 1 is the highpass and only exists in LP12+HP mode, matching the DSP.
(def dw-filter-bands (cut-p res-p hp-p)
  (let ((lp (dict :id 0 :type "lowpass"
                :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding cut-p)
                :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min cut-p)
                :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max cut-p)
                :gain 0 :gain-min -12 :gain-max 12
                :q (eseq.effects.custom-ui-runtime/custom-ui-param-binding res-p)
                :q-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min res-p)
                :q-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max res-p)
                :q-curve-offset 0.5 :q-curve-scale 6.7 :q-curve-power 3.0
                :enabled true :selected true)))
    (if (dw-hp-enabled?)
      (list lp
        (dict :id 1 :type "highpass"
                :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding hp-p)
                :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min hp-p)
                :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max hp-p)
                :gain 0 :gain-min -12 :gain-max 12
                :q 0.707 :q-min 0 :q-max 1
                :lock-y true
                :enabled true :selected false))
      (list lp))))

(def dw-filter-detail ()
  (let ((cut-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "cutoff"))
      (res-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "resonance"))
      (hp-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "hp_freq"))
      (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-lego/ui-readout-panel-medium-s 3
      (v-stack :width :fill :height :fill :gap 0.22 :align :stretch
        (if (and cut-p res-p hp-p)
          (subtree :key (str "dw-filter-curve-" (if (dw-hp-enabled?) 1 0))
          (response-curve-editor
            :mode :filter
            :bands (dw-filter-bands cut-p res-p hp-p)
            :freq-min 10
            :freq-max 18000
            :gain-min -12
            :gain-max 12
            :q-min 0
            :q-max 1
            :background-color :instrument-control-bg
            :corner-radius 5
            :grid-color :border-inactive
            :stroke-color (dw-knob)
            :stroke-width 4.5
            :point-color (dw-head)
            :width :fill
            :height 5.5
            :on-action (lambda (event)
              (if (or (= (get event :type) :change-band)
                  (= (get event :type) :commit-band))
                (do
                  (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 3)
                  (if (= (get event :id) 1)
                    (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope hp-p (get event :freq))
                    (do
                      (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope cut-p (get event :freq))
                      (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope res-p (get event :q)))))
                nil))))
          (label "missing filter params" :font-size 8 :color :red :bg :transparent))))))

(def dw-out-block ()
  (dw-panel-small 0
    (dw-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "OUT" 2.6 (dw-head))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "vel_to_amp" "vel" 5.2 2 false (dw-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "spread" "sprd" 5.2 2 false (dw-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "volume_db" "vol" 5.2 1 "dB" (dw-text))))))

(def dw-voice-block ()
  (dw-panel-small 0
    (dw-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "VOICE" 3.2 (dw-head))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 5.2 (dw-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "glide_ms" "glide" 5.2 0 "ms" (dw-text))))))

(def dw-eq-block ()
  (dw-panel-small 2
    (dw-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "EQ" 2.2 (dw-head))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "eq_freq" "freq" 5.6 0 "Hz" (dw-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "eq_gain_db" "gain" 5.2 1 "dB" (dw-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "eq_q" "q" 4.0 1 false (dw-text))))))

(def dw-detail-column ()
  (v-stack :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
    (dw-out-block)
    (if (= eseq.vanilla/custom-ui-selected-section 3)
      (dw-filter-detail)
      (dw-env-detail))
    (dw-eq-block)))

;; The hp number picker, or an inert dimmed stand-in while LP24 hides it.
(def dw-hp-field ()
  (let ((hp-on (dw-hp-enabled?)))
    (subtree :key (str "dw-hp-field-" (if hp-on 1 0))
      (if hp-on
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "hp_freq" "hp" 5.0 0 "Hz" (dw-text))
        (let ((hp-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "hp_freq")))
          (let ((hp-val (if hp-p (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value hp-p)) 0)))
            (v-stack :width 5.0 :height 1.0 :gap 0.06 :align :start
              (label "hp" :font-size 9.0 :width 5.0 :height 0.75 :color (rgba 0.36 0.37 0.41 1) :bg :transparent)
              (label (str " " (round hp-val) " Hz") :font-size 9.5 :width 5.0 :height 0.85
                :color (rgba 0.36 0.37 0.41 1) :bg (rgba 0.11 0.12 0.14 1)))))))))

(def dw-filter-block ()
  (dw-panel-dense 3
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.0 :gap 0.18 :align :start
        (h-stack :gap 0.82 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-header-s 3 "FILTER" 3.4 (dw-head))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 3 "filter_mode" "mode" 5.4 (dw-fmode-options) (dw-text)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "keytrack" "key" 3.3 2 false (dw-text))
          (dw-hp-field)))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 3 "cutoff" "cut" 3.4 (dw-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 3 "resonance" "res" 3.4 (dw-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "filter_env_amt" "env" 3.4 (dw-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 3 "drive" "drive" 3.4 (dw-knob) 2)))))

;; Monomachine track-chain character: sample-rate reduction and AM.
(def dw-character-block ()
  (dw-panel-dense 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "CHARACTER" 5.6 (dw-head)))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "srr" "srr" 4.2 (dw-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "am_depth" "am" 4.2 (dw-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 2 "am_rate" "am rate" 4.2 (dw-knob) 1)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (v-stack :width (dw-osc-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
      (dw-osc-block)
      (dw-draw-block))
    (dw-detail-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (dw-filter-block)
      (dw-character-block)
      (dw-voice-block))))
