;; Operator — per-operator identity expressed through semantic theme colors.
;; Clicking an op panel, filter panel, or center tab swaps the center view —
;; envelope detail above context controls and the tab strip below.

(def opx-gold   () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def opx-teal   () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def opx-coral  () :red)
(def opx-violet () (eseq.effects.custom-ui-lego/ui-accent-violet))
(def opx-cream  () :fg)
(def opx-ice    () (eseq.effects.custom-ui-lego/ui-accent-blue))

(def opx-surf-warm () :instrument-group-bg)
(def opx-surf-cool () :instrument-group-bg)
(def opx-surf-dark () :instrument-control-bg)

;; selected section's panel goes transparent (Ableton-style: the rack
;; background shows through); unselected panels keep their darker surface
(def opx-warm-surface (section)
  (eseq.effects.custom-ui-lego/ui-lego-sel-surface section :transparent (opx-surf-warm)))
(def opx-cool-surface (section)
  (eseq.effects.custom-ui-lego/ui-lego-sel-surface section :transparent (opx-surf-cool)))

(def opx-bord-warm () :border-inactive)
(def opx-bord-cool () :border-inactive)
(def opx-bord-dark () :border-inactive)

(def opx-panel-dense (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) surface border stripe body))
(def opx-panel-small (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) surface border stripe body))
(def opx-panel-strip (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (* 2.0 (eseq.effects.custom-ui-lego/ui-lego-strip-w)) (eseq.effects.custom-ui-lego/ui-lego-full-h) surface border stripe body))

