;; Clarinet shares the physical-model family surface and scoped host controls.
(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM CLARINET"
    (list
      '("SINGLE REED" ("reed.pressure" "Pressure" 2 :linear) ("reed.stiffness" "Stiffness" 2 :linear))
      '("CYLINDRICAL BORE" ("bore.damping_hz" "Damping Hz" 0 :log) ("bore.warp" "Warp" 2 :linear))
      '("BODY & BELL" ("color.body" "Body mix" 2 :linear) ("color.bell_hz" "Bell Hz" 0 :log))
      '("EXPRESSION" ("expression.vib_cent" "Vibrato ct" 1 :linear) ("expression.growl" "Growl" 2 :linear)))
    (list
      (dict :title "Reed"
        :view (lambda () (eseq.effects.physical-model-surface/curved-reed-view
          (eseq.effects.physical-model-surface/bind "reed.stiffness")
          (eseq.effects.physical-model-surface/bind "reed.closure") 1
          (eseq.effects.physical-model-surface/bind "reed.curve")))
        :controls (lambda () '(("reed.closure" "Closure" 3) ("reed.curve" "Reed curve" 2)
          ("reed.air" "Air noise" 3) ("reed.vel_blow" "Velocity > breath" 2)))
        :hint "Pressure, stiffness and closure can soften or overblow the reed.")
      (dict :title "Bore"
        :view (lambda () (eseq.effects.physical-model-surface/bore-loss-view
          (eseq.effects.physical-model-surface/bind "bore.damping_hz")
          (eseq.effects.physical-model-surface/bind "bore.loss")))
        :controls (lambda () '(("bore.loss" "Reflection loss" 3) ("bore.tune" "Tune cents" 1)))
        :hint "Warp bends the partial spacing; 0 keeps the natural bore.")
      (dict :title "Body"
        :view (lambda () (eseq.effects.physical-model-surface/body-view))
        :controls (lambda () '(("color.body_hz" "Body Hz" 0) ("color.body_q" "Body Q" 2)))
        :hint "Body color shapes the hollow tone; Bell rolls off the top.")
      (dict :title "Motion"
        :view (lambda () (eseq.effects.physical-model-surface/vibrato-view))
        :controls (lambda () '(("expression.vib_hz" "Vibrato Hz" 2) ("expression.vib_wait" "Vibrato delay ms" 0)
          ("expression.vib_air" "Breath vibrato" 3) ("expression.growl_hz" "Growl Hz" 1)))
        :hint "Vibrato fades in after its delay; growl roughens the breath.")
      (dict :title "Amp"
        :view (lambda () (eseq.effects.physical-model-surface/envelope 4))
        :controls (lambda () '(("amp.attack" "Attack ms" 1) ("amp.decay" "Decay ms" 1) ("amp.sustain" "Sustain" 2)
          ("amp.release" "Release ms" 1) ("gain" "Output" 2)))
        :hint "The envelope drives breath; velocity also controls output."))))
