;; R8 Kick 03 — musical surface for the acoustic reconstruction.
;; Identification coefficients live in the DSP, not a wall of lab readouts.
;; Each exposed parameter appears exactly once and uses the standard
;; reactive / p-lock / modulation-aware custom instrument control contract.

(def r8a-body-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def r8a-motion-c () (eseq.effects.custom-ui-lego/ui-accent-green))
(def r8a-contact-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def r8a-color-c () (eseq.effects.custom-ui-lego/ui-accent-violet))

(def r8a-knob (name title color decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s
    0 name title 6.8 3.25 3.25 color decimals))
(def r8a-row (a b c)
  (h-stack :gap 0.32 :align :start a b c))
(def r8a-note (text)
  (label text :width 21 :height 0.82 :font-size 9 :color :dim :bg :transparent))
(def r8a-panel (title color top bottom footer)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s 23 10.6 0 :instrument-group-bg
    (v-stack :width :fill :gap 0.22 :align :start
      (h-stack :width :fill :height 1.10 :align :center
        (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 title 12 color))
      top
      bottom
      footer)))

(def r8a-body ()
  (r8a-panel "01 / MEMBRANE" (r8a-body-c)
    (r8a-row
      (r8a-knob "tune" "TUNE" (r8a-body-c) 1)
      (r8a-knob "weight" "WEIGHT" (r8a-body-c) 2)
      (r8a-knob "head" "HEAD" (r8a-body-c) 2))
    (r8a-row
      (r8a-knob "decay" "DECAY" (r8a-body-c) 2)
      (r8a-knob "damp" "DAMP" (r8a-body-c) 2)
      (r8a-knob "stretch" "STRETCH" (r8a-body-c) 2))
    (h-stack :width :fill :gap 0.35 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.6 (r8a-body-c))
      (label "low weight / head tension" :width 15 :height 0.82 :font-size 9 :color :dim :bg :transparent))))

(def r8a-motion ()
  (r8a-panel "02 / IMPACT" (r8a-motion-c)
    (r8a-row
      (r8a-knob "bend" "BEND" (r8a-motion-c) 2)
      (r8a-knob "bend_time" "BEND TIME" (r8a-motion-c) 2)
      (r8a-knob "attack" "RISE" (r8a-motion-c) 2))
    (r8a-row
      (r8a-knob "length" "LENGTH" (r8a-motion-c) 0)
      (r8a-knob "punch" "PUNCH" (r8a-motion-c) 2)
      (r8a-knob "dynamics" "TOUCH" (r8a-motion-c) 2))
    (r8a-note "pitch drop / cut / velocity response")))

(def r8a-contact ()
  (r8a-panel "03 / CONTACT" (r8a-contact-c)
    (r8a-row
      (r8a-knob "knock" "KNOCK" (r8a-contact-c) 2)
      (r8a-knob "shell_tune" "SHELL TUNE" (r8a-contact-c) 1)
      (r8a-knob "ring" "RING" (r8a-contact-c) 2))
    (r8a-row
      (r8a-knob "beater" "BEATER" (r8a-contact-c) 2)
      (r8a-knob "hardness" "HARDNESS" (r8a-contact-c) 2)
      (r8a-knob "contact" "CONTACT" (r8a-contact-c) 2))
    (r8a-note "shell / felt-to-wood / contact time")))

(def r8a-color ()
  (r8a-panel "04 / PRINT" (r8a-color-c)
    (r8a-row
      (r8a-knob "air" "AIR" (r8a-color-c) 2)
      (r8a-knob "track" "KEYTRACK" (r8a-color-c) 2)
      (r8a-knob "drive" "DRIVE" (r8a-color-c) 2))
    (r8a-row
      (r8a-knob "tone" "TONE" (r8a-color-c) 2)
      (r8a-knob "crush" "CRUSH" (r8a-color-c) 2)
      (r8a-knob "level" "LEVEL" (r8a-color-c) 2))
    (r8a-note "texture / dark-to-bright / grit")))

(defsynth-ui
  (h-stack :gap 0.35 :align :start
    (r8a-body)
    (r8a-motion)
    (r8a-contact)
    (r8a-color)))
