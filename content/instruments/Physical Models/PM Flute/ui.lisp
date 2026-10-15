;; The original jet and body, with the breath oscillators exposed as Flutter.
(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM FLUTE"
    (list
      '("AIR JET" ("breath" "Breath" 2 :linear) ("embouchure" "Embouchure" 2 :linear))
      '("FEEDBACK" ("wvfb" "Feedback" 2 :linear) ("detunex" "Tune" 2 :linear))
      '("RESONANT BODY" ("resbodymix" "Body mix" 2 :linear) ("freq" "Body freq" 0 :log))
      '("FLUTTER" ("flutter.depth" "Depth" 2 :linear) ("flutter.rate" "Rate" 2 :log)))
    (list
      (dict :title "Jet"
        :view (lambda () (eseq.effects.physical-model-surface/column-view
          (eseq.effects.physical-model-surface/bind "embouchure") "Jet delay"))
        :controls (lambda () '(("breathnoise" "Air noise" 3))))
      (dict :title "Loop"
        :view (lambda () (eseq.effects.physical-model-surface/envelope 1))
        :controls (lambda () '(("amp.attack" "Attack" 1) ("amp.decay" "Decay" 1)
          ("amp.sustain" "Sustain" 2) ("amp.release" "Release" 1) ("gain" "Output" 2))))
      (dict :title "Body"
        :view (lambda () (eseq.effects.physical-model-surface/partials-view))
        :controls (lambda () '(("bright" "Brightness" 2) ("stretch" "Partial stretch" 3))))
      (dict :title "Flutter"
        :view (lambda () (eseq.effects.physical-model-surface/flutter-view))
        :controls (lambda () '(("flutter.drift_hz" "Depth drift" 2) ("flutter.floor" "Minimum depth" 3)))))))
