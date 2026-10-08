;; Map arming in a `neural` instance's expanded node editor.
;; Capture with --buffer "*neural · neural 1*".
(capture-project
  (track :sampler :name "A")
  (track :sampler :name "B"))
(import alez.neural.variable-reset)
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
          (xp-id (graph-node-process-add g 1 "neural-transpose")))
      (do
        (node-cable g 1 "bind" rand-id "wire" xp-id "amount")
        (alez.neural.variable-reset/gvr-expand-node g 1)
        (alez.neural.variable-reset/gvr-map-arm g rand-id "out")))))
