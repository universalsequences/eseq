;; VILLAIN Snare panel. Copied into the instrument by build_pca.py.
;; Master Snare 2's panel plus the three family axes: principal components
;; of the 32 snares (pc1, pc2: head modes; pc3: chain), in standard deviations of
;; the family; 0 = as fitted. Every other knob is relative (1 = as fitted).
;; CRISP/NOTE block (the 3rd axis + Release) / Note page: Release (ms; 2000 = Hold),
;; the fade when the note ends. Tune lives on the Note page.
(defsynth-ui
  (eseq.effects.physical-model-surface/panel "VILLAIN SNARE"
    (list
      '("SNARES" ("snare_a" "Snare A" 0 :linear) ("snare_b" "Snare B" 0 :linear))
      '("MORPH" ("blend" "Blend" 2 :linear) ("exaggerate" "Exaggerate" 2 :linear))
      '("FAMILY" ("pc1" "Sustain" 2 :linear) ("pc2" "Tension" 2 :linear))
      '("CRISP/NOTE" ("pc3" "Crisp" 2 :linear) ("release" "Release" 0 :log)))
    (list
      (dict :title "Axes"
        :view (lambda () (v-stack :gap 0.3 :padding 0.3
          (label "Axes the 32 snares vary along (0 = as fitted, units = std devs)." :height 0.6 :font-size 9.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Sustain +: the head rings longer, with more body." :height 0.6 :font-size 9.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Tension +: less pitch drop, higher head, a bit longer." :height 0.6 :font-size 9.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Crisp +: cleaner sampler, more crack and top end." :height 0.6 :font-size 9.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("pc1" "Sustain" 2) ("pc2" "Tension" 2) ("pc3" "Crisp" 2)
                               ("decay" "Decay" 2) ("buzz" "Buzz" 2) ("rattle" "Rattle" 2)))
        :hint "Axes add to Blend; with Exaggerate they never go past its own reach.")
      (dict :title "Snares"
        :view (lambda () (v-stack :gap 0.1 :padding 0.25
          (label "0=SP01 1=SP02 2=SP03 3=SP04 4=SP05 5=SP06 6=SP07 7=SP08" :height 0.5 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "8=SP09 9=SP10 10=SP11 11=SP12 12=SP13 13=SP14 14=SP15 15=SP16" :height 0.5 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "16=SP17 17=SP18 18=SP19 19=SP20 20=XCF 21=XDXDFGHS" :height 0.5 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "22=YETPRE 23=YNG 24=YUTE 25=YYUUU 26=ZDSZS 27=ZETWY" :height 0.5 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "28=ZFG 29=ZSFG 30=ZV 31=ZXDVFB" :height 0.5 :font-size 8.5 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("body" "Body" 2) ("ring" "Ring" 2) ("bend" "Pitch motion" 2)
                               ("crack" "Crack" 2) ("strokes" "Strokes" 2) ("level" "Level" 2)))
        :hint "Strokes: the flams and drags of the snares that have them.")
      (dict :title "Air"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Room: the body of air after the hit. Extra: each snare's own" :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "noise beds, tails and grace strokes. A knob does nothing when" :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "neither snare has that part." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("room" "Room" 2) ("extra" "Extra" 2) ("hiss" "Hiss" 2) ("dust" "Dust" 2)))
        :hint "Noise is synthesized fresh on every hit; nothing is sampled.")
      (dict :title "Note"
        :view (lambda () (v-stack :gap 0.4 :padding 0.3
          (label "Release: fade after the note ends (step dur). 2000 = Hold." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Hold ignores note length and rack choke. Never lengthens." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))
          (label "Drive: tape push. Grit: SP sampler. Length: hit fade-out." :height 0.7 :font-size 10 :bg :transparent :v-align :center :color (rgba 0.16 0.06 0.11 1))))
        :controls (lambda () '(("release" "Release ms" 0) ("tune" "Tune" 2) ("length" "Length" 2)
                               ("drive" "Drive" 2) ("grit" "Grit" 2) ("pc3" "Crisp" 2)))
        :hint "A3 is the fitted register. A rack choke fades in Release ms."))))
