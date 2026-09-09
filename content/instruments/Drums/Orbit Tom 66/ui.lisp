;; Compact performance surface. DSP voicing and presets remain intact.
;; Graphics show named analytic mechanisms, not rendered audio.
(defsynth-ui
  (eseq.effects.drum-surface/panel "ORBIT TOM 66"
    (list
      (list "Pitch" (list "tune" "Tune" 1 nil) (list "ratio" "Ratio" 2 nil))
      (list "Shape" (list "attack" "Attack" 2 nil) (list "decay" "Decay" 2 nil))
      (list "Timbre" (list "harm" "Harm" 2 nil) (list "bright" "Bright" 2 nil))
      (list "Output" (list "drive" "Drive" 2 nil) (list "level" "Level" 2 nil)))
    (list
      (dict :title "Pitch" :controls (lambda () (list (list "tune" "Tune" 1 nil) (list "ratio" "Ratio" 2 nil) (list "sweep" "Sweep" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/pitch-view (eseq.effects.drum-surface/bind "c_end") (eseq.effects.drum-surface/bind "c_a1") (eseq.effects.drum-surface/bind "c_a2") (eseq.effects.drum-surface/bind "c_r1") (eseq.effects.drum-surface/bind "c_r2") (eseq.effects.drum-surface/bind "sweep") (eseq.effects.drum-surface/bind "tune") (eseq.effects.drum-surface/bind "c_hold") "sweep")))
      (dict :title "Shape" :controls (lambda () (list (list "attack" "Attack" 2 nil) (list "decay" "Decay" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/quadratic-envelope
          (* (eseq.effects.drum-surface/value "attack_time") (eseq.effects.drum-surface/value "attack"))
          (eseq.effects.drum-surface/bind "decay") (eseq.effects.drum-surface/bind "amp_decay")
          (eseq.effects.drum-surface/bind "amp_curve"))))
      (dict :title "Timbre" :controls (lambda () (list (list "harm" "Harm" 2 nil) (list "bright" "Bright" 2 nil) (list "noise" "Noise" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/pitch-view (eseq.effects.drum-surface/bind "c_end") (eseq.effects.drum-surface/bind "c_a1") (eseq.effects.drum-surface/bind "c_a2") (eseq.effects.drum-surface/bind "c_r1") (eseq.effects.drum-surface/bind "c_r2") (eseq.effects.drum-surface/bind "sweep") (eseq.effects.drum-surface/bind "tune") (eseq.effects.drum-surface/bind "c_hold") "sweep")))
      (dict :title "Output" :controls (lambda () (list (list "drive" "Drive" 2 nil) (list "level" "Level" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/pitch-view (eseq.effects.drum-surface/bind "c_end") (eseq.effects.drum-surface/bind "c_a1") (eseq.effects.drum-surface/bind "c_a2") (eseq.effects.drum-surface/bind "c_r1") (eseq.effects.drum-surface/bind "c_r2") (eseq.effects.drum-surface/bind "sweep") (eseq.effects.drum-surface/bind "tune") (eseq.effects.drum-surface/bind "c_hold") "sweep"))))))
