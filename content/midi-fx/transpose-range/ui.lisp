;; Transpose range panel: low/high bounds in semitones around C4, with a
;; live readout of the window as note names. Notes outside the window are
;; folded back inside by octaves (see dsp.lisp).
(def tr-note-names
  (list "C" "C#" "D" "D#" "E" "F" "F#" "G" "G#" "A" "A#" "B"))

(def tr-note-name (semis)
  (let ((n (round semis)))
    (str (nth tr-note-names (mod (+ (mod n 12) 12) 12))
         (+ 4 (floor (/ n 12))))))

(def tr-live (name)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (if p (eseq.effects.custom-ui-runtime/custom-ui-param-value p) 0)))

(def tr-range-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium "RANGE" (eseq.effects.custom-ui-lego/ui-accent-blue)
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "min" "low" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "max" "high" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 0))))

(def tr-readout-block ()
  (let ((lo (tr-live "min"))
        (hi (tr-live "max")))
    (eseq.effects.custom-ui-lego/ui-readout-block-small "WINDOW" (eseq.effects.custom-ui-lego/ui-accent-blue)
      (subtree :key (str "tr-window-readout-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" lo "-" hi)
        (h-stack :gap 0.4 :align :center
          (label "folds into" :font-size 8.6 :color :dim :bg :transparent)
          (label (str (tr-note-name (if (< lo hi) lo hi)) " - " (tr-note-name (if (< lo hi) hi lo)))
            :font-size 10.5 :color :blue :bg :transparent)
          (label "(C4 = 0)" :font-size 8.2 :color :dim :bg :transparent))))))

(def-midi-fx-ui
  (h-stack :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (tr-range-block)
      (tr-readout-block))))
