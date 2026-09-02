;; Poseidon — Korg workstation character expressed through semantic theme
;; colors, with PCM selectors, multi-stage envelopes, and dual LFO/MOD strips.

;; Two accents, Ableton-style: every knob/fader/toggle is tri-knob (blue),
;; every section header/tab is tri-head (yellow). Text stays neutral.
(def tri-knob () (eseq.effects.custom-ui-lego/ui-accent-blue))
(def tri-head () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def tri-text () :fg)

(def tri-surf-cool () :instrument-group-bg)
(def tri-surf-dark () :instrument-control-bg)

(def tri-bord-cool () :border-inactive)
(def tri-bord-dark () :border-inactive)

(def tri-panel-dense (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) surface border stripe body))
(def tri-panel-small (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) surface border stripe body))
(def tri-panel-strip (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (* 2.5 (eseq.effects.custom-ui-lego/ui-lego-strip-w)) (eseq.effects.custom-ui-lego/ui-lego-full-h) surface border stripe body))

(def tri-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

(def tri-bank-file () "instruments/factory/poseidon/waves/bank.json")

(def tri-set-options ()
  (let ((metadata (asset-metadata (tri-bank-file))))
    (let ((sets (if metadata (get metadata :sets) nil)))
      (if (and sets (nth sets 0)) sets '("Bank")))))

(def tri-lfo-wave-options ()
  '("tri" "sawD" "sqr" "sine" "s&h"))

(def tri-ams-src-options ()
  '("f.eg" "a.eg" "lfo1" "lfo2" "key" "vel"))

(def tri-ams-dest-options ()
  '("pitch" "wave1" "wave2" "cutoff" "res" "amp" "pan"))

(def tri-fmode-options ()
  '("LP24 res" "LP12+HP"))

(def tri-sync-options ()
  '("free" "sync"))


;; Oscillator on/off as the builtin toggle widget, with a micro-style title.
(def tri-osc-toggle (name title accent)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
      (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if p
      (let ((on (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)) 0.5)))
        (subtree :key (str "tri-osc-toggle-" name "-" (if on 1 0))
          (v-stack :width 3.4 :height 1.18 :gap 0.16 :align :start
            (label title :font-size 9.0 :width 3.4 :height 0.56 :color :dim :bg :transparent)
            (toggle
              :value on
              :color accent
              :off-color :instrument-control-bg
              :knob-color :black
              :off-knob-color :dim
              :on-change (lambda (next-on)
                (do
                  (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 0)
                  (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if next-on 1 0))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

;; Oscillator visibility is independent from the selected modulation section
;; (both oscillators are section 0): a per-scope tab state, like core/wavetable.
(defstate tri-selected-oscillators '())

(def tri-selected-oscillator-for-scope (scope-name)
  (let ((entry
          (nth
            (filter |item| (= (get item :scope) scope-name)
              tri-selected-oscillators)
            0)))
    (if entry (get entry :oscillator) 0)))

(def tri-selected-oscillator ()
  (tri-selected-oscillator-for-scope (eseq.effects.custom-ui-runtime/custom-ui-scope-name)))

(def tri-set-selected-oscillator-for-scope (scope-name oscillator)
  (set! tri-selected-oscillators
    (cons
      (dict :scope scope-name :oscillator oscillator)
      (filter |item| (not (= (get item :scope) scope-name))
        tri-selected-oscillators))))

(def tri-osc-tab-callback (oscillator)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (lambda (info)
      (do
        (tri-set-selected-oscillator-for-scope (get scope :name) oscillator)
        (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 0)))))

(def tri-waves-per-set ()
  (let ((metadata (asset-metadata (tri-bank-file))))
    (let ((n (if metadata (get metadata :waves-per-set) nil)))
      (if n n 1))))

;; Two oscillator panels' worth of height, one tabbed panel.
(def tri-osc-h () (+ (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def tri-osc-w () (eseq.effects.custom-ui-lego/ui-lego-wide-col-w))
(def tri-viewer-w () 14.4)
(def tri-viewer-h () 4.6)

;; Live PCM wave display for the visible oscillator. :wave binds the param's
;; effective value (base + published modulation offset, as filter-table does)
;; so the highlighted wave follows LFO / envelope / AMS modulation of the wave
;; position, not just the knob. :set stays a plain value read.
(def tri-viewer (set-name wave-name warp-name fold-name)
  (let ((pset (eseq.effects.custom-ui-runtime/custom-ui-current-param set-name))
      (pwave (eseq.effects.custom-ui-runtime/custom-ui-current-param wave-name))
      (pwarp (eseq.effects.custom-ui-runtime/custom-ui-current-param warp-name))
      (pfold (eseq.effects.custom-ui-runtime/custom-ui-current-param fold-name)))
    (if (and pset pwave pwarp pfold)
      (wavetable-viewer
        :file (tri-bank-file)
        :waves-per-set (tri-waves-per-set)
        :set (eseq.effects.custom-ui-runtime/custom-ui-param-value pset)
        :wave (eseq.effects.param-controls/param-effective-value pwave)
        :warp (eseq.effects.param-controls/param-effective-value pwarp)
        :fold (eseq.effects.param-controls/param-effective-value pfold)
        :wave-color (tri-knob)
        :inactive-color :dim
        :background-color :instrument-group-bg
        :width (tri-viewer-w)
        :height (tri-viewer-h))
      (label "missing wavetable params" :font-size 8 :color :red :bg :transparent))))

(def tri-osc1-content ()
  (v-stack :width :fill :height :fill :gap 0.16 :align :start
    (h-stack :gap 0.20 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc1_set" "pcm set" 9.0 (tri-set-options) (tri-text))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_octave" "oct" 2.6 0 false (tri-text))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_tune" "tune" 3.4 0 "ct" (tri-text))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_vel_wave" "vel>wav" 3.6 0 false (tri-text)))
    (h-stack :width :fill :gap 0.40 :align :center
      (tri-viewer "osc1_set" "osc1_wave" "osc1_warp" "osc1_fold")
      (h-stack :gap 0.30 :align :center
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "osc1_wave" "wave" 3.4 3.6 3.2 (tri-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "osc1_warp" "warp" 3.4 3.6 3.2 (tri-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "osc1_fold" "fold" 3.4 3.6 3.2 (tri-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-fader-s 0 "osc1_gain_db" 2.3 1.95 (tri-knob) 1 false)))))

(def tri-osc2-content ()
  (v-stack :width :fill :height :fill :gap 0.16 :align :start
    (h-stack :gap 0.20 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc2_set" "pcm set" 9.0 (tri-set-options) (tri-text))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc2_octave" "oct" 2.6 0 false (tri-text))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc2_detune" "detune" 5.4 1 "st" (tri-text))
      (tri-osc-toggle "osc2_on" "on" (tri-knob))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc2_vel_wave" "vel>wav" 3.6 0 false (tri-text)))
    (h-stack :width :fill :gap 0.40 :align :center
      (tri-viewer "osc2_set" "osc2_wave" "osc2_warp" "osc2_fold")
      (h-stack :gap 0.30 :align :center
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "osc2_wave" "wave" 3.4 3.6 3.2 (tri-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "osc2_warp" "warp" 3.4 3.6 3.2 (tri-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "osc2_fold" "fold" 3.4 3.6 3.2 (tri-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-fader-s 0 "osc2_gain_db" 2.3 1.95 (tri-knob) 1 false)))))

(def tri-osc-block ()
  (let ((show-2 (= (tri-selected-oscillator) 1))
        (tab-width (/ (- (tri-osc-w) 1.0) 2.0)))
    (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 0 (tri-osc-w) (tri-osc-h) (tri-surf-cool) (tri-bord-cool) false
      (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
        (h-stack :width :fill :height 1.02 :gap 0.0 :align :stretch
          (eseq.effects.custom-ui-lego/ui-lego-underline-tab
            "OSC 1" tab-width (not show-2) (tri-head)
            (tri-osc-tab-callback 0) "tri-osc-tab-1")
          (eseq.effects.custom-ui-lego/ui-lego-underline-tab
            "OSC 2" tab-width show-2 (tri-head)
            (tri-osc-tab-callback 1) "tri-osc-tab-2"))
        (eseq.effects.custom-ui-lego/ui-detail-adsr-divider "tri-osc-tabs-divider")
        (box :width :fill :flex 1 :padding 0.12
          (if show-2 (tri-osc2-content) (tri-osc1-content)))))))

(def tri-panel-small-wide (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (tri-osc-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) surface border stripe body))

(def tri-voice-block ()
  (tri-panel-small-wide 0 (tri-surf-cool) (tri-bord-dark) false
    (tri-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "VOICE" 3.2 (tri-head))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "glide_ms" "glide" 4.0 0 "ms" (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "spread" "sprd" 4.0 2 false (tri-text))))))

(def tri-peg-block ()
  (tri-panel-small 0 (tri-surf-cool) (tri-bord-dark) false
    (tri-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "P.EG" 2.8 (tri-head))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "peg_amt_st" "amt" 6.0 1 "st" (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "peg_attack_ms" "atk" 6.0 0 "ms" (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "peg_decay_ms" "dec" 6.0 0 "ms" (tri-text))))))

(def tri-env-detail ()
  (eseq.effects.custom-ui-lego/ui-detail-adsr-tabs-s 2.4 (tri-head)
    0 "AMP" "aeg_attack_ms" "aeg_decay_ms" "aeg_sustain" "aeg_release_ms"
    1 "FLT" "feg_attack_ms" "feg_decay_ms" "feg_sustain" "feg_release_ms"))

(def tri-stage-block ()
  (tri-panel-small 0 (tri-surf-cool) (tri-bord-dark) false
    (tri-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "STAGE" 3.2 (tri-head))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "aeg_break" "a.brk" 3.8 2 false (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "aeg_slope_ms" "a.slp" 5.0 0 "ms" (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_break" "f.brk" 3.8 2 false (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_slope_ms" "f.slp" 5.0 0 "ms" (tri-text))))))

;; Filter response in place of the envelope plot while the FILTER panel
;; (section 3, distinct from the F.EG / FLT-tab section 1) is selected. Band 0 is the resonant lowpass (cutoff /
;; resonance, draggable); band 1 is the highpass (hp_freq only: no resonance,
;; so its handle is y-locked at the midpoint). Bindings, not value reads, so a
;; drag repaints only this widget (ported from digidrift ui.lisp).
;; Band 0 is always the resonant lowpass; band 1 (highpass) only exists
;; in LP12+HP mode, matching the DSP.
(def tri-filter-bands (cut-p res-p hp-p)
  (let ((lp (dict :id 0 :type "lowpass"
                :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding cut-p)
                :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min cut-p)
                :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max cut-p)
                :gain 0 :gain-min -12 :gain-max 12
                :q (eseq.effects.custom-ui-runtime/custom-ui-param-binding res-p)
                :q-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min res-p)
                :q-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max res-p)
                ;; Gentle through the middle, sharp only near the top.
                :q-curve-offset 0.5 :q-curve-scale 6.7 :q-curve-power 3.0
                :enabled true :selected true)))
    (if (tri-hp-enabled?)
      (list lp
        (dict :id 1 :type "highpass"
                :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding hp-p)
                :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min hp-p)
                :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max hp-p)
                :gain 0 :gain-min -12 :gain-max 12
                ;; fixed-Q highpass; draw it Butterworth-flat.
                :q 0.707 :q-min 0 :q-max 1
                :lock-y true
                :enabled true :selected false))
      (list lp))))

(def tri-filter-detail ()
  (let ((cut-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "cutoff"))
      (res-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "resonance"))
      (hp-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "hp_freq"))
      (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-lego/ui-readout-panel-medium-s 3
      (v-stack :width :fill :height :fill :gap 0.22 :align :stretch
        (if (and cut-p res-p hp-p)
          (subtree :key (str "tri-filter-curve-" (if (tri-hp-enabled?) 1 0))
          (response-curve-editor
            :mode :filter
            :bands (tri-filter-bands cut-p res-p hp-p)
            :freq-min 10
            :freq-max 18000
            :gain-min -12
            :gain-max 12
            :q-min 0
            :q-max 1
            :background-color :instrument-control-bg
            :corner-radius 5
            :grid-color :border-inactive
            :stroke-color (tri-knob)
            :stroke-width 4.5
            :point-color (tri-head)
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

(def tri-detail-column ()
  (v-stack :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
    (tri-peg-block)
    (if (= eseq.vanilla/custom-ui-selected-section 3)
      (tri-filter-detail)
      (tri-env-detail))
    (tri-stage-block)))

;; filter_mode: 0 = "LP24 res" (no highpass in the DSP), 1 = "LP12+HP".
;; Read reactively and keyed into a subtree so a mode flip rebuilds only
;; the widgets that depend on it (same pattern as tri-osc-toggle).
(def tri-hp-enabled? ()
  (let ((mode-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "filter_mode")))
    (if mode-p
      (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value mode-p)) 0.5)
      true)))

;; The hp number picker, or an inert dimmed stand-in while LP24 hides it.
(def tri-hp-field ()
  (let ((hp-on (tri-hp-enabled?)))
    (subtree :key (str "tri-hp-field-" (if hp-on 1 0))
      (if hp-on
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "hp_freq" "hp" 6.0 0 "Hz" (tri-text))
        ;; Ghosted twin of the micro-num field: same footprint and strip
        ;; background, muted solid colours (label alpha is not honoured), and
        ;; no widget underneath, so it neither edits nor drags.
        (let ((hp-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "hp_freq")))
          (let ((hp-val (if hp-p (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value hp-p)) 0)))
            (v-stack :width 6.0 :height 1.0 :gap 0.06 :align :start
              (label "hp" :font-size 9.0 :width 6.0 :height 0.68 :color (rgba 0.36 0.37 0.41 1) :bg :transparent)
              (label (str " " (round hp-val) " Hz") :font-size 9.5 :width 6.0 :height 0.75
                :color (rgba 0.36 0.37 0.41 1) :bg (rgba 0.11 0.12 0.14 1)))))))))

(def tri-filter-block ()
  (tri-panel-dense 3 (tri-surf-cool) (tri-bord-cool) false
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-header-s 3 "FILTER" 4.2 (tri-head))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 3 "filter_mode" "mode" 4.6 (tri-fmode-options) (tri-text)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "keytrack" "key" 3.3 2 false (tri-text))
          (tri-hp-field)))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 3 "cutoff" "cut" 4.2 (tri-knob) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 3 "resonance" "res" 4.2 (tri-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 3 "drive" "drive" 4.2 (tri-knob) 2)))))

