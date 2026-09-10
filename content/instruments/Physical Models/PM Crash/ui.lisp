(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM CRASH"
    (list
      '("METAL" ("voicing.character" "Voicing" 2 :linear) ("body.size" "Size" 2 :log))
      '("RING" ("body.decay" "Decay" 2 :log) ("body.damping" "Upper loss" 2 :log))
      '("CONTACT" ("stick.hardness" "Hardness" 2 :linear) ("body.wash" "Wash" 2 :linear))
      '("BELL & LEVEL" ("body.bell" "Bell" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Metal"
        :view (lambda () (eseq.effects.physical-model-surface/cymbal-body-view))
        :controls (lambda () '(("tuning.tracking" "Key tracking" 2)))
        :hint "Voicing follows the reference collection; Size changes the body.")
      (dict :title "Ring"
        :view (lambda () (eseq.effects.physical-model-surface/cymbal-loss-view))
        :controls (lambda () '(("contact.touch" "Choke" 2)))
        :hint "Choke dissipates the ringing state; key-up leaves it ringing.")
      (dict :title "Contact"
        :view (lambda () (eseq.effects.physical-model-surface/cymbal-contact-view false))
        :controls (lambda () '( ("output.color" "Spectral color" 2)))
        :hint "Finite stick contact excites a freely ringing metal body.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/cymbal-output-view))
        :controls (lambda () '(("output.width" "Stereo spread" 2)))
        :hint "Bell and Wash balance resolved and dense body resonances."))))
