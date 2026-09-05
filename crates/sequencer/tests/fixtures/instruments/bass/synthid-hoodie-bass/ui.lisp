(def hoodie-bass-envelope-column ()
  (eseq.effects.custom-ui-lego/ui-lego-column-full
    (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w) :height (eseq.effects.custom-ui-lego/ui-lego-full-h)
      (eseq.effects.custom-ui-lego/ui-adsr-switch
        0 "IDENTIFIED AMP" "amp_attack" "amp_decay" "amp_sustain" "amp_release"
        0 "IDENTIFIED AMP" "amp_attack" "amp_decay" "amp_sustain" "amp_release"))))

(def hoodie-bass-model-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-s "SYNTHID MODEL" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "brightness_decay" "BRIGHT" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "drive" "DRIVE" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "gain" "GAIN" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 3))))

(def hoodie-bass-source-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "RECOVERED SOURCE" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "MONOLOGUE" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-violet) :bg :transparent)
      (label "32 partial" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "steady +" :font-size 9.0 :color :dim :bg :transparent)
      (label "attack bank" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent))))

(def hoodie-bass-validation-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "IDENTIFICATION" (eseq.effects.custom-ui-lego/ui-accent-green) 0
    (eseq.effects.custom-ui-lego/ui-lego-text-row-4
      (label "G# 52.05 Hz" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-green) :bg :transparent)
      (label "MR-STFT" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-cyan) :bg :transparent)
      (label "distance" :font-size 9.0 :color :dim :bg :transparent)
      (label "70.55%" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (hoodie-bass-envelope-column)
    (eseq.effects.custom-ui-lego/ui-lego-column
      (hoodie-bass-model-block)
      (hoodie-bass-source-block)
      (hoodie-bass-validation-block))))
