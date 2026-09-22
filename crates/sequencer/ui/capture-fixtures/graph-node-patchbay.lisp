(capture-project
  (track :sampler :name "A")
  (track :sampler :name "B"))
(import alez.neural.variable-reset)
(def capture-after-sync ()
  (let ((g alez.neural.variable-reset/gvr-name))
    (let ((rand-id (graph-node-process-add g 1 "lane-rand"))
          (cmp-id (graph-node-process-add g 1 "lane-cmp"))
          (mask-id (graph-node-process-add g 1 "prob-mask")))
      (do
        (graph-node-process-wire g 1 rand-id "wire" cmp-id "a")
        (graph-node-process-fanout-add g 1 rand-id "wire" mask-id "prob")
        (alez.neural.variable-reset/gvr-expand-node 1)))))
