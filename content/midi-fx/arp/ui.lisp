;; Arpeggiator panel: clock + pattern on the left, gate/velocity feel on the
;; right. Built from the shared lego kit so p-locks, modulation rings and
;; section highlighting behave like every audio effect panel.
(def arp-rate-labels
  (list "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T"))

(def arp-direction-labels
  (list "up" "down" "up-down" "random"))

(def arp-pattern-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium "PATTERN" (eseq.effects.custom-ui-lego/ui-accent-blue)
    (h-stack :gap 0.5 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-option-s 0 "rate" "rate" 5.4 arp-rate-labels (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-option-s 0 "direction" "direction" 6.4 arp-direction-labels (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "octave" "octaves" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 0))))

(def arp-feel-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium "FEEL" (eseq.effects.custom-ui-lego/ui-accent-blue)
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "gate" "gate" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 1 "velocity" "vel" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))

(def-midi-fx-ui
  (h-stack :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (arp-pattern-block)
      (arp-feel-block))))
