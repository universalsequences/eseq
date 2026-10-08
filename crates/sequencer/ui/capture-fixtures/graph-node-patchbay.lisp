;; A `neural` instance's node patch bay (alez/neural, instance-kinds spec §7).
;; Capture with --buffer "*neural · neural 1*".
(capture-project
  (track :sampler :name "A")
  (track :sampler :name "B"))
(import alez.neural.variable-reset)
;; Host-created instance (the Packages tab's "New neural"); it is instance 1
;; of this fresh project and its view renders into *neural · neural 1*.
(host-command "instance-create" (dict :kind "alez/neural:neural"))
;; A cable between two slots of node `node`, through the host kinds'
;; `edit-process` (what `bind-port!` / `add-fanout!` send; `op` "bind" or
;; "add-fanout"), by slot id: the kinds list a slot this setup adds only
;; after the next sync.
(def node-cable (g node op from port to inlet)
  (host-command "edit-process"
    (dict :graph-id g.id :node node :proc-id from :port port :op op :all false
          :target (dict :graph-id g.id :node node :proc-id to :kind "inlet" :inlet inlet))))
(def capture-after-sync ()
  (let ((g (instance-ref 1)))
    (let ((rand-id (graph-node-process-add g 1 "lane-rand"))
          (cmp-id (graph-node-process-add g 1 "lane-cmp"))
          (mask-id (graph-node-process-add g 1 "prob-mask")))
      (do
        (node-cable g 1 "bind" rand-id "wire" cmp-id "a")
        (node-cable g 1 "add-fanout" rand-id "wire" mask-id "prob")
        (alez.neural.variable-reset/gvr-expand-node g 1)))))
