;; Deterministic *track* capture with a selected step carrying track-level locks.
(capture-project
  (track :sampler :name "Timebase p-lock" :num-steps 16))

(seq-select-step 4)
(seq-plock-timebase "8")
(seq-set-track-param :swing 63)
(seq-set-swing-resolution "1/8")
(seq-set-step-param-plock :velocity 0.72)
(seq-set-step-param-plock :duration 1.5)
(seq-set-step-param-plock :transpose 7)
