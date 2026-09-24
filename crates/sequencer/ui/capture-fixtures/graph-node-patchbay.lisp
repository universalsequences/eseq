;; A `neural` instance's node patch bay (alez/neural, instance-kinds spec §7).
;; Capture with --buffer "*neural · neural 1*".
(capture-project
  (track :sampler :name "A")
  (track :sampler :name "B"))
(import alez.neural.variable-reset)
;; Host-created instance (the Packages tab's "New neural"); it is instance 1
;; of this fresh project and its view renders into *neural · neural 1*.
(host-command "instance-create" (dict :kind "alez/neural:neural"))
(def capture-after-sync ()
  (let ((g (instance-ref 1)))
    (let ((rand-id (graph-node-process-add g 1 "lane-rand"))
          (cmp-id (graph-node-process-add g 1 "lane-cmp"))
          (mask-id (graph-node-process-add g 1 "prob-mask")))
      (do
        (graph-node-process-wire g 1 rand-id "wire" cmp-id "a")
        (graph-node-process-fanout-add g 1 rand-id "wire" mask-id "prob")
        (alez.neural.variable-reset/gvr-expand-node g 1)))))
