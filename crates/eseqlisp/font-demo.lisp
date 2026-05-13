;; font-demo.lisp -- proportional font system showcase

(defstate volume 75)
(defstate pan 50)
(defstate cutoff 8000)
(defstate resonance 20)
(defstate tab 0)
(defstate active true)
(defstate attack 15)
(defstate decay 40)
(defstate sustain 65)
(defstate release 30)

(effect
  (v-stack :padding 1 :gap 1.5

    ;; Header — large font inherited by children
    (h-stack :font-size 20 :gap 2 :align :center
      (label "Synthesizer" :color :white)
      (toggle :bind active)
      (label "bypass" :color :dim :font-size 12))

    ;; Tabs
    (tabs :items '("mixer" "filter" "envelope")
          :bind tab :padding 0.5

      ;; Tab 0: Mixer
      (v-stack :padding 1 :gap 1 :font-size 13
        (h-stack :gap 2 :align :center
          (label "Volume" :color :green)
          (hslider :min 0 :max 100 :bind volume))
        (h-stack :gap 2 :align :center
          (label "Pan" :color :yellow)
          (hslider :min 0 :max 100 :bind pan))
        (label "Adjust levels and stereo position" :color :dim :font-size 10))

      ;; Tab 1: Filter
      (v-stack :padding 1 :gap 1
        (h-stack :gap 2 :align :center
          (label "Cutoff" :color :cyan)
          (hslider :min 20 :max 20000 :bind cutoff))
        (h-stack :gap 2 :align :center
          (label "Resonance" :color :cyan)
          (hslider :min 0 :max 100 :bind resonance)))

      ;; Tab 2: Envelope
      (v-stack :padding 1 :gap 0.5
        (label "ADSR Envelope" :color :white :font-size 16)
        (h-stack :gap 1 :font-size 12
          (v-stack :align :center :gap 0.5
            (vslider :min 0 :max 100 :bind attack)
            (label "Attack" :color :cyan))
          (v-stack :align :center :gap 0.5
            (vslider :min 0 :max 100 :bind decay)
            (label "Decay" :color :green))
          (v-stack :align :center :gap 0.5
            (vslider :min 0 :max 100 :bind sustain)
            (label "Sustain" :color :yellow))
          (v-stack :align :center :gap 0.5
            (vslider :min 0 :max 100 :bind release)
            (label "Release" :color :red)))))

    ;; Footer — small font
    (h-stack :gap 3 :font-size 10
      (label "CPU 2.1%" :color :dim)
      (label "Voices 4/16" :color :dim)
      (label "48kHz" :color :dim))))
