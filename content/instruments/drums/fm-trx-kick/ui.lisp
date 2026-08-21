(def trx-body-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "BODY CORE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tune" "tune" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "amp_decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body_level" "body" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sub_level" "sub" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def trx-shape-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "SHAPE / SUB" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body_shape" "shape" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "harmonic" "harm" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "phase_bite" "bite" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sub_ratio" "ratio" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def trx-global-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-base-note 4.2 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "gain" "gain" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "low_cut" "lowcut" 4.2 0 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 0 "drift" "drift" 4.2 3 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def trx-sweep-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "PITCH MACHINE" (eseq.effects.custom-ui-lego/ui-accent-blue) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "bend" "bend" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "bend_decay" "time" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "punch" "punch" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "punch_decay" "snap" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0))))

(def trx-fm-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "FM KNOCK" (eseq.effects.custom-ui-lego/ui-accent-green) 1
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "fm_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "fm_ratio" "ratio" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "fm_decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "fm_feedback" "fb" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def trx-decay-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "DECAY SPLIT" (eseq.effects.custom-ui-lego/ui-accent-cyan) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "body_decay" "body" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "sub_decay" "sub" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "amp_release" "rel" 4.7 0 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def trx-click-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "CLICK" (eseq.effects.custom-ui-lego/ui-accent-orange) 2
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "click_level" "level" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "click_tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "click_decay" "decay" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "click_noise_mix" "noise" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))

(def trx-noise-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "SNAP / AIR" (eseq.effects.custom-ui-lego/ui-accent-violet) 2
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "noise_level" "snap" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "noise_tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "noise_color" "color" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 2 "air_level" "air" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))

(def trx-noise-detail ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "NOISE DETAIL" (eseq.effects.custom-ui-lego/ui-accent-blue) 2
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "noise_decay" "decay" 4.7 0 false (eseq.effects.custom-ui-lego/ui-accent-violet))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "noise_q" "q" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 2 "click_sweep" "c-swp" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(def trx-drive-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "DIRT / TONE" (eseq.effects.custom-ui-lego/ui-accent-green) 3
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 3 "drive" "drive" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 3 "dirt" "dirt" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 3 "squash" "squash" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 3 "tone" "tone" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 0))))

(def trx-tone-detail ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TONE DETAIL" (eseq.effects.custom-ui-lego/ui-accent-green) 3
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 3 "body_tone" "body" 4.7 0 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 3 "resonance" "res" 4.7 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 3 "amp_attack" "atk" 4.7 0 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def trx-machine-readout ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "MACHINE" (eseq.effects.custom-ui-lego/ui-accent-cyan) 3
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "body" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "+ fm" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent)
      (label "+ click" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent)
      (label "+ snap" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (trx-body-block)
      (trx-shape-block)
      (trx-global-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (trx-sweep-block)
      (trx-fm-block)
      (trx-decay-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (trx-click-block)
      (trx-noise-block)
      (trx-noise-detail))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (trx-drive-block)
      (trx-tone-detail)
      (trx-machine-readout))))