(def opx-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

(def opx-wave-options ()
  '("sine" "sin4b" "sin8b" "tri" "saw" "sqr" "noise" "user"))

(def opx-env-mode-options ()
  '("norm" "loop" "sync"))

(def opx-sync-div-options ()
  '("1/1" "1/2" "1/4" "1/8" "1/16" "1/32"))

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

;; one operator panel — slim: tab letter, coarse+fine knobs, level fader.
;; Wave / ratio-fixed / vel live in the center oscillator view for the
;; selected op. Click anywhere on the panel to drive the center view.
(def opx-op-block (section tab-text accent coarse-p fine-p level-p)
  (opx-panel-dense section (opx-warm-surface section) (opx-bord-warm) accent
    (h-stack :width :fill :height :fill :gap 0.50 :align :center
      (box :width 1.4 :height :fill :v-align :center
        (eseq.effects.custom-ui-lego/ui-lego-tab-s section tab-text 1.4 1.05 accent :black))
      (h-stack :gap 0.14 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s section coarse-p "coarse" 4.2 accent 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s section fine-p "fine" 4.2 (opx-cream) 2))
      (eseq.effects.custom-ui-lego/ui-lego-fader-s section level-p 2.3 1.95 accent 1 false))))

(def opx-opa-block ()
  (opx-op-block 0 "A" (opx-gold) "opa_coarse" "opa_fine" "opa_level_db"))
(def opx-opb-block ()
  (opx-op-block 1 "B" (opx-teal) "opb_coarse" "opb_fine" "opb_level_db"))
(def opx-opc-block ()
  (opx-op-block 2 "C" (opx-coral) "opc_coarse" "opc_fine" "opc_level_db"))
(def opx-opd-block ()
  (opx-op-block 3 "D" (opx-violet) "opd_coarse" "opd_fine" "opd_level_db"))

;; FM router: algorithm + FM drive + feedback
(def opx-algo-block ()
  (opx-panel-small 0 (opx-surf-warm) (opx-bord-warm) (opx-gold)
    (opx-small-row
      (h-stack :gap 0.52 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "FM" 2.0 (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "algorithm" "algorithm" 5.4 (opx-alg-options) (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fm_drive_db" "drive" 3.8 1 "dB" (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "feedback" "feedbk" 3.8 2 false (opx-coral))))))

;; per-op enables
(def opx-ops-block ()
  (opx-panel-small 2 (opx-surf-warm) (opx-bord-warm) false
    (opx-small-row
      (h-stack :gap 0.42 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "OPS" 2.6 (opx-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "opa_on" "A" 4.0 (opx-onoff-options) (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "opb_on" "B" 4.0 (opx-onoff-options) (opx-teal))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "opc_on" "C" 4.0 (opx-onoff-options) (opx-coral))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "opd_on" "D" 4.0 (opx-onoff-options) (opx-violet))))))

(def opx-global-block ()
  (opx-panel-small 4 (opx-surf-cool) (opx-bord-dark) false
    (opx-small-row
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 4 "GLB" 2.4 (opx-cream))
        (box :width 0.8)
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 4 4.0 (opx-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 4 "glide_ms" "glide" 3.6 0 "ms" (opx-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 4 "volume_db" "vol" 4.4 1 "dB" (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 4 "tone" "tone" 3.4 2 false (opx-cream))))))

;; ---------------------------------------------------------------------------
;; Center column — mode-driven detail (selected section picks the view)
;; ---------------------------------------------------------------------------

;; envelope detail: ADSR editor + value readouts, accent-tinted per mode
(def opx-env-detail (section title accent attack decay sustain release)
  (eseq.effects.custom-ui-lego/ui-detail-adsr-body-x-s section title accent attack decay sustain release))

;; user-drawn waveform detail: 16 packed partial faders (draw the spectrum;
;; select wave "user" on any operator to play it)
(def opx-partial-fader (name)
  (eseq.effects.custom-ui-lego/ui-lego-vfader-s 5 name 1.28 2.05 (opx-gold)))

(def opx-partials-detail ()
    (v-stack :width :fill :height :fill :gap 0.10 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-header-s 5 "USER WAVE — PARTIALS 1-16" 14.0 (opx-gold))
      (h-stack :gap 0.0 :align :start
        (opx-partial-fader "partial_1")
        (opx-partial-fader "partial_2")
        (opx-partial-fader "partial_3")
        (opx-partial-fader "partial_4")
        (opx-partial-fader "partial_5")
        (opx-partial-fader "partial_6")
        (opx-partial-fader "partial_7")
        (opx-partial-fader "partial_8")
        (opx-partial-fader "partial_9")
        (opx-partial-fader "partial_10")
        (opx-partial-fader "partial_11")
        (opx-partial-fader "partial_12")
        (opx-partial-fader "partial_13")
        (opx-partial-fader "partial_14")
        (opx-partial-fader "partial_15")
        (opx-partial-fader "partial_16"))))

(def opx-center-detail ()
  (if (= custom-ui-selected-section 5)
    (opx-partials-detail)
    (if (= custom-ui-selected-section 4)
      (opx-env-detail 4 "FILTER ENV" (opx-ice)
        "fenv_attack" "fenv_decay" "fenv_sustain" "fenv_release")
      (if (= custom-ui-selected-section 3)
        (opx-env-detail 3 "OP D ENV" (opx-violet)
          "opd_attack" "opd_decay" "opd_sustain" "opd_release")
        (if (= custom-ui-selected-section 2)
          (opx-env-detail 2 "OP C ENV" (opx-coral)
            "opc_attack" "opc_decay" "opc_sustain" "opc_release")
          (if (= custom-ui-selected-section 1)
            (opx-env-detail 1 "OP B ENV" (opx-teal)
              "opb_attack" "opb_decay" "opb_sustain" "opb_release")
            (opx-env-detail 0 "OP A ENV" (opx-gold)
              "opa_attack" "opa_decay" "opa_sustain" "opa_release")))))))

;; oscillator sub-view for the selected op: wave / ratio-fixed / vel / env mode
(def opx-osc-sub (section letter accent wave-p fixed-p freq-p vel-p envmode-p)
  (box :width :fill :height 2.55
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.54 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s section (str "OSC " letter) 3.4 accent)
	(box :width 0.2)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s section wave-p "wave" 4.6 (opx-wave-options) accent)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s section vel-p "vel" 2.8 2 false (opx-cream)))
      (h-stack :gap 0.24 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s section fixed-p "mode" 4.5 (opx-fixed-options) (opx-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s section freq-p "fixed hz" 4.8 1 "Hz" (opx-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s section envmode-p "env" 4.7 (opx-env-mode-options) accent)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s section "env_loop_rate_hz" "loop" 4.0 1 "Hz" (opx-teal))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s section "env_sync_div" "sync" 4.2 (opx-sync-div-options) (opx-teal))))))

;; filter-envelope sub-view: amount + mode (+ shared loop/sync clocks)
(def opx-filterenv-sub ()
  (box :width :fill :height 2.55
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.24 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 4 "FILTER ENV" 6.4 (opx-ice))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 4 "fenv_amt" "env amt" 3.6 1 false (opx-ice))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 4 "fenv_mode" "mode" 5.3 (opx-env-mode-options) (opx-ice)))
      (h-stack :gap 1.54 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 4 "env_loop_rate_hz" "loop" 3.4 1 "Hz" (opx-teal))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 4 "env_sync_div" "sync" 4.5 (opx-sync-div-options) (opx-teal))))))

