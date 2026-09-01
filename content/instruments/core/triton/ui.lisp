;; Triton — Korg workstation character expressed through semantic theme
;; colors, with PCM selectors, multi-stage envelopes, and dual LFO/MOD strips.

(def tri-blue   () (eseq.effects.custom-ui-lego/ui-accent-blue))
(def tri-teal   () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def tri-silver () :fg)
(def tri-amber  () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def tri-violet () (eseq.effects.custom-ui-lego/ui-accent-violet))

(def tri-surf-cool () :instrument-group-bg)
(def tri-surf-dark () :instrument-control-bg)

(def tri-bord-cool () :border-inactive)
(def tri-bord-dark () :border-inactive)

(def tri-panel-dense (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) surface border stripe body))
(def tri-panel-small (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) surface border stripe body))
(def tri-panel-strip (section surface border stripe body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (* 2.0 (eseq.effects.custom-ui-lego/ui-lego-strip-w)) (eseq.effects.custom-ui-lego/ui-lego-full-h) surface border stripe body))

(def tri-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

(def tri-bank-file () "instruments/core/triton/waves/bank.json")

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

(def tri-onoff-options ()
  '("off" "on"))

(def tri-sync-options ()
  '("free" "sync"))

(def tri-osc1-block ()
  (tri-panel-dense 0 (tri-surf-cool) (tri-bord-cool) (tri-blue)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.6 :gap 0.18 :align :start
        (h-stack :gap 0.20 :align :end
          (box :width 1.3 :height 1.18 :v-align :end
            (eseq.effects.custom-ui-lego/ui-lego-tab-s 0 "1" 1.3 0.92 (tri-blue) :black))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc1_set" "pcm set" 4.8 (tri-set-options) (tri-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_octave" "oct" 2.4 0 false (tri-silver)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_tune" "tune" 3.1 0 "ct" (tri-silver))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc1_vel_wave" "vel>wav" 3.4 0 false (tri-teal))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc1_wave" "wave" 3.7 (tri-blue) 0))
      (eseq.effects.custom-ui-lego/ui-lego-fader-s 0 "osc1_gain_db" 2.3 1.95 (tri-blue) 1 false))))

(def tri-osc2-block ()
  (tri-panel-dense 0 (tri-surf-cool) (tri-bord-cool) (tri-teal)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.6 :gap 0.18 :align :start
        (h-stack :gap 0.20 :align :end
          (box :width 1.3 :height 1.18 :v-align :end
            (eseq.effects.custom-ui-lego/ui-lego-tab-s 0 "2" 1.3 0.92 (tri-teal) :black))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc2_set" "pcm set" 4.8 (tri-set-options) (tri-teal))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc2_octave" "oct" 2.4 0 false (tri-silver)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "osc2_on" "on" 3.1 (tri-onoff-options) (tri-teal))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "osc2_vel_wave" "vel>wav" 3.4 0 false (tri-teal))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc2_wave" "wave" 3.7 (tri-teal) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "osc2_detune" "det" 3.7 (tri-silver) 1))
      (eseq.effects.custom-ui-lego/ui-lego-fader-s 0 "osc2_gain_db" 2.3 1.95 (tri-teal) 1 false))))

(def tri-voice-block ()
  (tri-panel-small 0 (tri-surf-cool) (tri-bord-dark) false
    (tri-small-row
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "VOICE" 3.2 (tri-silver))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 (tri-silver))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "glide_ms" "glide" 4.0 0 "ms" (tri-amber))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "spread" "sprd" 4.0 2 false (tri-silver))))))

(def tri-peg-block ()
  (tri-panel-small 0 (tri-surf-cool) (tri-bord-dark) false
    (tri-small-row
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "P.EG" 2.8 (tri-amber))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "peg_amt_st" "amt" 4.0 1 "st" (tri-amber))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "peg_attack_ms" "atk" 4.0 0 "ms" (tri-amber))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "peg_decay_ms" "dec" 4.0 0 "ms" (tri-amber))))))

(def tri-env-detail ()
  (eseq.effects.custom-ui-lego/ui-detail-adsr-switch-s
    0 "AMP EG" "aeg_attack_ms" "aeg_decay_ms" "aeg_sustain" "aeg_release_ms"
    1 "FILTER EG" "feg_attack_ms" "feg_decay_ms" "feg_sustain" "feg_release_ms"))

