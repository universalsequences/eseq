;; Compact performance surface. DSP voicing and presets remain intact.
;; Graphics show named analytic mechanisms, not rendered audio.
(defsynth-ui
  (eseq.effects.drum-surface/panel "DIGI SNARE"
    (list
      (list "Play" (list "ptch" "Pitch" 1 nil) (list "dec" "Decay ms" 0 nil))
      (list "Machine" (list "snap" "Snap" 2 nil) (list "tone" "Tone" 2 nil))
      (list "Filter" (list "fltf" "Filter Hz" 0 nil) (list "fltw" "Width Hz" 0 nil))
      (list "Color" (list "dist" "Distortion" 2 nil) (list "level" "Level" 2 nil)))
    (list
      (dict :title "Play" :controls (lambda () (list (list "engine" "Engine" 0 '("TRX-SD" "EFM-SD" "EFM-RS")) (list "sustain" "Sustain" 2 nil) (list "release" "Release ms" 0 nil) (list "ptch" "Pitch" 1 nil) (list "dec" "Decay ms" 0 nil) (list "humanize" "Humanize" 2 nil) (list "level" "Level" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/dsr-envelope (let ((e (eseq.effects.drum-surface/value "engine")) (d (eseq.effects.drum-surface/value "dec"))) (if (= e 1) (max 15 (min 1000 (* d 0.42))) (if (= e 3) (max 8 (min 300 (* d 0.18))) d))))))
      (dict :title "Machine" :controls (lambda () (if (= (eseq.effects.drum-surface/value "engine") 1) (list (list "bump" "Bump" 1 nil) (list "benv" "Bend ms" 0 nil) (list "tune" "Tune" 2 nil) (list "clip" "Clip" 2 nil) (list "noise" "Noise" 2 nil) (list "ndec" "Noise decay" 0 nil) (list "hpf" "Hpf" 0 nil)) (if (= (eseq.effects.drum-surface/value "engine") 2) (list (list "mod_amt" "FM depth" 2 nil) (list "mfrq" "FM Hz" 0 nil) (list "mdec" "FM decay ms" 0 nil) (list "noise" "Noise" 2 nil) (list "ndec" "Noise decay" 0 nil) (list "hpf" "Hpf" 0 nil) (list "clip" "Clip" 2 nil)) (list (list "mod_amt" "FM depth" 2 nil) (list "mfrq" "FM Hz" 0 nil) (list "mdec" "FM decay ms" 0 nil) (list "noise" "Noise" 2 nil) (list "ndec" "Noise decay" 0 nil) (list "hpf" "Hpf" 0 nil) (list "clip" "Clip" 2 nil)))))
        :visual (lambda () (if (= (eseq.effects.drum-surface/value "engine") 1) (eseq.effects.drum-surface/envelope "Pitch bend T60 / 0–1 s" (eseq.effects.drum-surface/bind "benv") "Noise T60 / 0–1 s" (eseq.effects.drum-surface/bind "ndec") "benv" "ndec") (eseq.effects.drum-surface/envelope "Amplitude T60 / 0–1 s" (eseq.effects.drum-surface/bind "dec") "FM depth T60 / 0–1 s" (eseq.effects.drum-surface/bind "mdec") "dec" "mdec"))))
      (dict :title "Filter" :controls (lambda () (list (list "fltf" "Filter Hz" 0 nil) (list "fltw" "Width Hz" 0 nil) (list "fltq" "Filter Q" 2 nil) (list "eqf" "EQ Hz" 0 nil) (list "eqg" "EQ dB" 1 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Body T60 / 0–1 s" (eseq.effects.drum-surface/bind "dec") "Noise T60 / 0–1 s" (eseq.effects.drum-surface/bind "ndec") "dec" "ndec")))
      (dict :title "Color" :controls (lambda () (list (list "srr" "Crush" 2 nil) (list "dist" "Distortion" 2 nil) (list "amd" "AM depth" 2 nil) (list "amf" "AM Hz" 0 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Body T60 / 0–1 s" (eseq.effects.drum-surface/bind "dec") "Noise T60 / 0–1 s" (eseq.effects.drum-surface/bind "ndec") "dec" "ndec"))))))
