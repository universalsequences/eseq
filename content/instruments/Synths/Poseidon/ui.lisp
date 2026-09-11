;; Poseidon: persistent primary controls surround one scoped contextual display.
(def tri-accent () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def tri-ink () :black)
;; One host-owned selection for source, filter, amp, LFO and voice detail.
(def tri-section () eseq.vanilla/custom-ui-selected-section)
(def tri-owner (name)
  (if (string-starts-with? name "osc1_") 0
    (if (string-starts-with? name "osc2_") 1
    (if (or (string-starts-with? name "feg_") (= name "filter_mode") (= name "cutoff") (= name "resonance") (= name "drive") (= name "hp_freq") (= name "keytrack")) 2
    (if (or (string-starts-with? name "aeg_") (= name "vel_to_amp") (= name "voice_pan") (= name "volume_db")) 3
    (if (or (string-starts-with? name "lfo1_") (string-starts-with? name "ams1_")) 4
    (if (or (string-starts-with? name "lfo2_") (string-starts-with? name "ams2_")) 5
    6)))))))
(def tri-knob (name title width height size decimals taper)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s (tri-owner name) name title
    width height size (tri-accent) decimals taper :widget-knob-track 10.5 9.5 :right))
(def tri-panel (section width height body)
  (box :width width :height height :padding 0.12 :corner-radius 2
    :debug-name (str "tri-panel-" section)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    :background-color (if (= (tri-section) section) :instrument-panel-bg :instrument-group-bg) body))
(def tri-switch (name title width)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((on (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)) 0.5)))
      (button title :debug-name (str "tri-switch-" name) :width width :height 0.75 :font-size 8 :padding 0 :corner-radius 1
        :color (if on (tri-ink) :dim)
        :background-color (if on (tri-accent) :instrument-control-bg)
        :on-click (lambda (x y r)
          (do
            (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope (tri-owner name))
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p (if on 0 1))))))))
(def tri-num (name title width decimals ink)
  (tri-readout (tri-owner name) name title false width 1.1 0.5 decimals
    (if (= decimals 0) 1 0.01) ink))
(def tri-option (name options width ink surface)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "tri-option-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "tri-option-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (dropdown :width width :height 0.8 :font-size 8
          :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
          :value-index-offset (get p :min) :options options
          :text-color ink :chevron-color ink :badge-color :transparent
          :bg-color surface :border-color :transparent
          :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
          :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
          :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
          :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
          :on-change (lambda (v)
            (do
              (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope (tri-owner name))
              (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v))))))))))
(def tri-readout (section name title labels width height label-height decimals step ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white ink)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "tri-num-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "tri-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width width :height height :gap 0.06
          (label title :v-align :center :height label-height :font-size 8.6 :color ink :bg :transparent)
          (number-picker :width width :height 0.50 :noui true :decimals decimals :step step :font-size 8.0 :value-labels labels
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) ink)
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (if (number? section)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))))
(def tri-bank-file () "instruments/Synths/Poseidon/waves/bank.json")

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


(def tri-waves-per-set ()
  (let ((metadata (asset-metadata (tri-bank-file))))
    (let ((n (if metadata (get metadata :waves-per-set) nil)))
      (if n n 1))))

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
        :wave-color (tri-ink)
        :inactive-color (rgba 0.0 0.10 0.16 0.45)
        :background-color (tri-accent)
        :width (tri-viewer-w)
        :height (tri-viewer-h))
      (label "missing wavetable params" :font-size 8 :color :red :bg :transparent))))

(def tri-hp-enabled? ()
  (let ((mode-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "filter_mode")))
    (if mode-p
      (> (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value mode-p)) 0.5)
      true)))

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
        (if (and cut-p res-p hp-p)
          (subtree :key (str "tri-filter-curve-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" (if (tri-hp-enabled?) 1 0))
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
            :stroke-color (tri-accent)
            :stroke-width 3
            :point-color (tri-accent)
            :width 35.2
            :height 2.0 :debug-name "tri-filter-response"
            :on-action (lambda (event)
              (if (or (= (get event :type) :change-band)
                  (= (get event :type) :commit-band))
                (if (= (get event :id) 1)
                    (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope hp-p (get event :freq))
                    (do
                      (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope cut-p (get event :freq))
                      (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope res-p (get event :q))))
                nil))))
          (label "missing filter params" :font-size 8 :color :red :bg :transparent))))

(def tri-viewer-w () 35.2)
(def tri-viewer-h () 5.0)
(def tri-select-label (section title width)
  (button title :debug-name (str "tri-select-" section) :width width :height 0.8 :font-size 8 :padding 0
    :color (if (= (tri-section) section) (tri-ink) (tri-accent))
    :background-color (if (= (tri-section) section) (tri-accent) :instrument-control-bg)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))
