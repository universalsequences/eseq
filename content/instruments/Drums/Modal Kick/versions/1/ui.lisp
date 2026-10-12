;; Compact performance surface for the note-tracking modal drum.
;; Graphics show named analytic mechanisms, not rendered audio.
(defsynth-ui
  (eseq.effects.drum-surface/panel "MODAL KICK"
    (list
      (list "Heads" (list "tune" "Tune" 1 nil) (list "release" "Release" 0 nil))
      (list "Beater" (list "bend" "Bend" 2 nil) (list "bend_time" "Bend time" 0 nil))
      (list "Shell" (list "muffle" "Muffle" 2 nil) (list "port" "Port" 2 nil))
      (list "Color" (list "drive" "Drive" 2 nil) (list "level" "Level" 2 nil)))
    (list
      (dict :title "Heads" :controls (lambda () (list (list "release" "Release" 0 nil) (list "release2" "Reso decay" 0 nil) (list "pitch2_ratio" "Head ratio" 2 nil) (list "stretch" "Stretch" 2 nil) (list "tilt" "Tilt" 2 nil) (list "head_couple" "Head coupling" 2 nil) (list "bottom_mix" "Bottom mic" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Nominal batter T60 / 0–1 s" (eseq.effects.drum-surface/bind "release") "Nominal resonant head T60 / 0–1 s" (eseq.effects.drum-surface/bind "release2") "release" "release2")))
      (dict :title "Beater" :controls (lambda () (list (list "beater_hard" "Hardness" 5 nil) (list "beater_speed" "Speed" 3 nil) (list "beater_size" "Size" 2 nil) (list "bend" "Bend" 2 nil) (list "bend_time" "Bend time" 0 nil) (list "click" "Click" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Nominal batter T60 / 0–1 s" (eseq.effects.drum-surface/bind "release") "Nominal resonant head T60 / 0–1 s" (eseq.effects.drum-surface/bind "release2") "release" "release2")))
      (dict :title "Shell" :controls (lambda () (list (list "shell_pitch" "Shell interval" 1 nil) (list "shell_decay" "Shell decay" 0 nil) (list "shell_level" "Shell level" 2 nil) (list "muffle" "Muffle" 2 nil) (list "port" "Port" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Nominal shell T60 / 0–1 s" (eseq.effects.drum-surface/bind "shell_decay") "Nominal batter T60 / 0–1 s" (eseq.effects.drum-surface/bind "release") "shell_decay" "release")))
      (dict :title "Color" :controls (lambda () (list (list "drive_mode" "Drive type" 0 '("Off" "Sym1" "Sym2" "Sym3" "Asym1" "Asym2" "Asym3")) (list "bright" "Bright" 2 nil) (list "drive" "Drive" 2 nil) (list "tone" "Tone" 2 nil) (list "punch" "Punch" 2 nil) (list "level" "Level" 2 nil) (list "lpf" "Lpf" 0 nil) (list "hpf" "Hpf" 0 nil) (list "smoothing" "Smoothing" 1 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Nominal batter T60 / 0–1 s" (eseq.effects.drum-surface/bind "release") "Nominal resonant head T60 / 0–1 s" (eseq.effects.drum-surface/bind "release2") "release" "release2")))
      (dict :title "Bank" :controls (lambda () (list (list "bank" "Bank" 2 nil) (list "bank_env" "Env depth" 2 nil) (list "bank_freq" "Cutoff" 2 nil) (list "bank_res" "Resonance" 2 nil) (list "bank_time" "Time ms" 0 nil) (list "bank_harm" "Harmonic" 2 nil) (list "bank_crunch" "Crush" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Bank envelope T60 / 0–1 s" (eseq.effects.drum-surface/bind "bank_time") "Nominal batter T60 / 0–1 s" (eseq.effects.drum-surface/bind "release") "bank_time" "release")))
      (dict :title "Bank FX" :controls (lambda () (list (list "bank_drive" "Drive" 2 nil) (list "bank_recon" "Smooth" 2 nil) (list "bank_track" "Tracking" 2 '("free" "key"))))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Bank envelope T60 / 0–1 s" (eseq.effects.drum-surface/bind "bank_time") "Nominal batter T60 / 0–1 s" (eseq.effects.drum-surface/bind "release") "bank_time" "release"))))))
