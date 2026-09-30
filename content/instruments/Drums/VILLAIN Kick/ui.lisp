;; VILLAIN Kick panel. Copied into the instrument by build_pca.py.
;; Kick 2's panel plus the three family axes: principal components of the 33
;; kicks' modal banks (pc1..pc3, in standard deviations of the family; 0 = the
;; kick as fitted). Every other knob is relative (1 = as fitted; Tilt 0 = as fitted).
(defsynth-ui
  (eseq.effects.physical-model-surface/panel "VILLAIN KICK"
    (list
      '("KICKS" ("kick_a" "Kick A" 0 :linear) ("kick_b" "Kick B" 0 :linear))
      '("MORPH" ("blend" "Blend" 2 :linear) ("exaggerate" "Exaggerate" 2 :linear))
      '("FAMILY" ("pc1" "Deep" 2 :linear) ("pc2" "Swell" 2 :linear))
      '("TENSION" ("pc3" "Tension" 2 :linear) ("bend" "Drop" 2 :linear)))
    (list
      (dict :title "Axes"
        :view (lambda () (v-stack :gap 0.3 :padding 0.3
          (label "Axes the 33 kicks vary along (0 = as fitted, units = std devs)." :height 0.6 :font-size 9.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Deep +: lower sub, slower and longer pitch drop." :height 0.6 :font-size 9.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Swell +: sub blooms later with a longer tail." :height 0.6 :font-size 9.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Tension +: less drop, higher sub and body modes." :height 0.6 :font-size 9.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("pc1" "Deep" 2) ("pc2" "Swell" 2) ("pc3" "Tension" 2)
                               ("decay" "Decay" 2) ("knock" "Knock" 2) ("tick" "Tick" 2)))
        :hint "Modal bank only: noise has no shared axis (use Air / Room / Click).")
      (dict :title "Shape"
        :view (lambda () (v-stack :gap 0.15 :padding 0.25
          (label "Kicks: 0=51 1=52 2=54 3=55 4=56 5=57 6=58 7=59 8=60" :height 0.55 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "9=61 10=62 11=63 12=64 13=65 14=66 15=67 16=68 17=69 18=70" :height 0.55 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "19=71 20=72 21=73 22=74 23=75 24=76 25=77 26=78 27=79" :height 0.55 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "28=81 29=85 30=86 31=98 32=99   Tilt: - = highs die first" :height 0.55 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("drop_time" "Drop time" 2) ("tilt" "Tilt" 2) ("spread" "Spread" 2)
                               ("beat" "Beat" 2) ("body" "Body" 2) ("attack" "Attack" 2)))
        :hint "Spread stretches the modes about the sub; Beat detunes its partner.")
      (dict :title "Colour"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Shape: each kick's own waveshaper. Growl: kick 63's buzz." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Grit: body-driven noise. Ceiling below 1 hits the clipper harder." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("shape" "Shape" 2) ("growl" "Growl" 2) ("grit" "Grit" 2)
                               ("drive" "Drive" 2) ("ceiling" "Ceiling" 2) ("clip_amt" "Clip amount" 2)))
        :hint "These blend between the two kicks' settings too.")
      (dict :title "Air"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Air: the band-limited burst at the hit." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Click: the bright edge. Tone shifts both filter corners." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("air" "Air" 2) ("air_time" "Air time" 2) ("air_tone" "Air tone" 2)
                               ("click" "Click" 2) ("click_time" "Click time" 2) ("click_tone" "Click tone" 2)))
        :hint "Noise is synthesized fresh on every hit; nothing is sampled.")
      (dict :title "Room"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Room: the dark tail. Floor: the kick's extra noise layer." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Crackle / Hum: vinyl ticks and mains hum where a kick has them." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("room" "Room" 2) ("room_time" "Room time" 2) ("room_tone" "Room tone" 2)
                               ("floor" "Floor" 2) ("crackle" "Crackle" 2) ("hum" "Hum" 2)))
        :hint "A knob does nothing when neither kick has that layer.")
      (dict :title "Out"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Tune: pitch of everything. Hold / Release: the gate." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Length: where the hit fades out." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("tune" "Tune" 2) ("hold" "Hold" 2) ("release" "Release" 2)
                               ("length" "Length" 2) ("level" "Level" 2)))
        :hint "A3 is the fitted register."))))