(def tri-osc-block (section prefix title)
  (tri-panel section 25.5 3.65
    (v-stack :gap 0.2
      (h-stack :gap 0.4
        (tri-select-label section title 5.2)
        (tri-option (str prefix "_set") (tri-set-options) 14 :fg :instrument-control-bg)
        (if (= section 1) (tri-switch "osc2_on" "On" 4.0) (box :width 0 :height 0)))
      (h-stack :gap 0.8
        (tri-knob (str prefix "_wave") "Wave" 7.6 2.2 3.0 0 "linear")
        (if (= section 0)
          (tri-knob "osc1_tune" "Tune ct" 7.6 2.2 3.0 0 "linear")
          (tri-knob "osc2_detune" "Detune st" 7.6 2.2 3.0 1 "linear"))
        (tri-knob (str prefix "_gain_db") "Gain dB" 7.6 2.2 3.0 1 "linear")))))
(def tri-voice-block ()
  (tri-panel 6 25.5 2.3
    (v-stack :gap 0.1
      (tri-select-label 6 "VOICE" 5.8)
      (h-stack :gap 0.6
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 6 7.7 :fg)
        (tri-num "glide_ms" "Glide ms" 7.7 0 :dim)
        (tri-num "spread" "Spread" 7.7 2 :dim)))))
