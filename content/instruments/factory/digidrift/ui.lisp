;; Factory Drift — warm analog character expressed through semantic theme
;; colors, with source tabs, vertical gain faders, and accent-striped panels.
;; vs core/drift: the filter panel exposes the new filter_drive knob
;; (cut / res / drive); hp cutoff moved to a micro control beside keytrack.

;; Ableton Drift palette discipline: outside the dark oscillator column every
;; knob is drift-knob (blue) and every section header is drift-head (yellow).
;; Inside the dark column each source row owns one colour (osc1 / osc2 /
;; noise) that tints its tab, knobs, fader and toggle.
(def drift-knob   () (eseq.effects.custom-ui-lego/ui-accent-blue))
;(def drift-head   () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def drift-head   () :dim)
(def drift-text   () :fg)
(def drift-orange () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def drift-ice    () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def drift-pink   () :red)

(def drift-surf-warm () :mixer-control-bg)
(def drift-surf-cool () :instrument-group-bg)
(def drift-surf-dark () :instrument-control-bg)

(def drift-bord-warm () :border-inactive)
(def drift-bord-cool () :border-inactive)
(def drift-bord-dark () :border-inactive)

;; The dark oscillator column stacks three equal rows (osc1 / osc2 / noise)
;; in the same height the other columns use, so its knobs are a touch
;; shorter than the 3.36 full-height cells elsewhere.
;; Osc rows are 3.2 tall; the lighter noise row is 2.6 so the three rows
;; read as balanced rather than one sparse row matching two dense ones.
(def drift-row-knob-h () 3.2)
(def drift-noise-knob-h () 2.6)
(def drift-row-knob (name title accent decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 4.2 (drift-row-knob-h) (drift-row-knob-h) accent decimals))
(def drift-noise-knob (name title accent decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 4.2 (drift-noise-knob-h) (drift-noise-knob-h) accent decimals))

(def drift-panel-dense (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) surface border stripe body))
(def drift-panel-small (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) surface border stripe body))
;; Full-height filter column: three oversized knobs wide (20 cells, not the
;; standard 24). Filter knobs are ~1.5x the regular 4.2 x 3.36 cells.
(def drift-filter-col-w () 20.0)
(def drift-filter-knob-w () 6.0)
(def drift-filter-knob-h () 4.9)
(def drift-filter-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 3 name title (drift-filter-knob-w) (drift-filter-knob-h) (drift-filter-knob-h) (drift-knob) decimals))
(def drift-filter-log-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-taper-sized-s 3 name title (drift-filter-knob-w) (drift-filter-knob-h) (drift-filter-knob-h) (drift-knob) decimals "log"))
(def drift-panel-column (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (drift-filter-col-w)
    (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-small-h) (* 2 (eseq.effects.custom-ui-lego/ui-lego-gap)))
    surface border stripe body))

