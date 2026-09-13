(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM ELECTRIC BASS"
    (list
      '("PLUCK" ("pluck.position" "Position" 2 :linear) ("pluck.softness" "Softness" 2 :linear))
      '("STRING" ("string.mute" "Mute" 2 :linear) ("string.damping" "Damping" 2 :linear))
      '("PICKUP" ("pickup.position" "Position" 2 :linear) ("pickup.aperture" "Width" 3 :linear))
      '("OUTPUT" ("output.tone_hz" "Tone Hz" 0 :log) ("texture.slow" "Slow %" 1 :linear)))
    (list
      (dict :title "Pluck"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "FINGER / PICK" :height 0.7 :font-size 12 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Bridge  0 -------- pluck -------- 0.5  Center" :height 0.7 :font-size 11 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "A wider contact softens the released string shape." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("pluck.velocity_tone" "Velocity > brightness" 2) ("pluck.attack_ms" "Fundamental arrives ms" 0)))
        :hint "Applied on the next note. Attack delays only the fundamental.")
      (dict :title "String"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "64 STRING MODES" :height 0.7 :font-size 12 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Mute shortens the note; damping removes upper ringing." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Friction damps each partial by ln(n): the octave dies faster than the root." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("string.decay_s" "Open decay s" 2) ("string.stiffness" "Stiffness" 2) ("string.friction" "Friction" 1)
          ("string.release_s" "Note-off decay s" 3) ("string.tune" "Tune cents" 1)))
        :hint "Decay is a nominal 60 dB fall; damping and mute can shorten it.")
      (dict :title "Pickup"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "MAGNETIC PICKUP" :height 0.7 :font-size 12 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Bridge  0 -------- pickup -------- 0.5  Center" :height 0.7 :font-size 11 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Position selects harmonics; width averages string motion." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("pickup.position" "Pickup position" 3) ("pickup.aperture" "Pickup width" 3)
          ("vinyl.hiss" "Vinyl hiss" 2) ("vinyl.rumble" "Vinyl rumble" 2)))
        :hint "Measured from the bridge. Vinyl adds the record floor while a note plays.")
      (dict :title "Output"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "SAMPLER TEMPO STRETCH" :height 0.7 :font-size 12 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "String -> slice-repeat stretch -> tone" :height 0.7 :font-size 11 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Slow % is how far the sampler tempo drops; pitch is untouched." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("output.resonance" "Tone resonance" 2) ("output.steep" "Tone steep" 2) ("output.gain" "Output" 2)
          ("texture.grain_ms" "Slice ms" 0) ("texture.xfade_ms" "Slice crossfade ms" 1)))
        :hint "Slow 0 is clean. Wobble rate is one per slice; keep the crossfade short."))))
