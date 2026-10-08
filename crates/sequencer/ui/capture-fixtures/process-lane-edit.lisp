;; Expanded process lane geometry with authored values and a step selection.
(capture-project
  (track :sampler :name "Lane editing" :steps (0 4 8 12)))

(def-accumulator capture-lane
  :target (step-param :transpose)
  :amount (amount :lane true :default 0)
  :range (-24 24)
  :mode :clip)
(def capture-chain
  (processes :track 0
    (capture-lane :amount (lane 0 4 8 12 16 20 24 12 0 -4 -8 -12 -16 -20 -24 -12))))

(def capture-after-sync ()
  (let ((t (eseq.kinds/track 0)))
    (eseq.sequencer/set-track-expanded t true)
    (eseq.sequencer/set-track-param-mode t
      (+ eseq.seqv-track-params/seqv-process-lane-mode-offset
        (- (len t.lanes) 1)))
    (seq-select-step-range 1 5)))
