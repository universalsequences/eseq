(capture-project
  (track :sampler :name "A")
  (track :sampler :name "B"))
(import alez.neural.variable-reset)
(def capture-after-sync ()
  (let ((g alez.neural.variable-reset/gvr-name))
    (let ((rand-id (graph-node-process-add g 1 "lane-rand"))
          (xp-id (graph-node-process-add g 1 "neural-transpose")))
      (do
        (graph-node-process-wire g 1 rand-id "wire" xp-id "amount")
        (alez.neural.variable-reset/gvr-expand-node 1)
        (alez.neural.variable-reset/gvr-map-arm rand-id "out")))))
