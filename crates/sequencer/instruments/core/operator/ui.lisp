;; Operator — FM character: gold/teal/coral/violet per-operator identity,
;; tabbed op panels with level faders, dual ADSR detail switchers,
;; filter+shaper column, LFO / pitch utility strips
;; (built on the ui-lego-panel-x / tab / fader pieces).

(def opx-gold   () (rgba 1.00 0.76 0.30 1.0))
(def opx-teal   () (rgba 0.36 0.86 0.80 1.0))
(def opx-coral  () (rgba 1.00 0.45 0.40 1.0))
(def opx-violet () (rgba 0.72 0.55 1.00 1.0))
(def opx-cream  () (rgba 0.93 0.89 0.80 1.0))
(def opx-ice    () (rgba 0.50 0.76 1.00 1.0))

(def opx-surf-warm () (rgba 0.094 0.086 0.066 1.0))
(def opx-surf-cool () (rgba 0.140 0.146 0.150 1.0))
(def opx-surf-dark () (rgba 0.055 0.058 0.064 1.0))

(def opx-bord-warm () (rgba 0.44 0.32 0.10 0.060))
(def opx-bord-cool () (rgba 0.14 0.36 0.34 0.060))
(def opx-bord-dark () (rgba 0.22 0.22 0.26 0.060))

(def opx-panel-dense (section surface border stripe body)
  (ui-lego-panel-x-s section (ui-lego-col-w) (ui-lego-dense-h) surface border stripe body))
(def opx-panel-small (section surface border stripe body)
  (ui-lego-panel-x-s section (ui-lego-col-w) (ui-lego-small-h) surface border stripe body))
(def opx-panel-strip (section surface border stripe body)
  (ui-lego-panel-x-s section (* 2.0 (ui-lego-strip-w)) (ui-lego-full-h) surface border stripe body))