;; small panels: one 1.18-high control row, vertically centered in the panel
(def drift-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

(def drift-wave1-options ()
  '("sine" "tri" "shark" "sat" "saw" "pulse" "rect"))

(def drift-wave2-options ()
  '("sine" "tri" "sat" "saw" "rect"))

(def drift-src-options ()
  '("env1" "env2" "lfo" "key" "vel"))

(def drift-lfo-wave-options ()
  '("sine" "tri" "sawU" "sawD" "sqr" "s&h" "wndr"))

(def drift-dest-options ()
  '("o1 gain" "o1 shp" "o2 gain" "o2 det" "nz gain" "lp frq" "lp res" "hp frq" "vol"))

(def drift-ftype-options ()
  '("I 12dB" "II 24dB"))

(def drift-lfo-mode-options ()
  '("hz" "ratio"))

(def drift-env2-mode-options ()
  '("adsr" "cyc"))

(def drift-retrig-options ()
  '("free" "trig"))


;; Ableton-style filter send: a ">" chip after each source's gain knob that
;; lights in the row colour when the source feeds the filter, dark when dry.
(def drift-route-chip (name height accent)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if p
      (let ((filt (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)) 0.5)))
        (subtree :key (str "drift-route-chip-" name "-" (if filt 1 0))
          (button ">" :width 2.1 :height height
            :font-size 11.0
            :color (if filt :black :dim)
            :background-color (if filt accent :mixer-control-bg)
            :corner-radius 3
            :h-align :center :v-align :center
            :on-click (lambda (x y r)
              (do
                (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 0)
                (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if filt 0 1)))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

;; Oscillator source tab that IS the on/off switch: the solid "1" / "2" block
;; carries the row colour while the oscillator is on and goes dark when off.
(def drift-osc-tab (name text accent)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
      (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if p
      (let ((on (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)) 0.5)))
        (subtree :key (str "drift-osc-tab-" name "-" (if on 1 0))
          (box :width 2.1 :height 1.5 :v-align :end
            (button text :width 2.3 :height 1.5
              :font-size 8.8
              :color (if on :black :dim)
              :background-color (if on accent :mixer-control-bg)
              :corner-radius 3
              :h-align :center :v-align :center
              :on-click (lambda (x y r)
                (do
                  (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 0)
                  (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if on 0 1))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

;; Shape mod (osc1_shape_src/amt), voice_pan and spread stay in the DSP but
;; are not exposed here: shape is @mod and has a MOD-matrix destination, so
;; the dedicated slot only duplicated that.
(def drift-osc1-block ()
  (h-stack :width :fill :height :fill :gap 0.30 :align :center
    (v-stack :width 7.0 :gap 0.18 :align :start
      (h-stack :gap 0.20 :align :end
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc1_wave" "wave" 4.8 (drift-wave1-options) (drift-orange))))
    (h-stack :gap 0.10 :align :center
      (drift-row-knob "osc1_octave" "octave" (drift-orange) 0)
      (drift-row-knob "osc1_shape" "shape" (drift-orange) 2)
      (drift-osc-tab "osc1_on" "1" (drift-orange))
      (drift-row-knob "osc1_gain_db" "gain" (drift-orange) 1))
    (drift-route-chip "osc1_route" 1.5 (drift-orange))))

(def drift-osc2-block ()
  (h-stack :width :fill :height :fill :gap 0.30 :align :center
    (v-stack :width 7.0 :gap 0.18 :align :start
      (h-stack :gap 0.20 :align :end
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc2_wave" "wave" 4.4 (drift-wave2-options) (drift-ice))))
    (h-stack :gap 0.10 :align :center
      (drift-row-knob "osc2_octave" "octave" (drift-ice) 0)
      (drift-row-knob "osc2_detune" "det" (drift-ice) 1)
      (drift-osc-tab "osc2_on" "2" (drift-ice))
      (drift-row-knob "osc2_gain_db" "gain" (drift-ice) 1))
    (drift-route-chip "osc2_route" 1.5 (drift-ice))))

(def drift-source-block ()
  (h-stack :width :fill :height :fill :gap 0.30 :align :center
    (box :width 15.5)
    (box :width 2.3 :height 1.5 :v-align :end
      (eseq.effects.custom-ui-lego/ui-lego-tab-s 0 "N" 2.3 1.5 (drift-pink) :black))
    (h-stack :gap 0.10 :align :start
      (drift-noise-knob "noise_gain_db" "noise" (drift-pink) 0))
    (drift-route-chip "noise_route" 1.6 (drift-pink))))


(def drift-cyc-block ()
  (drift-panel-small 0 (drift-surf-cool) (drift-bord-dark) false
    (drift-small-row
      (h-stack :gap 0.22 :align :end
        (v-stack
          (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "CYC" 2.4 (drift-head))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "env2_mode" "mode" 5.0 (drift-env2-mode-options) (drift-text))
          )
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "cyc_rate_hz" "rate" 4.7 1 "Hz" (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "cyc_tilt" "tilt" 4.7 2 false (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "cyc_hold" "hold" 4.7 2 false (drift-text))))))

(def drift-env-detail ()
  (eseq.effects.custom-ui-lego/ui-detail-adsr-tabs-s 1.3 (drift-head)
    0 "1" "env1_attack" "env1_decay" "env1_sustain" "env1_release"
    1 "2" "env2_attack" "env2_decay" "env2_sustain" "env2_release"))

(def drift-global-block ()
  (drift-panel-small 0 (drift-surf-cool) (drift-bord-dark) false
    (drift-small-row
      (h-stack :gap 0.22 :align :end
        (box :height 1.8 :v-align :start
          (h-stack
            (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "GLB" 2.4 (drift-head))
            (v-stack
              (box :height 0.15)
              (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 3.0 (drift-text))
              )
            ))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "glide_ms" "glide" 5.0 0 "ms" (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "volume_db" "vol" 5.0 1 "dB" (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "vel_to_vol" "vel" 5.0 2 false (drift-text))))))

;; Filter response in place of the envelope plot while the filter column
;; (section 3) is selected. Band 0 is the resonant lowpass (lp_freq / lp_res,
;; draggable); band 1 is the highpass (hp_freq only: no resonance, so its
;; handle is y-locked at the midpoint). Bindings, not value
;; reads, so a drag repaints only this widget (see core/wavetable ui.lisp).
(def drift-filter-detail ()
  (let ((cut-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "lp_freq"))
        (res-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "lp_res"))
        (hp-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "hp_freq"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-lego/ui-readout-panel-medium-s 3
      (v-stack :width :fill :height :fill :gap 0.22 :align :stretch
        (box :width :fill :height 0.72 :h-align :start :v-align :center
          (h-stack (box :width 0.4)
            (label "FILTER" :font-size 8.4 :color :dim :bg :transparent)))
        (if (and cut-p res-p hp-p)
          (response-curve-editor
            :mode :filter
            :bands (list
              (dict :id 0 :type "lowpass"
                :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding cut-p)
                :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min cut-p)
                :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max cut-p)
                :gain 0 :gain-min -12 :gain-max 12
                :q (eseq.effects.custom-ui-runtime/custom-ui-param-binding res-p)
                :q-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min res-p)
                :q-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max res-p)
                ;; Ableton Drift-style resonance plot: gentle through the middle
                ;; (res 0.5 ~ +2 dB), sharp only near the top (res 1 ~ Q 7).
                :q-curve-offset 0.5 :q-curve-scale 6.7 :q-curve-power 3.0
                :enabled true :selected true)
              (dict :id 1 :type "highpass"
                :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding hp-p)
                :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min hp-p)
                :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max hp-p)
                :gain 0 :gain-min -12 :gain-max 12
                ;; the DSP highpass is a fixed-Q svf; draw it Butterworth-flat.
                :q 0.707 :q-min 0 :q-max 1
                :lock-y true
                :enabled true :selected false))
            :freq-min 10
            :freq-max 18000
            :gain-min -12
            :gain-max 12
            :q-min 0
            :q-max 1
            :background-color :instrument-control-bg
            :corner-radius 5
            :grid-color :border-inactive
            :stroke-color (drift-knob)
            :stroke-width 4.5
            :point-color (drift-head)
            :width :fill
            :height 4.0
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
                nil)))
          (label "missing filter params" :font-size 8 :color :red :bg :transparent))))))

(def drift-detail-column ()
  (v-stack :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
    (drift-cyc-block)
    (if (= eseq.vanilla/custom-ui-selected-section 3)
      (drift-filter-detail)
      (drift-env-detail))
    (drift-global-block)))

;; Filter column, directly right of the oscillators (VCO -> Filter): the
;; filter itself on top, its modulation routing below, one full-height panel.
(def drift-filter-column ()
  (drift-panel-column 3 (drift-surf-cool) (drift-bord-cool) false
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.01 :align :start
        (h-stack :gap 0.22 :align :end
          (v-stack
            (eseq.effects.custom-ui-lego/ui-lego-header-s 3 "FILTER" 3.6 (drift-head))
            (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 3 "filter_type" "type" 4.4 (drift-ftype-options) (drift-text))
            )
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "keytrack" "key" 5.0 2 false (drift-text))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "hp_freq" "hp" 5.0 0 "Hz" (drift-text)))
        (h-stack :gap 0.10 :align :start
          (drift-filter-log-knob "lp_freq" "cut" 0)
          (drift-filter-knob "lp_res" "res" 2)
          (drift-filter-knob "filter_drive" "drive" 2))
        (h-stack :gap 0.22 :align :end
          (v-stack
            (eseq.effects.custom-ui-lego/ui-lego-header-s 3 "FMOD" 3.0 (drift-head))
            (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 3 "lp_mod1_src" "src1" 5.2 (drift-src-options) (drift-text))
            )
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "lp_mod1_amt" "amt1" 3.3 1 false (drift-text))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 3 "lp_mod2_src" "src2" 5.2 (drift-src-options) (drift-text))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "lp_mod2_amt" "amt2" 3.3 1 false (drift-text)))))))

;; Pitch modulation as its own full-height column: wide source dropdowns
;; up top, amount/drift knobs (a size between the regular and filter knobs).
(def drift-pitch-col-w () 16.0)
(def drift-pitch-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 1 name title 4.8 4.9 4.9 (drift-knob) decimals))

(def drift-pitch-column ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 1 (drift-pitch-col-w)
    (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-small-h) (* 2 (eseq.effects.custom-ui-lego/ui-lego-gap)))
    (drift-surf-cool) (drift-bord-cool) false
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.3 :align :start
        (h-stack :gap 0.22 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "PITCH" 3.2 (drift-head))
          )
        (h-stack :gap 0.10 :align :start
          (v-stack
            (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 1 "pitch_mod1_src" "src1" 5.4 (drift-src-options) (drift-text))
            (drift-pitch-knob "pitch_mod1_amt" "amt1" 1)
            )
          (v-stack
            (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 1 "pitch_mod2_src" "src2" 5.4 (drift-src-options) (drift-text))
            (drift-pitch-knob "pitch_mod2_amt" "amt2" 1)
            )
          (v-stack
            (box :height 1.1)
            (drift-pitch-knob "drift" "drift" 2))))))
  )