(def tri-feg-block ()
  (tri-panel-dense 1 (tri-surf-cool) (tri-bord-cool) false
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "F.EG" 2.8 (tri-head))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_atk_lvl" "atk lv" 5.0 2 false (tri-text)))
        (h-stack :gap 0.20 :align :start
          (box :width 2.8)
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_rel_lvl" "rel lv" 5.0 2 false (tri-text))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "feg_int_oct" "int" 4.2 (tri-knob) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "feg_vel_oct" "vel>int" 4.2 (tri-knob) 1)))))

(def tri-amp-block ()
  (tri-panel-small 0 (tri-surf-cool) (tri-bord-dark) false
    (tri-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "AMP" 2.4 (tri-head))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "vel_to_amp" "vel" 6.0 2 false (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "voice_pan" "pan" 6.0 2 false (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "volume_db" "vol" 6.0 1 "dB" (tri-text))))))

(def tri-lfo1-strip ()
  (tri-panel-strip 2 (tri-surf-cool) (tri-bord-cool) false
    (v-stack :width :fill :gap 0.08 :align :left
      (box :height 0.2)
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "LFO1" 5.6 (tri-head))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo1_wave" "wave" 5.6 (tri-lfo-wave-options) (tri-text)))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo1_rate_hz" "rate" 5.6 2 "Hz" (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo1_fade_ms" "fade" 5.6 0 "ms" (tri-text)))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo1_keysync" "key" 5.6 (tri-sync-options) (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo1_to_pitch" "pitch" 5.6 0 "ct" (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo1_to_cutoff" "cutoff" 5.6 2 "oct" (tri-text))
        )
      (box :height 1)
      (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "MOD A" 5.6 (tri-head))
      (h-stack :gap 0.16 :align :baseline
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ams1_src" "src" 5.6 (tri-ams-src-options) (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ams1_dest" "dest" 5.6 (tri-ams-dest-options) (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "ams1_amt" "amt" 5.6 2 false (tri-text))
        )
      )
    )
  )

