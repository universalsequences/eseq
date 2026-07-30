;; Deterministic capture of the neural sequencer's aggregate piano keyboard.
(capture-project
  (track :sampler :name "Piano 1")
  (track :sampler :name "Piano 2")
  (track :sampler :name "Piano 3")
  (track :sampler :name "Piano 4")
  (track :sampler :name "Piano 5")
  (track :sampler :name "Piano 6")
  (track :sampler :name "Piano 7")
  (track :sampler :name "Piano 8"))

(load "@/scripts/sequencers/graph-neural-8x8-demo.lisp")

(def capture-track-active-notes
  (list
    (list (dict :note 36 :velocity 0.20 :trigger-id 1) (dict :note 60 :velocity 0.35 :trigger-id 2))
    (list (dict :note 40 :velocity 0.30 :trigger-id 3) (dict :note 64 :velocity 0.45 :trigger-id 4))
    (list (dict :note 43 :velocity 0.40 :trigger-id 5) (dict :note 67 :velocity 0.55 :trigger-id 6))
    (list (dict :note 48 :velocity 0.50 :trigger-id 7) (dict :note 72 :velocity 0.65 :trigger-id 8))
    (list (dict :note 52 :velocity 0.60 :trigger-id 9) (dict :note 76 :velocity 0.75 :trigger-id 10))
    (list (dict :note 55 :velocity 0.70 :trigger-id 11) (dict :note 79 :velocity 0.85 :trigger-id 12))
    (list (dict :note 60 :velocity 0.90 :trigger-id 13) (dict :note 84 :velocity 0.95 :trigger-id 14))
    (list (dict :note 60 :velocity 0.90 :trigger-id 15) (dict :note 67 :velocity 0.80 :trigger-id 16) (dict :note 91 :velocity 1.0 :trigger-id 17))))

;; SEQ is host-owned and read-only to ordinary Lisp. Publish a capture-only
;; view with deterministic activity after the production script has installed
;; its live effect.
(effect-buffer "*8x8*"
  (g8-panel SEQ.current-pattern
    SEQ.graph-visualizations
    SEQ.track-events
    SEQ.track-event-current-beat
    SEQ.track-colors
    capture-track-active-notes))
