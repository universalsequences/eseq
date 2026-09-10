;; The original jet and body, with the breath oscillators exposed as Flutter.
(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM FLUTE"
    (list
      '("AIR JET" ("breath" "Breath" 2 :linear) ("embouchure" "Embouchure" 2 :linear))
      '("FEEDBACK" ("wvfb" "Feedback" 2 :linear) ("detunex" "Tune st" 2 :linear))
      '("RESONANT BODY" ("resbodymix" "Body mix" 2 :linear) ("freq" "Body Hz" 0 :log))
      '("FLUTTER" ("flutter.depth" "Depth" 2 :linear) ("flutter.rate" "Rate Hz" 2 :log)))
    (list
      (dict :title "Jet"
        :view (lambda () (eseq.effects.physical-model-surface/column-view
          (eseq.effects.physical-model-surface/bind "embouchure") "Jet delay / fraction of the bore period"))
        :controls (lambda () '(("breathnoise" "Air noise" 3)))
        :hint "Embouchure changes jet timing; noise roughens the breath.")
      (dict :title "Loop"
        :view (lambda () (eseq.effects.physical-model-surface/envelope 1))
        :controls (lambda () '(("amp.attack" "Attack ms" 1) ("amp.decay" "Decay ms" 1)
          ("amp.sustain" "Sustain" 2) ("amp.release" "Release ms" 1) ("gain" "Output" 2)))
        :hint "Feedback sustains the air column. Output 0.50 is unity.")
      (dict :title "Body"
        :view (lambda () (eseq.effects.physical-model-surface/partials-view))
        :controls (lambda () '(("bright" "Brightness" 2) ("stretch" "Partial stretch" 3)))
        :hint "Body mix blends the four resonances with the jet tone.")
      (dict :title "Flutter"
        :view (lambda () (eseq.effects.physical-model-surface/flutter-view))
        :controls (lambda () '(("flutter.drift_hz" "Depth drift Hz" 2) ("flutter.floor" "Minimum depth" 3)))
        :hint "Depth 0 turns flutter off; minimum depth is capped by Depth."))))
