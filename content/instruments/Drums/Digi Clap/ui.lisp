;; Compact performance surface. DSP voicing and presets remain intact.
;; Graphics show named analytic mechanisms, not rendered audio.
(defsynth-ui
  (eseq.effects.drum-surface/panel "DIGI CLAP"
    (list
      (list "Play" (list "ptch" "Pitch" 1 nil) (list "dec" "Decay ms" 0 nil))
      (list "Burst" (list "snap" "Snap" 2 nil) (list "body" "Body" 2 nil))
      (list "Filter" (list "tone" "Tone" 0 nil) (list "reso" "Reso" 2 nil))
      (list "Color" (list "dist" "Distortion" 2 nil) (list "level" "Level" 2 nil)))
    (list
      (dict :title "Burst" :controls (lambda () (list (list "engine" "Engine" 0 '("808" "909" "LINN")) (list "bursts" "Bursts" 0 nil) (list "sprd" "Spread ms" 1 nil) (list "bdec" "Burst decay" 1 nil) (list "humanize" "Humanize" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/burst-view)))
      (dict :title "Voice" :controls (lambda () (if (= (eseq.effects.drum-surface/value "engine") 1) (list (list "clip" "Clip" 2 nil)) (if (= (eseq.effects.drum-surface/value "engine") 2) (list (list "clip" "Clip" 2 nil) (list "tlpf" "Tail LPF" 0 nil)) (list (list "clip" "Clip" 2 nil) (list "air" "Air" 2 nil) (list "crowd" "Crowd" 2 nil) (list "bits" "Bits" 0 nil)))))
        :visual (lambda () (eseq.effects.drum-surface/burst-view)))
      (dict :title "Filter" :controls (lambda () (list (list "fltf" "Filter Hz" 0 nil) (list "fltw" "Width Hz" 0 nil) (list "fltq" "Filter Q" 2 nil) (list "eqf" "EQ Hz" 0 nil) (list "eqg" "EQ dB" 1 nil)))
        :visual (lambda () (eseq.effects.drum-surface/burst-view)))
      (dict :title "Color" :controls (lambda () (list (list "srr" "Crush" 2 nil) (list "dist" "Distortion" 2 nil) (list "amd" "AM depth" 2 nil) (list "amf" "AM Hz" 0 nil) (list "level" "Level" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/burst-view))))))
