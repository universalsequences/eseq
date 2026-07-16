;; Deterministic *fx* capture fixture for the track-attached process panel.
(capture-project
  (track :sampler :name "Sampler"))

(load "@/scripts/processes/process-inlet-patch-demo.lisp")
(process-inlet-demo-attach-track 0)

(def capture-after-sync ()
  (process-panel-select-slot (nth SEQ.process-slots 0)))