(def tri-stage-block ()
  (tri-panel-small 0 (tri-surf-cool) (tri-bord-dark) false
    (tri-small-row
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "STAGE" 3.2 (tri-silver))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "aeg_break" "a.brk" 3.6 2 false (tri-silver))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "aeg_slope_ms" "a.slp" 3.6 0 "ms" (tri-silver))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_break" "f.brk" 3.6 2 false (tri-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_slope_ms" "f.slp" 3.6 0 "ms" (tri-blue))))))

(def tri-detail-column ()
  (v-stack :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
    (tri-peg-block)
    (tri-env-detail)
    (tri-stage-block)))

(def tri-filter-block ()
  (tri-panel-dense 1 (tri-surf-cool) (tri-bord-cool) (tri-blue)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "FILTER" 4.2 (tri-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 1 "filter_mode" "mode" 4.6 (tri-fmode-options) (tri-blue)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "keytrack" "keytrk" 3.1 2 false (tri-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "hp_freq" "hp" 3.6 0 "Hz" (tri-silver))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "cutoff" "cut" 3.7 (tri-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "resonance" "res" 3.7 (tri-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "drive" "drive" 3.7 (tri-amber) 2)))))

(def tri-feg-block ()
  (tri-panel-dense 1 (tri-surf-cool) (tri-bord-cool) (tri-teal)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 9.4 :gap 0.18 :align :start
        (h-stack :gap 0.22 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "F.EG" 2.8 (tri-teal))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_start" "start" 3.0 2 false (tri-teal))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_atk_lvl" "atk lv" 3.0 2 false (tri-teal)))
        (h-stack :gap 0.20 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "feg_rel_lvl" "rel lv" 3.0 2 false (tri-teal))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "feg_int_oct" "int" 3.7 (tri-teal) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "feg_vel_oct" "vel>int" 3.7 (tri-silver) 1)))))

(def tri-amp-block ()
  (tri-panel-small 0 (tri-surf-cool) (tri-bord-dark) false
    (tri-small-row
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "AMP" 2.4 (tri-amber))
        (box :width 1)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "vel_to_amp" "vel" 3.6 2 false (tri-amber))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "voice_pan" "pan" 3.6 2 false (tri-silver))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "volume_db" "vol" 4.4 1 "dB" (tri-amber))))))

(def tri-lfo1-strip ()
  (tri-panel-strip 2 (tri-surf-cool) (tri-bord-cool) (tri-violet)
    (v-stack :width :fill :gap 0.08 :align :center
      (h-stack :gap 0.16 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "LFO1" 5.6 (tri-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo1_wave" "wave" 5.6 (tri-lfo-wave-options) (tri-violet)))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo1_rate_hz" "rate" 5.6 2 "Hz" (tri-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo1_fade_ms" "fade" 5.6 0 "ms" (tri-violet)))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo1_keysync" "key" 5.6 (tri-sync-options) (tri-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo1_to_pitch" "pitch" 5.6 0 "ct" (tri-amber)))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo1_to_cutoff" "cutoff" 5.6 2 "oct" (tri-blue))
      (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "MOD A" 5.6 (tri-amber))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ams1_src" "src" 5.6 (tri-ams-src-options) (tri-amber))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ams1_dest" "dest" 5.6 (tri-ams-dest-options) (tri-amber)))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "ams1_amt" "amt" 5.6 2 false (tri-amber)))))

(def tri-lfo2-strip ()
  (tri-panel-strip 2 (tri-surf-cool) (tri-bord-cool) (tri-violet)
    (v-stack :width :fill :gap 0.08 :align :center
      (h-stack :gap 0.16 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "LFO2" 5.6 (tri-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo2_wave" "wave" 5.6 (tri-lfo-wave-options) (tri-violet)))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo2_rate_hz" "rate" 5.6 2 "Hz" (tri-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo2_fade_ms" "fade" 5.6 0 "ms" (tri-violet)))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lfo2_keysync" "key" 5.6 (tri-sync-options) (tri-violet))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo2_to_amp" "amp" 5.6 2 false (tri-amber)))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "lfo2_to_cutoff" "cutoff" 5.6 2 "oct" (tri-blue))
      (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "MOD B" 5.6 (tri-amber))
      (h-stack :gap 0.16 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ams2_src" "src" 5.6 (tri-ams-src-options) (tri-amber))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "ams2_dest" "dest" 5.6 (tri-ams-dest-options) (tri-amber)))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 2 "ams2_amt" "amt" 5.6 2 false (tri-amber)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (tri-osc1-block)
      (tri-osc2-block)
      (tri-voice-block))
    (tri-detail-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (tri-filter-block)
      (tri-feg-block)
      (tri-amp-block))
    (h-stack :width 14.7 :gap 0.30 :align :stretch
      (tri-lfo1-strip)
      (tri-lfo2-strip))))
