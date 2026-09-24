;; Map arming in a `neural` instance's expanded node editor.
;; Capture with --buffer "*neural · neural 1*".
(capture-project
  (track :sampler :name "A")
  (track :sampler :name "B"))
(import alez.neural.variable-reset)
(host-command "instance-create" (dict :kind "alez/neural:neural"))
(def capture-after-sync ()
  (let ((g (instance-ref 1)))
    (let ((rand-id (graph-node-process-add g 1 "lane-rand"))
          (xp-id (graph-node-process-add g 1 "neural-transpose")))
      (do
        (graph-node-process-wire g 1 rand-id "wire" xp-id "amount")
        (alez.neural.variable-reset/gvr-expand-node g 1)
        (alez.neural.variable-reset/gvr-map-arm g rand-id "out")))))
