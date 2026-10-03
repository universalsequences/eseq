(def gtr-caption (text)
  (label text :width 35.3 :height 0.42 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def pluck-view ()
  (v-stack :gap 0.02
      (gtr-caption "Every key plays the nearest recorded pitch's plucks:")
      (gtr-caption "C#2: 2 recorded plucks, soft to strong")
      (gtr-caption "D#2: 3 recorded plucks, soft to strong")
      (gtr-caption "G#2: 8 recorded plucks, soft to strong")
      (gtr-caption "F3: 12 recorded plucks, soft to strong")
      (gtr-caption "G3: 10 recorded plucks, soft to strong")
      (gtr-caption "C4: 4 recorded plucks, soft to strong")))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM NYLON GUITAR"
    (list
      '("PLUCK" ("pluck.take" "Take" 2 :linear) ("pluck.finger" "Finger" 2 :linear))
      '("STRING" ("string.decay" "Decay" 2 :log) ("string.mute" "Mute" 2 :linear))
      '("HAND" ("pluck.position" "Position" 2 :linear) ("pluck.humanize" "Humanize" 2 :linear))
      '("BODY & LEVEL" ("body.resonance" "Body" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Pluck"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("pluck.vel_take" "Vel > take" 2) ("pluck.vel_tone" "Vel > tone" 2) ("pluck.detail" "Detail" 2)))
        :hint "Each key borrows a recorded pluck of the nearest recorded pitch.")
      (dict :title "String"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("string.damping" "Damping" 2) ("string.release" "Release s" 2) ("string.stiffness" "Stiffness" 2)))
        :hint "Damping scales high-partial loss; Release is the fretting hand lifting.")
      (dict :title "Pitch"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("string.tune" "Tune ct" 0) ("string.vibrato" "Vibrato ct" 1) ("string.vib_rate" "Vib rate" 1)))
        :hint "Vibrato is the fretting finger rolling the string.")
      (dict :title "Fret"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("fret.buzz" "Buzz" 2) ("fret.level" "Buzz level" 2) ("fret.spread" "Spread" 2) ("body.wood" "Wood Hz" 0)))
        :hint "Buzz sets the fret gap; Buzz level only how loud it is heard.")
      (dict :title "Body"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("body.body" "Body EQ" 2) ("body.resonance" "Resonance" 2) ("body.contact" "Contact" 2)))
        :hint "Measured radiation EQ, air/top resonances and finger noise.")
      (dict :title "Output"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("output.tone_hz" "Tone Hz" 0) ("output.gain" "Output" 2)))
        :hint "The reference is mono; both channels carry the same signal."))))