;; user-wave sub-view: normalize toggle
(def opx-userwave-sub ()
  (box :width :fill :height 2.55
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 1.42 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 5 "USER WAVE" 6.5 (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 5 "user_norm" "normalize" 4.0 (opx-onoff-options) (opx-gold)))
      (label "set wave 'user' on an operator to hear it" :font-size 8.0 :color :dim :bg :transparent))))

(def opx-center-sub ()
  (if (= custom-ui-selected-section 5)
    (opx-userwave-sub)
    (if (= custom-ui-selected-section 4)
      (opx-filterenv-sub)
      (if (= custom-ui-selected-section 3)
        (opx-osc-sub 3 "D" (opx-violet) "opd_wave" "opd_fixed" "opd_freq_hz" "opd_vel" "opd_env_mode")
        (if (= custom-ui-selected-section 2)
          (opx-osc-sub 2 "C" (opx-coral) "opc_wave" "opc_fixed" "opc_freq_hz" "opc_vel" "opc_env_mode")
          (if (= custom-ui-selected-section 1)
            (opx-osc-sub 1 "B" (opx-teal) "opb_wave" "opb_fixed" "opb_freq_hz" "opb_vel" "opb_env_mode")
            (opx-osc-sub 0 "A" (opx-gold) "opa_wave" "opa_fixed" "opa_freq_hz" "opa_vel" "opa_env_mode")))))))

;; mode tabs: filled with accent when selected (Ableton's little filled box)
(def opx-mode-tab (section text width accent)
  (eseq.effects.custom-ui-lego/ui-lego-mode-tab-s section text width 0.98 accent))

(def opx-mode-tabs ()
  (box :width :fill :height 1.30 :v-align :center
    (h-stack :gap 0.30 :align :center
      (opx-mode-tab 0 "A" 2.6 (opx-gold))
      (opx-mode-tab 1 "B" 2.6 (opx-teal))
      (opx-mode-tab 2 "C" 2.6 (opx-coral))
      (opx-mode-tab 3 "D" 2.6 (opx-violet))
      (opx-mode-tab 4 "FILT" 4.2 (opx-ice))
      (opx-mode-tab 5 "WAVE" 4.2 (opx-gold)))))

;; accent of whichever mode drives the center view (stripe color)
(def opx-center-accent ()
  (if (= custom-ui-selected-section 5) (opx-gold)
    (if (= custom-ui-selected-section 4) (opx-ice)
      (if (= custom-ui-selected-section 3) (opx-violet)
        (if (= custom-ui-selected-section 2) (opx-coral)
          (if (= custom-ui-selected-section 1) (opx-teal)
            (opx-gold)))))))

(def opx-center-height ()
  (+ (* 2.0 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-small-h) (* 2.0 (eseq.effects.custom-ui-lego/ui-lego-gap))))

;; one continuous center panel: detail view / context sub-view / mode tabs,
;; separated by hairline dividers
(def opx-center-column ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s custom-ui-selected-section
    (eseq.effects.custom-ui-lego/ui-lego-col-w) (opx-center-height)
    (opx-surf-dark) (opx-bord-dark) (opx-center-accent)
    (v-stack :width :fill :height :fill :gap 0.16
      (box :width :fill :height 3.7 (opx-center-detail))
      (eseq.effects.custom-ui-lego/ui-lego-divider)
      (box :width :fill :height 2.55 (opx-center-sub))
      (eseq.effects.custom-ui-lego/ui-lego-divider)
      (opx-mode-tabs))))

