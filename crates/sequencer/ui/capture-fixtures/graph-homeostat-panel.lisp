;; Deterministic production-path capture for the graph homeostat visualization.
(capture-project
  (track :sampler :name "Graph Homeostat"))

(load "@/scripts/sequencers/graph-neural-variable-reset-demo.lisp")

;; Show the destination-neuron affordance in its selected state. The focused
;; interaction regression separately verifies that matrix column 3 drives this
;; state; this fixture pins the resulting Metal composition.
(def capture-after-sync ()
  (set! gvr-selected-neuron 3))
