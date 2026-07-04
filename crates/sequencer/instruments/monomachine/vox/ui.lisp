;; monomachine/vox — VO-6-spirit formant vocal machine.
;; Dense 3-column layout: VOICE/CONS/AMP | VOWEL/LOFI/GLB | GULP filter/FENV.

(def vox-cons-options ()
  '("none" "s" "sh" "f" "t" "k" "p" "h"))

(def vox-voice-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "VOX" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "growl_hz" "grwl hz" 4.4 0 "Hz" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "glide_ms" "glide" 3.4 0 "ms" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "pan_width" "wid" 3.1 2 false (ui-accent-violet))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "glottis" "glot" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "breath" "brth" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "growl" "grwl" 3.7 (ui-accent-orange) 2)))))

(def vox-cons-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 0 "CONS" 3.6 (ui-accent-violet))
          (ui-lego-micro-option-s 0 "cons_type" "type" 4.4 (vox-cons-options) (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "cons_len_ms" "len" 3.4 0 "ms" (ui-accent-violet))
          (ui-lego-micro-num-s 0 "sibilance" "sib" 3.1 2 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 0 "cons_level" "lvl" 3.7 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "cons_duck" "duck" 3.7 (ui-accent-violet) 2)))))

(def vox-amp-block ()
  (ui-control-panel-small-s 0
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 0 "AMP" 3.6 (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp_attack_ms" "atk" 3.0 0 "ms" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp_hold_ms" "hld" 3.0 0 "ms" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp_decay_ms" "dec" 3.0 0 "ms" (ui-accent-orange))
      (ui-lego-micro-num-s 0 "amp_release_ms" "rel" 3.0 0 "ms" (ui-accent-orange)))))

(def vox-vowel-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "VOWL" 3.6 (ui-accent-cyan))
          (ui-lego-micro-num-s 1 "formant_q" "Q" 3.1 1 false (ui-accent-cyan))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "vowel" "vowl" 3.7 (ui-accent-cyan) 0)
        (ui-lego-knob-s 1 "vowel_morph" "mrph" 3.7 (ui-accent-cyan) 2)
        (ui-lego-knob-s 1 "formant_shift" "shft" 3.7 (ui-accent-blue) 2)))))

(def vox-lofi-block ()
  (ui-control-panel-dense-s 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 1 "LOFI" 3.6 (ui-accent-orange))
          (ui-lego-micro-num-s 1 "am_rate" "am rate" 4.4 1 "Hz" (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 1 "eq_freq" "eq frq" 3.7 0 "Hz" (ui-accent-blue))
          (ui-lego-micro-num-s 1 "eq_q" "eq Q" 3.1 1 false (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 1 "am_depth" "AM" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 1 "srr" "srr" 3.7 (ui-accent-orange) 2)
        (ui-lego-knob-s 1 "eq_gain_db" "EQ" 3.7 (ui-accent-blue) 1)))))

(def vox-global-block ()
  (ui-control-panel-small-s 1
    (h-stack :gap 0.18 :align :start
      (ui-lego-badge-s 1 "GLB" 3.6 (ui-accent-green))
      (ui-lego-micro-base-note-s 1 3.0 (ui-accent-green))
      (ui-lego-micro-num-s 1 "gain" "gain" 3.0 2 false (ui-accent-green)))))

(def vox-gulp-block ()
  (ui-control-panel-dense-s 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 2 "GULP" 3.6 (ui-accent-green))
          (ui-lego-micro-num-s 2 "keytrack" "key" 3.1 2 false (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "flt_res_lo" "rLo" 3.1 1 false (ui-accent-green))
          (ui-lego-micro-num-s 2 "flt_res_hi" "rHi" 3.1 1 false (ui-accent-green))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 2 "flt_base" "base" 3.7 (ui-accent-green) 0)
        (ui-lego-knob-s 2 "flt_width" "wdth" 3.7 (ui-accent-green) 2)
        (ui-lego-knob-s 2 "drive" "drv" 3.7 (ui-accent-orange) 2)))))

(def vox-fenv-block ()
  (ui-control-panel-dense-s 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.16 :align :start
          (ui-lego-badge-s 2 "FENV" 3.6 (ui-accent-blue))
          (ui-lego-micro-num-s 2 "fenv_attack_ms" "atk" 4.4 0 "ms" (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 2 "fenv_decay_ms" "dec" 4.4 0 "ms" (ui-accent-blue))))
      (h-stack :gap 0.08 :align :start
        (ui-lego-knob-s 2 "env_to_base" "toB" 3.7 (ui-accent-blue) 2)
        (ui-lego-knob-s 2 "env_to_width" "toW" 3.7 (ui-accent-blue) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.30 :align :stretch
    (ui-lego-column
      (vox-voice-block)
      (vox-cons-block)
      (vox-amp-block))
    (ui-lego-column
      (vox-vowel-block)
      (vox-lofi-block)
      (vox-global-block))
    (ui-lego-column-2
      (vox-gulp-block)
      (vox-fenv-block))))