(def tri-lfo2-strip ()
  (tri-panel-strip 2 (tri-surf-cool) (tri-bord-cool) false
    (v-stack :width :fill :gap 0.08 :align :left
      (box :height 0.2)
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "LFO2" 5.6 (tri-head))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo2_wave" "wave" 5.6 (tri-lfo-wave-options) (tri-text)))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo2_rate_hz" "rate" 5.6 2 "Hz" (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo2_fade_ms" "fade" 5.6 0 "ms" (tri-text)))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo2_keysync" "key" 5.6 (tri-sync-options) (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo2_to_amp" "amp" 5.6 2 false (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo2_to_cutoff" "cutoff" 5.6 2 "oct" (tri-text))
        )
      (box :height 1)
      (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "MOD B" 5.6 (tri-head))        
      (h-stack :gap 0.16 :align :baseline
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ams2_src" "src" 5.6 (tri-ams-src-options) (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ams2_dest" "dest" 5.6 (tri-ams-dest-options) (tri-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "ams2_amt" "amt" 5.6 2 false (tri-text))
        )
      )))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (v-stack :width (tri-osc-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
      (tri-osc-block)
      (tri-voice-block))
    (tri-detail-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (tri-filter-block)
      (tri-feg-block)
      (tri-amp-block))
    (h-stack :width 14.7 :gap 0.05 :align :stretch
      (tri-lfo1-strip)
      (tri-lfo2-strip))))