(def opx-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

(def opx-wave-options ()
  '("sine" "sin4b" "sin8b" "tri" "saw" "sqr" "noise"))

(def opx-fixed-options ()
  '("ratio" "fixed"))

(def opx-alg-options ()
  '("D-C-B-A" "D-C-B|A" "DC>B-A" "D-C|B-A" "BCD>A" "D>CB>A" "D-C,B>A" "D-C|B|A" "B-A|C|D" "D-A|B|C" "A|B|C|D"))

(def opx-onoff-options ()
  '("off" "on"))

(def opx-lfo-wave-options ()
  '("sine" "tri" "sawU" "sawD" "sqr" "s&h"))

(def opx-lfo-mode-options ()
  '("hz" "ratio"))

(def opx-retrig-options ()
  '("free" "trig"))

(def opx-filter-type-options ()
  '("LP12" "LP24" "BP" "HP" "notch" "morph"))

(def opx-shaper-options ()
  '("soft" "hard" "fold" "digi"))

;; one operator panel: tab letter, wave/vel row, ratio-or-fixed row,
;; coarse+fine knobs, level fader (level = carrier volume AND FM index)
(def opx-op-block (section tab-text accent
                   wave-p coarse-p fine-p fixed-p freq-p level-p vel-p)
  (opx-panel-dense section (opx-surf-warm) (opx-bord-warm) accent
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.6 :gap 0.18 :align :start
        (h-stack :gap 0.20 :align :end
          (box :width 1.3 :height 1.18 :v-align :end
            (ui-lego-tab-s section tab-text 1.3 0.92 accent :black))
          (ui-lego-micro-option-s section wave-p "wave" 4.6 (opx-wave-options) accent)
          (ui-lego-micro-num-s section vel-p "vel" 2.8 2 false (opx-cream)))
        (h-stack :gap 0.20 :align :start
          (ui-lego-micro-option-s section fixed-p "mode" 3.4 (opx-fixed-options) (opx-cream))
          (ui-lego-micro-num-s section freq-p "fixed hz" 4.8 1 "Hz" (opx-cream))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s section coarse-p "coarse" 3.7 accent 0)
        (ui-lego-knob-s section fine-p "fine" 3.7 (opx-cream) 2))
      (ui-lego-fader-s section level-p 2.3 1.95 accent 1 false))))

(def opx-opa-block ()
  (opx-op-block 0 "A" (opx-gold)
    "opa_wave" "opa_coarse" "opa_fine" "opa_fixed" "opa_freq_hz" "opa_level_db" "opa_vel"))
(def opx-opb-block ()
  (opx-op-block 1 "B" (opx-teal)
    "opb_wave" "opb_coarse" "opb_fine" "opb_fixed" "opb_freq_hz" "opb_level_db" "opb_vel"))
(def opx-opc-block ()
  (opx-op-block 2 "C" (opx-coral)
    "opc_wave" "opc_coarse" "opc_fine" "opc_fixed" "opc_freq_hz" "opc_level_db" "opc_vel"))
(def opx-opd-block ()
  (opx-op-block 3 "D" (opx-violet)
    "opd_wave" "opd_coarse" "opd_fine" "opd_fixed" "opd_freq_hz" "opd_level_db" "opd_vel"))

;; FM router: algorithm + FM drive + feedback
(def opx-algo-block ()
  (opx-panel-small 0 (opx-surf-warm) (opx-bord-warm) (opx-gold)
    (opx-small-row
      (h-stack :gap 0.52 :align :end
        (ui-lego-header-s 0 "FM" 2.0 (opx-gold))
        (ui-lego-micro-option-s 0 "algorithm" "algorithm" 5.4 (opx-alg-options) (opx-gold))
        (ui-lego-micro-num-s 0 "fm_drive_db" "drive" 3.8 1 "dB" (opx-gold))
        (ui-lego-micro-num-s 0 "feedback" "feedbk" 3.8 2 false (opx-coral))))))

;; per-op enables
(def opx-ops-block ()
  (opx-panel-small 2 (opx-surf-warm) (opx-bord-warm) false
    (opx-small-row
      (h-stack :gap 0.42 :align :end
        (ui-lego-header-s 2 "OPS" 2.6 (opx-cream))
        (ui-lego-micro-option-s 2 "opa_on" "A" 3.0 (opx-onoff-options) (opx-gold))
        (ui-lego-micro-option-s 2 "opb_on" "B" 3.0 (opx-onoff-options) (opx-teal))
        (ui-lego-micro-option-s 2 "opc_on" "C" 3.0 (opx-onoff-options) (opx-coral))
        (ui-lego-micro-option-s 2 "opd_on" "D" 3.0 (opx-onoff-options) (opx-violet))))))

;; envelope detail switchers: click op A/B or C/D panels to swap
(def opx-env-ab-detail ()
  (ui-detail-adsr-switch-s
    0 "OP A ENV" "opa_attack" "opa_decay" "opa_sustain" "opa_release"
    1 "OP B ENV" "opb_attack" "opb_decay" "opb_sustain" "opb_release"))

(def opx-env-cd-detail ()
  (ui-detail-adsr-switch-s
    2 "OP C ENV" "opc_attack" "opc_decay" "opc_sustain" "opc_release"
    3 "OP D ENV" "opd_attack" "opd_decay" "opd_sustain" "opd_release"))

(def opx-global-block ()
  (opx-panel-small 0 (opx-surf-cool) (opx-bord-dark) false
    (opx-small-row
      (h-stack :gap 0.22 :align :end
        (ui-lego-header-s 0 "GLB" 2.4 (opx-cream))
        (box :width 0.8)
        (ui-lego-micro-base-note-s 0 4.0 (opx-cream))
        (ui-lego-micro-num-s 0 "glide_ms" "glide" 3.6 0 "ms" (opx-cream))
        (ui-lego-micro-num-s 0 "volume_db" "vol" 4.4 1 "dB" (opx-gold))
        (ui-lego-micro-num-s 0 "tone" "tone" 3.4 2 false (opx-cream))))))

(def opx-env-column ()
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap)
    (opx-env-ab-detail)
    (opx-env-cd-detail)))

(def opx-filter-block ()
  (opx-panel-dense 4 (opx-surf-cool) (opx-bord-cool) (opx-ice)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (ui-lego-header-s 4 "FILTER" 4.2 (opx-ice))
          (ui-lego-micro-option-s 4 "filter_type" "type" 4.4 (opx-filter-type-options) (opx-ice)))
        (h-stack :gap 0.20 :align :start
          (ui-lego-micro-option-s 4 "filter_on" "on" 3.0 (opx-onoff-options) (opx-ice))
          (ui-lego-micro-num-s 4 "filter_keytrack" "key" 2.9 2 false (opx-ice))
          (ui-lego-micro-num-s 4 "filter_drive" "drv" 2.9 2 false (opx-coral))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 4 "filter_freq" "freq" 3.7 (opx-ice) 0)
        (ui-lego-knob-s 4 "filter_res" "res" 3.7 (opx-ice) 2)
        (ui-lego-knob-s 4 "filter_morph" "morph" 3.7 (opx-cream) 2)))))

(def opx-shaper-block ()
  (opx-panel-dense 4 (opx-surf-cool) (opx-bord-cool) (opx-coral)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (ui-lego-header-s 4 "SHAPER" 4.2 (opx-coral))
          (ui-lego-micro-option-s 4 "shaper_type" "curve" 4.4 (opx-shaper-options) (opx-coral)))
        (h-stack :gap 0.16 :align :start
          (ui-lego-micro-num-s 4 "fenv_attack" "fA" 2.2 0 false (opx-ice))
          (ui-lego-micro-num-s 4 "fenv_decay" "fD" 2.2 0 false (opx-ice))
          (ui-lego-micro-num-s 4 "fenv_sustain" "fS" 2.0 2 false (opx-ice))
          (ui-lego-micro-num-s 4 "fenv_release" "fR" 2.2 0 false (opx-ice))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 4 "shaper_drive_db" "drive" 3.7 (opx-coral) 1)
        (ui-lego-knob-s 4 "shaper_wet" "wet" 3.7 (opx-coral) 2)
        (ui-lego-knob-s 4 "fenv_amt" "env" 3.7 (opx-ice) 1)))))

