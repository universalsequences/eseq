;; Lane patchbay capture: two expanded tracks, each with the default layer
;; plus track-layer lanes wired rand -> cmp -> roll through `:connect`. Two
;; patchbays share one layout, so this doubles as the cross-track check:
;; every cable must stay inside its own track's boxes. Edit natives enqueue
;; history commands the capture harness does not apply, so wires are authored.
(capture-project
  (track :instrument "factory:Synths/Digi Drift" :name "Digi Drift" :steps (0 4 8 12))
  (track :sampler :name "Sampler" :steps (0 8)))

(def lane-patchbay-attach-0
  (processes :track 0
    (lane-rand :connect '((wire (process-inlet :lane-cmp :a))))
    (lane-cmp :connect '((wire (process-inlet :lane-roll :gate))))
    (lane-roll)))

(def lane-patchbay-attach-1
  (processes :track 1
    (lane-count :connect '((wire (process-inlet :lane-veto :gate))))
    (lane-veto)))

(def capture-after-sync ()
  (do
    (eseq.sequencer/set-track-expanded (nth SEQ.track-ids 0) true)
    (eseq.sequencer/set-track-param-mode (nth SEQ.track-ids 0)
      (+ eseq.seqv-track-params/seqv-process-lane-mode-offset 8))
    (eseq.sequencer/set-track-expanded (nth SEQ.track-ids 1) true)
    (eseq.sequencer/set-track-param-mode (nth SEQ.track-ids 1)
      (+ eseq.seqv-track-params/seqv-process-lane-mode-offset 2))
    (eseq.sequencer/lane-patch-show true)))
