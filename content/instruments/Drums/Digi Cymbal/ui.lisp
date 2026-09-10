;; Digi Hat's compact performance surface, with two cymbal machines.
;; Curves show the DSP's T60 envelopes, not rendered audio.
(def digi-cymbal-envelope ()
  (if (= (eseq.effects.drum-surface/value "engine") 1)
    (eseq.effects.drum-surface/envelope
      "Metal body T60 / 0–1 s" (eseq.effects.drum-surface/bind "dec")
      "Top band T60 / 0–1 s" (max 30 (min 1200 (* 0.42 (eseq.effects.drum-surface/value "dec"))))
      "dec" "dec")
    (eseq.effects.drum-surface/envelope
      "Amplitude T60 / 0–1 s" (eseq.effects.drum-surface/bind "dec")
      "FM depth T60 / 0–1 s" (eseq.effects.drum-surface/bind "mdec")
      "dec" "mdec")))

(defsynth-ui
  (eseq.effects.drum-surface/panel "DIGI CYMBAL"
    (list
      (list "Play"
        (if (= (eseq.effects.drum-surface/value "engine") 1)
          (list "size" "Size" 2 nil) (list "ptch" "Pitch" 1 nil))
        (list "dec" "Decay ms" 0 nil))
      (list "Machine"
        (if (= (eseq.effects.drum-surface/value "engine") 1)
          (list "humanize" "Humanize" 2 nil) (list "mod_amt" "FM depth" 2 nil))
        (list "level" "Level" 2 nil))
      (list "Filter" (list "fltf" "Filter Hz" 0 nil) (list "fltw" "Width Hz" 0 nil))
      (list "Color" (list "dist" "Distortion" 2 nil) (list "srr" "Crush" 2 nil)))
    (list
      (dict :title "Play"
        :controls (lambda () (list
          (list "engine" "Engine" 0 '("TRX-CY" "EFM-CY")) (list "sustain" "Sustain" 2 nil) (list "release" "Release ms" 0 nil)
          (list "dec" "Decay ms" 0 nil) (list "level" "Level" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/dsr-envelope (eseq.effects.drum-surface/bind "dec"))))
      (dict :title "Machine"
        :controls (lambda ()
          (if (= (eseq.effects.drum-surface/value "engine") 1)
            (list (list "rich" "Richness" 2 nil) (list "top" "Top" 2 nil)
              (list "ttun" "Top Hz" 0 nil) (list "peak" "Peak" 2 nil))
            (list (list "ptch" "Pitch" 1 nil) (list "mod_amt" "FM depth" 2 nil)
              (list "mfrq" "FM Hz" 0 nil) (list "mdec" "FM decay ms" 0 nil)
              (list "fb" "Feedback" 2 nil) (list "hpf" "HPF Hz" 0 nil))))
        :visual (lambda () (digi-cymbal-envelope)))
      (dict :title "Filter"
        :controls (lambda () (list
          (list "fltf" "Filter Hz" 0 nil) (list "fltw" "Width Hz" 0 nil)
          (list "fltq" "Filter Q" 2 nil) (list "eqf" "EQ Hz" 0 nil) (list "eqg" "EQ dB" 1 nil)))
        :visual (lambda () (digi-cymbal-envelope)))
      (dict :title "Color"
        :controls (lambda () (list
          (list "dist" "Distortion" 2 nil) (list "srr" "Crush" 2 nil)
          (list "amd" "AM depth" 2 nil) (list "amf" "AM Hz" 0 nil)))
        :visual (lambda () (digi-cymbal-envelope))))))