;; ---------------------------------------------------------------------------
;; Right side — filter / shaper / global, LFO + pitch strips
;; ---------------------------------------------------------------------------

(def opx-filter-block ()
  (opx-panel-dense 4 (opx-cool-surface 4) (opx-bord-cool) (opx-ice)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-header-s 4 "FILTER" 4.2 (opx-ice))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 4 "filter_type" "type" 4.4 (opx-filter-type-options) (opx-ice)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 4 "filter_on" "on" 3.0 (opx-onoff-options) (opx-ice))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 4 "filter_keytrack" "key" 2.9 2 false (opx-ice))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 4 "filter_drive" "drv" 2.9 2 false (opx-coral))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 4 "filter_freq" "freq" 3.7 (opx-ice) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 4 "filter_res" "res" 3.7 (opx-ice) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 4 "filter_morph" "morph" 3.7 (opx-cream) 2)))))

(def opx-shaper-block ()
  (opx-panel-dense 4 (opx-surf-cool) (opx-bord-cool) (opx-coral)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 4 "SHAPER" 4.2 (opx-coral))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 4 "shaper_type" "curve" 4.4 (opx-shaper-options) (opx-coral)))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 4 "shaper_drive_db" "drive" 3.7 (opx-coral) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 4 "shaper_wet" "wet" 3.7 (opx-coral) 2)))))

(def opx-lfo-strip ()
  (opx-panel-strip 5 (opx-surf-cool) (opx-bord-cool) (opx-violet)
    (v-stack :width :fill :gap 0.08 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-header-s 5 "LFO" 5.6 (opx-violet))
      (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 5 "lfo_wave" "wave" 5.6 (opx-lfo-wave-options) (opx-violet))
      (h-stack
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 5 "lfo_mode" "mode" 5.6 (opx-lfo-mode-options) (opx-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 5 "lfo_retrig" "retrig" 5.6 (opx-retrig-options) (opx-gold)))
      (h-stack
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "lfo_rate_hz" "rate" 5.6 2 "Hz" (opx-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "lfo_ratio" "ratio" 5.6 2 false (opx-violet)))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "lfo_amount" "amount" 5.6 2 false (opx-violet))
      (h-stack
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "lfo_to_pitch" "pitch" 5.6 1 "st" (opx-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "lfo_to_filter" "filt" 5.6 1 "oct" (opx-ice))))))

(def opx-pitch-strip ()
  (opx-panel-strip 5 (opx-surf-cool) (opx-bord-warm) (opx-gold)
    (v-stack :width :fill :gap 0.08 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-header-s 5 "PITCH" 5.6 (opx-gold))
      (h-stack
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "penv_amount" "env amt" 5.6 1 "st" (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 5 "penv_mode" "env mode" 5.6 (opx-env-mode-options) (opx-gold)))
      (h-stack
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "penv_attack" "A" 5.6 0 "ms" (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "penv_decay" "D" 5.6 0 "ms" (opx-gold)))
      (h-stack
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "penv_sustain" "S" 5.6 2 false (opx-gold))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "penv_release" "R" 5.6 0 "ms" (opx-gold)))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "transpose" "transp" 5.6 0 "st" (opx-cream))
      (h-stack
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "spread" "spread" 5.6 2 false (opx-cream))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 5 "voice_pan" "pan" 5.6 2 false (opx-cream))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (opx-opa-block)
      (opx-opb-block)
      (opx-algo-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (opx-opc-block)
      (opx-opd-block)
      (opx-ops-block))
    (opx-center-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (opx-filter-block)
      (opx-shaper-block)
      (opx-global-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (opx-lfo-strip)
      (opx-pitch-strip))))
