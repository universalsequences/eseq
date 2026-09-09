;; Compact performance surface. DSP voicing and presets remain intact.
;; Graphics show named analytic mechanisms, not rendered audio.
(defsynth-ui
  (eseq.effects.drum-surface/panel "R8 KICK 03"
    (list
      (list "Body" (list "tune" "Tune" 1 nil) (list "decay" "Decay" 2 nil))
      (list "Impact" (list "bend" "Bend" 2 nil) (list "punch" "Punch" 2 nil))
      (list "Contact" (list "beater" "Beater" 2 nil) (list "knock" "Knock" 2 nil))
      (list "Print" (list "tone" "Tone" 2 nil) (list "level" "Level" 2 nil)))
    (list
      (dict :title "Body" :controls (lambda () (list (list "tune" "Tune" 1 nil) (list "weight" "Weight" 2 nil) (list "head" "Head" 2 nil) (list "decay" "Decay" 2 nil) (list "damp" "Damp" 2 nil) (list "stretch" "Stretch" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/cut-view)))
      (dict :title "Impact" :controls (lambda () (list (list "bend" "Bend" 2 nil) (list "bend_time" "Bend time" 2 nil) (list "attack" "Attack" 1 nil) (list "length" "Length" 0 nil) (list "punch" "Punch" 2 nil) (list "dynamics" "Touch" 2 nil)))
        :visual (lambda () (let ((bend (pow (eseq.effects.drum-surface/value "bend") 2)))
          (eseq.effects.drum-surface/pitch-view 42.3414 (* 42.3414 1.8 bend) (* 42.3414 0.928374 bend)
            (/ -1 0.00236874) (/ -1 0.0387451) (eseq.effects.drum-surface/bind "bend_time") (eseq.effects.drum-surface/bind "tune") 0 "bend_time"))))
      (dict :title "Contact" :controls (lambda () (list (list "knock" "Knock" 2 nil) (list "shell_tune" "Shell tune" 1 nil) (list "ring" "Ring" 2 nil) (list "beater" "Beater" 2 nil) (list "hardness" "Hardness" 2 nil) (list "contact" "Contact" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/cut-view)))
      (dict :title "Print" :controls (lambda () (list (list "air" "Air" 2 nil) (list "track" "Track" 2 nil) (list "drive" "Drive" 2 nil) (list "tone" "Tone" 2 nil) (list "crush" "Crush" 2 nil) (list "level" "Level" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/cut-view))))))