(def opx-lfo-strip ()
  (opx-panel-strip 5 (opx-surf-cool) (opx-bord-cool) (opx-violet)
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-header-s 5 "LFO" 5.6 (opx-violet))
      (ui-lego-micro-option-s 5 "lfo_wave" "wave" 5.6 (opx-lfo-wave-options) (opx-violet))
      (h-stack
        (ui-lego-micro-option-s 5 "lfo_mode" "mode" 5.6 (opx-lfo-mode-options) (opx-violet))
        (ui-lego-micro-option-s 5 "lfo_retrig" "retrig" 5.6 (opx-retrig-options) (opx-gold)))
      (h-stack
        (ui-lego-micro-num-s 5 "lfo_rate_hz" "rate" 5.6 2 "Hz" (opx-violet))
        (ui-lego-micro-num-s 5 "lfo_ratio" "ratio" 5.6 2 false (opx-violet)))
      (ui-lego-micro-num-s 5 "lfo_amount" "amount" 5.6 2 false (opx-violet))
      (h-stack
        (ui-lego-micro-num-s 5 "lfo_to_pitch" "pitch" 5.6 1 "st" (opx-violet))
        (ui-lego-micro-num-s 5 "lfo_to_filter" "filt" 5.6 1 "oct" (opx-ice))))))

(def opx-pitch-strip ()
  (opx-panel-strip 5 (opx-surf-cool) (opx-bord-warm) (opx-gold)
    (v-stack :width :fill :gap 0.08 :align :center
      (ui-lego-header-s 5 "PITCH" 5.6 (opx-gold))
      (ui-lego-micro-num-s 5 "penv_amount" "env amt" 5.6 1 "st" (opx-gold))
      (h-stack
        (ui-lego-micro-num-s 5 "penv_attack" "A" 5.6 0 "ms" (opx-gold))
        (ui-lego-micro-num-s 5 "penv_decay" "D" 5.6 0 "ms" (opx-gold)))
      (h-stack
        (ui-lego-micro-num-s 5 "penv_sustain" "S" 5.6 2 false (opx-gold))
        (ui-lego-micro-num-s 5 "penv_release" "R" 5.6 0 "ms" (opx-gold)))
      (ui-lego-micro-num-s 5 "transpose" "transp" 5.6 0 "st" (opx-cream))
      (h-stack
        (ui-lego-micro-num-s 5 "spread" "spread" 5.6 2 false (opx-cream))
        (ui-lego-micro-num-s 5 "voice_pan" "pan" 5.6 2 false (opx-cream))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (opx-opa-block)
      (opx-opb-block)
      (opx-algo-block))
    (ui-lego-column
      (opx-opc-block)
      (opx-opd-block)
      (opx-ops-block))
    (opx-env-column)
    (ui-lego-column
      (opx-filter-block)
      (opx-shaper-block)
      (opx-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (opx-lfo-strip)
      (opx-pitch-strip))))
