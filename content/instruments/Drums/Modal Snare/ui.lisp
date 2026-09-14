;; The same compact performance surface as Membrane Snare, with the modal
;; voice's own strike, decay-law, mode-split and wire-contact controls.
;; Curves show nominal isolated T60s, not the mixed audio waveform.
(def modal-snare-head-envelopes ()
  (eseq.effects.drum-surface/envelope
    "Nominal batter T60 / 0–1 s" (eseq.effects.drum-surface/bind "release")
    "Nominal resonant head T60 / 0–1 s" (eseq.effects.drum-surface/bind "release2")
    "release" "release2"))

(defsynth-ui
  (eseq.effects.drum-surface/panel "MODAL SNARE"
    (list
      (list "Heads" (list "tune" "Tune" 1 nil) (list "release" "Release" 0 nil))
      (list "Strike" (list "stroke" "Stroke" 2 nil) (list "press" "Press" 2 nil))
      (list "Wires" (list "snares" "Snares" 2 nil) (list "snare_tension" "Tension" 2 nil))
      (list "Color" (list "tone" "Tone" 2 nil) (list "level" "Level" 2 nil)))
    (list
      (dict :title "Heads"
        :controls (lambda () (list
          (list "release2" "Reso decay" 0 nil)
          (list "pitch2_ratio" "Head ratio" 2 nil)
          (list "stretch" "Spread" 2 nil)
          (list "split" "Mode split" 2 nil)
          (list "tilt" "Decay tilt" 2 nil)
          (list "visc" "Viscosity" 2 nil)
          (list "head_couple" "Head coupling" 2 nil)
          (list "bottom_mix" "Bottom mic" 2 nil)))
        :visual (lambda () (modal-snare-head-envelopes)))
      (dict :title "Strike"
        :controls (lambda () (list
          (list "tip" "Tip size" 2 nil)
          (list "stick_hard" "Hardness" 5 nil)
          (list "stick_speed" "Speed" 3 nil)
          (list "scrape" "Scrape" 2 nil)
          (list "bend" "Bend" 2 nil)
          (list "palm" "Palm damping" 4 nil)))
        :visual (lambda () (modal-snare-head-envelopes)))
      (dict :title "Wires"
        :controls (lambda () (list
          (list "wire_pitch" "Wire pitch" 0 nil)
          (list "wire_decay" "Wire decay" 0 nil)
          (list "rattle" "Rattle" 2 nil)
          (list "wire_kick" "Bounce" 2 nil)
          (list "contact_loss" "Contact loss" 4 nil)
          (list "wire_tone" "Wire tone" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope
          "Nominal wire T60 / 0–1 s" (eseq.effects.drum-surface/bind "wire_decay")
          "Nominal resonant head T60 / 0–1 s" (eseq.effects.drum-surface/bind "release2")
          "wire_decay" "release2")))
      (dict :title "Color"
        :controls (lambda () (list
          (list "bright" "Bright" 2 nil)
          (list "punch" "Punch" 2 nil)
          (list "drive" "Drive" 2 nil)))
        :visual (lambda () (modal-snare-head-envelopes)))
      (dict :title "Rim"
        :controls (lambda () (list
          (list "rim_pitch" "Rim pitch" 0 nil)
          (list "rim_track" "Rim tracking" 2 nil)
          (list "rim_decay" "Rim decay" 0 nil)
          (list "rim_level" "Rim level" 2 nil)
          (list "rim_drive" "Rim drive" 5 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope
          "Nominal rim T60 / 0–1 s" (eseq.effects.drum-surface/bind "rim_decay")
          "Nominal batter T60 / 0–1 s" (eseq.effects.drum-surface/bind "release")
          "rim_decay" "release"))))))