(def tri-env-plot (section prefix height)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :width 35.2 :height height :debug-name "tri-envelope"
      :background-color :instrument-control-bg :curve-color (tri-accent) :point-color (tri-accent)
      :attack (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_attack_ms") 2)
      :decay (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_decay_ms") 150)
      :sustain (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_sustain") 0.85)
      :release (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_release_ms") 180)
      :on-change (lambda (env)
        (do
          (eseq.effects.custom-ui-sections/custom-ui-set-active-adsr scope section (get env :active))
          (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
            (str prefix "_attack_ms") (str prefix "_decay_ms")
            (str prefix "_sustain") (str prefix "_release_ms") env))))))

(def tri-env-controls (prefix)
  (h-stack :gap 0.6
    (tri-num (str prefix "_attack_ms") "Attack ms" 8.3 0 (tri-ink))
    (tri-num (str prefix "_decay_ms") "Decay ms" 8.3 0 (tri-ink))
    (tri-num (str prefix "_sustain") "Sustain" 8.3 2 (tri-ink))
    (tri-num (str prefix "_release_ms") "Release ms" 8.3 0 (tri-ink))))
(def tri-stage-controls (prefix)
  (h-stack :gap 0.6
    (tri-num (str prefix "_break") "Break Level" 8.3 2 (tri-ink))
    (tri-num (str prefix "_slope_ms") "Slope ms" 8.3 0 (tri-ink))))
(def tri-osc-detail (prefix)
  (v-stack :gap 0.3
    (tri-viewer (str prefix "_set") (str prefix "_wave") (str prefix "_warp") (str prefix "_fold"))
    (h-stack :gap 0.8
      (tri-num (str prefix "_octave") "Octave" 8.1 0 (tri-ink))
      (tri-num (str prefix "_vel_wave") "Vel→Wave" 8.1 0 (tri-ink))
      (tri-num (str prefix "_warp") "Warp" 8.1 2 (tri-ink))
      (tri-num (str prefix "_fold") "Fold" 8.1 2 (tri-ink)))))
(def tri-filter-page ()
  (v-stack :gap 0.15
    (tri-filter-detail)
    (tri-env-plot 2 "feg" 2.0)
    (tri-env-controls "feg")
    (h-stack :gap 0.5
      (tri-num "feg_atk_lvl" "Attack Level" 8.3 2 (tri-ink))
      (tri-num "feg_rel_lvl" "Release Level" 8.3 2 (tri-ink))
      (tri-num "feg_int_oct" "Intensity oct" 8.3 1 (tri-ink))
      (tri-num "feg_vel_oct" "Vel→Int oct" 8.3 1 (tri-ink)))
    (h-stack :gap 0.5
      (tri-num "feg_break" "Break Level" 8.3 2 (tri-ink))
      (tri-num "feg_slope_ms" "Slope ms" 8.3 0 (tri-ink))
      (tri-num "keytrack" "Keytrack" 8.3 2 (tri-ink))
      (if (tri-hp-enabled?) (tri-num "hp_freq" "HP Hz" 8.3 0 (tri-ink))
        (label "HP Off" :height 1.1 :width 8.3 :font-size 8 :color (tri-ink) :bg :transparent :v-align :center)))))
(def tri-amp-page ()
  (v-stack :gap 0.3
    (tri-env-plot 3 "aeg" 4.0)
    (tri-env-controls "aeg")
    (tri-stage-controls "aeg")))
(def tri-voice-page ()
  (v-stack :gap 0.4
    (label "PITCH ENVELOPE" :height 0.75 :v-align :center :font-size 8 :color (tri-ink) :bg :transparent)
    (h-stack :gap 0.8
      (tri-num "peg_amt_st" "Amount st" 10.8 1 (tri-ink))
      (tri-num "peg_attack_ms" "Attack ms" 10.8 0 (tri-ink))
      (tri-num "peg_decay_ms" "Decay ms" 10.8 0 (tri-ink)))))
(def tri-lfo-preview (prefix)
  (let ((wave (eseq.effects.custom-ui-controls/ui-param-bound-value (str prefix "_wave") 0)))
    (subtree :key (str "tri-lfo-preview-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" prefix)
      (lfo-curve :width 35.2 :height 3.8 :debug-name "tri-lfo-preview"
        :shape (nth '(0 8 2 1 4) (round (reactive-value wave)))
        :cycles 2 :curve-color (tri-accent) :fill-color :transparent :background-color :instrument-control-bg))))
(def tri-lfo-page (prefix ams target target-title)
  (v-stack :gap 0.3
    (tri-lfo-preview prefix)
    (h-stack :gap 0.6 :align :center
      (tri-option (str prefix "_wave") (tri-lfo-wave-options) 8.3 :fg :instrument-control-bg)
      (tri-option (str prefix "_keysync") (tri-sync-options) 8.3 :fg :instrument-control-bg)
      (tri-num (str prefix "_fade_ms") "Fade ms" 8.3 0 (tri-ink)))
    (h-stack :gap 0.6
      (tri-num target target-title 11 2 (tri-ink))
      (tri-num (str prefix "_to_cutoff") "Cutoff oct" 11 2 (tri-ink)))
    (h-stack :gap 0.6 :align :center
      (tri-option (str ams "_src") (tri-ams-src-options) 10.8 :fg :instrument-control-bg)
      (tri-option (str ams "_dest") (tri-ams-dest-options) 10.8 :fg :instrument-control-bg)
      (tri-num (str ams "_amt") "Mod Amount" 10.8 2 (tri-ink)))))
(def tri-detail-column ()
  (let ((section (tri-section)))
    (box :width 36 :height 9.8 :padding 0.35 :corner-radius 2
      :background-color (tri-accent) :debug-name "tri-detail-display"
      (v-stack :gap 0.3
        (box :width 35.2 :height 0.7 :background-color (tri-ink)
          (label (str "POSEIDON / " (nth '("OSCILLATOR 1" "OSCILLATOR 2" "FILTER" "AMPLITUDE" "LFO 1" "LFO 2" "VOICE") section))
            :width 35.2 :height 0.7 :h-align :center :v-align :center :font-size 8 :color (tri-accent) :bg :transparent))
        (if (= section 0) (tri-osc-detail "osc1")
          (if (= section 1) (tri-osc-detail "osc2")
          (if (= section 2) (tri-filter-page)
          (if (= section 3) (tri-amp-page)
          (if (= section 4) (tri-lfo-page "lfo1" "ams1" "lfo1_to_pitch" "Pitch ct")
          (if (= section 5) (tri-lfo-page "lfo2" "ams2" "lfo2_to_amp" "Amp")
          (tri-voice-page)))))))))))
(def tri-filter-block ()
  (tri-panel 2 23.5 4.85
    (v-stack :gap 0.3
      (h-stack :gap 0.6
        (tri-select-label 2 "FILTER" 6)
        (tri-option "filter_mode" (tri-fmode-options) 15 :fg :instrument-control-bg))
      (h-stack :gap 0.6
        (tri-knob "cutoff" "Cutoff Hz" 7.3 3.0 3.8 0 "log")
        (tri-knob "resonance" "Resonance" 7.3 3.0 3.8 2 "linear")
        (tri-knob "drive" "Drive" 7.3 3.0 3.8 2 "linear")))))
(def tri-amp-block ()
  (tri-panel 3 23.5 4.85
    (v-stack :gap 0.3
      (tri-select-label 3 "AMP" 6)
      (h-stack :gap 0.6
        (tri-knob "vel_to_amp" "Velocity" 7.3 3.0 3.8 2 "linear")
        (tri-knob "voice_pan" "Pan" 7.3 3.0 3.8 2 "linear")
        (tri-knob "volume_db" "Vol dB" 7.3 3.0 3.8 1 "linear")))))
(def tri-lfo-strip (section prefix title)
  (tri-panel section 9.5 4.85
    (v-stack :gap 0.4 :align :center
      (tri-select-label section title 7.8)
      (tri-knob (str prefix "_rate_hz") "Rate Hz" 8.5 3.0 3.8 2 "log"))))
(defsynth-ui
  (h-stack :gap 0.15 :align :start
    (v-stack :gap 0.1
      (tri-osc-block 0 "osc1" "OSC 1") (tri-osc-block 1 "osc2" "OSC 2") (tri-voice-block))
    (tri-detail-column)
    (v-stack :gap 0.1 (tri-filter-block) (tri-amp-block))
    (v-stack :gap 0.1 (tri-lfo-strip 4 "lfo1" "LFO 1") (tri-lfo-strip 5 "lfo2" "LFO 2"))))