;; LFO and MOD share one column as two tall rows, with dropdowns wide
;; enough for their longest option ("env2", "o1 gain", "lp frq").
(def drift-mod-col-w () 22.0)
(def drift-mod-row-h ()
  (/ (- (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-small-h) (* 2 (eseq.effects.custom-ui-lego/ui-lego-gap)))
        (eseq.effects.custom-ui-lego/ui-lego-gap))
     2))
(def drift-mod-panel (body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 2 (drift-mod-col-w) (drift-mod-row-h) (drift-surf-cool) (drift-bord-cool) false
    (box :width :fill :height :fill :v-align :start body)))

(def drift-lfo-block ()
  (drift-mod-panel
    (v-stack :width :fill :gap 0.40 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "LFO" 2.6 (drift-head))
      (h-stack :gap 0.22 :align :end
        (box :width 2.0)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo_wave" "wave" 5.0 (drift-lfo-wave-options) (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo_mode" "mode" 4.6 (drift-lfo-mode-options) (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo_retrig" "retrig" 4.6 (drift-retrig-options) (drift-text)))
      (h-stack :gap 0.22 :align :end
        (box :width 2.6 :height 0.1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_rate_hz" "rate" 5.0 2 "Hz" (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_ratio" "ratio" 4.6 2 false (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo_amount" "amt" 4.6 2 false (drift-text))))))

(def drift-matrix-block ()
  (drift-mod-panel
    (v-stack :width :fill :gap 0.00 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "MOD" 2.6 (drift-head))
      (h-stack :gap 0.22 :align :end
        (box :width 2.6 :height 0.1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "mm1_src" "src1" 5.2 (drift-src-options) (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "mm1_dest" "dst1" 6.0 (drift-dest-options) (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "mm1_amt" "amt1" 6.0 2 false (drift-text)))
      (h-stack :gap 0.22 :align :end
        (box :width 2.6 :height 0.1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "mm2_src" "src2" 5.2 (drift-src-options) (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "mm2_dest" "dst2" 6.0 (drift-dest-options) (drift-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "mm2_amt" "amt2" 6.0 2 false (drift-text))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    ;(eseq.effects.custom-ui-lego/ui-lego-column
    (box :padding 0.1 :corner-radius 11 :background-color  :mixer-control-bg
      ;; Three knob rows in the column height: row padding and gaps stay
      ;; minimal so the column does not grow past the other columns.
      (v-stack :gap 0.06 :width :fill
        (box :padding 0.04
          (drift-osc1-block)
          )
        (box :width 26 :height 0.05 :background-color :mixer-strip-selected-bg)
        (box :padding 0.04
          (drift-osc2-block)
          )
        (box :width 26 :height 0.05 :background-color :mixer-strip-selected-bg)
        (box :padding 0.04
          (drift-source-block)
          )
        )
      )
    ; )
    (drift-filter-column)
    (drift-detail-column)
    (drift-pitch-column)
    (v-stack :width (drift-mod-col-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
      (drift-lfo-block)
      (drift-matrix-block))))
