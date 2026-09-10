;; Original reed and acoustic voice stay independently playable through Morph.
(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM SAXOPHONE"
    (list
      '("ORIGINAL REED" ("stiffness" "Stiffness" 2 :linear) ("brightness" "Brightness" 2 :linear))
      '("ACOUSTIC REED" ("reed.pressure" "Pressure" 2 :linear) ("reed.reed" "Reed slope" 2 :linear))
      '("AIR COLUMN" ("bore.acoustic" "Morph" 2 :linear) ("bore.blow_pos" "Blow position" 2 :linear))
      '("EXPRESSION" ("expression.vib_cent" "Vibrato ct" 1 :linear) ("expression.growl" "Growl" 2 :linear)))
    (list
      (dict :title "Original"
        :view (lambda () (eseq.effects.physical-model-surface/reed-view
          (eseq.effects.physical-model-surface/bind "stiffness") 0.7 -1))
        :controls (lambda () '(("brnoise" "Breath noise" 3)))
        :hint "Morph 0: original reed. Morph 1: acoustic voice.")
      (dict :title "Reed"
        :view (lambda () (eseq.effects.physical-model-surface/reed-view
          (eseq.effects.physical-model-surface/bind "reed.reed")
          (eseq.effects.physical-model-surface/bind "reed.closure") 1))
        :controls (lambda () '(("reed.closure" "Closure" 3) ("reed.air" "Air noise" 3) ("reed.vel_blow" "Velocity > breath" 2)))
        :hint "Acoustic voice / pressure and closure can choke or overblow.")
      (dict :title "Bore"
        :view (lambda () (eseq.effects.physical-model-surface/column-view
          (eseq.effects.physical-model-surface/bind "bore.blow_pos") "Excitation position / fraction of the acoustic bore"))
        :controls (lambda () '(("bore.damping" "Damping" 3) ("bore.bell_hz" "Bell cutoff Hz" 0)))
        :hint "Acoustic voice / position reshapes the harmonic balance.")
      (dict :title "Motion"
        :view (lambda () (eseq.effects.physical-model-surface/vibrato-view))
        :controls (lambda () '(("expression.vib_hz" "Vibrato Hz" 2) ("expression.vib_wait" "Vibrato delay ms" 0)
          ("expression.vib_air" "Breath vibrato" 3) ("expression.growl_hz" "Growl Hz" 1)))
        :hint "Acoustic voice / vibrato fades in; growl starts immediately.")
      (dict :title "Body"
        :view (lambda () (eseq.effects.physical-model-surface/body-view))
        :controls (lambda () '(("color.body" "Body mix" 2) ("color.body_hz" "Body Hz" 0) ("color.body_q" "Body Q" 2)))
        :hint "Acoustic voice / body color follows the bell filter.")
      (dict :title "Amp"
        :view (lambda () (eseq.effects.physical-model-surface/envelope 5))
        :controls (lambda () '(("amp.attack" "Attack ms" 1) ("amp.decay" "Decay ms" 1) ("amp.sustain" "Sustain" 2)
          ("amp.release" "Release ms" 1) ("gain" "Output" 2)))
        :hint "Shared breath contour and output for both voices."))))
