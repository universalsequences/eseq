;; Wavetable — Ableton Wavetable-inspired UI: two oscillator columns with a
;; live wavetable-viewer (selected wave in accent, warp/fold/morph reactive),
;; plus filter / envelope / global column.

(def wt-orange () (rgba 1.00 0.64 0.22 1.0))
(def wt-blue   () (rgba 0.45 0.78 1.00 1.0))
(def wt-cream  () (rgba 0.93 0.88 0.78 1.0))
(def wt-grey   () (rgba 0.46 0.46 0.48 0.55))

(def wt-surf () (rgba 0.075 0.072 0.068 1.0))
(def wt-surf-cool () (rgba 0.146 0.146 0.154 1.0))
(def wt-bord () (rgba 0.30 0.24 0.12 0.060))
(def wt-bord-cool () (rgba 0.14 0.30 0.44 0.060))

(def wt-osc-h () (+ (ui-lego-dense-h) (ui-lego-dense-h) (ui-lego-small-h)
                    (ui-lego-gap) (ui-lego-gap)))

(def wt-bank-file () "instruments/core/wavetable/waves/bank.json")

(def wt-set-options ()
  '("Basic Shapes" "Harmonics" "Sub" "Saw Dual" "Saw Harmonics" "Pulse PW"
    "Quad Saw" "Beating" "5th Brutal" "Sync Additive" "Sync Digital"
    "FM Feedback" "FM Fold" "FM Harmonics" "Primes" "No Primes"
    "Galactica" "Squeeze" "Organ" "Noise" "Vowels" "Choir" "Throat"
    "Talk Box" "Phoneme" "Vox Sync" "SFX Chaos" "Bit Vox"))

(def wt-onoff-options () '("off" "on"))
(def wt-fmode-options () '("LP" "BP" "HP"))

(def wt-viewer (section set-name wave-name warp-name fold-name accent)
  (let ((pset (custom-ui-current-param set-name))
        (pwave (custom-ui-current-param wave-name))
        (pwarp (custom-ui-current-param warp-name))
        (pfold (custom-ui-current-param fold-name)))
    (if (and pset pwave)
      (wavetable-viewer
        :file (wt-bank-file)
        :waves-per-set 16
        :set (custom-ui-param-value pset)
        :wave (custom-ui-param-value pwave)
        :warp (custom-ui-param-value pwarp)
        :fold (custom-ui-param-value pfold)
        :wave-color accent
        :inactive-color (wt-grey)
        :background-color (rgba 0.035 0.038 0.042 1.0)
        :width :fill
        :height 4.9)
      (label "missing wavetable params" :font-size 8 :color :red :bg :transparent))))

(def wt-osc-panel (section tag accent set-name wave-name warp-name fold-name
                   semi-name det-name gain-name extra)
  (ui-lego-panel-x-s section (ui-lego-col-w) (wt-osc-h) (wt-surf) (wt-bord) accent
    (v-stack :width :fill :gap 0.16 :align :start
      (h-stack :gap 0.18 :align :end
        (box :width 1.3 :height 1.18 :v-align :end
          (ui-lego-tab-s section tag 1.3 0.92 accent :black))
        (ui-lego-micro-option-s section set-name "table" 7.6 (wt-set-options) accent)
        extra
        (ui-lego-micro-num-s section semi-name "semi" 2.8 0 "st" (wt-cream))
        (ui-lego-micro-num-s section det-name "det" 2.8 0 "ct" (wt-cream)))
      (wt-viewer section set-name wave-name warp-name fold-name accent)
      (h-stack :gap 0.30 :align :start
        (ui-lego-knob-s section wave-name "wave" 3.7 accent 1)
        (ui-lego-knob-s section warp-name "warp" 3.7 accent 2)
        (ui-lego-knob-s section fold-name "fold" 3.7 accent 2)
        (box :width 0.8)
        (ui-lego-fader-s section gain-name 3.0 1.55 accent 1 "dB")))))

(def wt-osc1-block ()
  (wt-osc-panel 0 "1" (wt-orange)
    "osc1_set" "osc1_wave" "osc1_warp" "osc1_fold"
    "osc1_semi" "osc1_detune" "osc1_gain_db"
    (box :width 0.02 :height 0.1)))

(def wt-osc2-block ()
  (wt-osc-panel 1 "2" (wt-blue)
    "osc2_set" "osc2_wave" "osc2_warp" "osc2_fold"
    "osc2_semi" "osc2_detune" "osc2_gain_db"
    (ui-lego-micro-option-s 1 "osc2_on" "on" 2.6 (wt-onoff-options) (wt-blue))))

;; The filter panel is its own section (3) so that selecting it switches the
;; env detail to the filter envelope; clicking anywhere else shows the amp env.
(def wt-filter-block ()
  (ui-lego-panel-x-s 3 (ui-lego-col-w) (ui-lego-dense-h) (wt-surf-cool) (wt-bord-cool) (wt-blue)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (ui-lego-header-s 3 "FILTER" 4.2 (wt-blue))
          (ui-lego-micro-option-s 3 "filter_mode" "mode" 3.6 (wt-fmode-options) (wt-blue)))
        (h-stack :gap 0.20 :align :start
          (ui-lego-micro-num-s 3 "keytrack" "keytrack" 3.6 2 false (wt-cream))
          (ui-lego-micro-num-s 3 "filter_env_amt" "env amt" 4.4 0 "Hz" (wt-cream))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 3 "cutoff" "cut" 3.7 (wt-blue) 0)
        (ui-lego-knob-s 3 "resonance" "res" 3.7 (wt-blue) 2)))))

(def wt-env-detail ()
  (ui-detail-adsr-switch-s
    2 "AMP ENV" "amp_attack_ms" "amp_decay_ms" "amp_sustain" "amp_release_ms"
    3 "FILT ENV" "filt_attack_ms" "filt_decay_ms" "filt_sustain" "filt_release_ms"))

(def wt-global-block ()
  (ui-lego-panel-x-s 2 (ui-lego-col-w) 1.7 (wt-surf-cool) (wt-bord-cool) false
    (box :width :fill :height :fill :v-align :center
      (h-stack :gap 0.40 :align :end
        (ui-lego-header-s 2 "GLB" 2.4 (wt-cream))
        (ui-lego-micro-base-note-s 2 4.0 (wt-cream))
        (ui-lego-micro-num-s 2 "vel_sens" "vel" 3.4 2 false (wt-cream))
        (ui-lego-micro-num-s 2 "volume_db" "vol" 4.6 1 "dB" (wt-orange))))))

(def wt-detail-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (wt-filter-block)
    (wt-env-detail)
    (wt-global-block)))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (wt-osc1-block)
    (wt-osc2-block)
    (wt-detail-column)))
