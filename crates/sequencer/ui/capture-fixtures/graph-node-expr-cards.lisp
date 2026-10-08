;; Expr cards in a `neural` node bay (docs/expr-process-spec.md §2, §3.2):
;; a clean card fed by a cable, a card whose last commit failed (error dot),
;; and a card with more inlets than fit (+n badge). The clean card is
;; selected, so the inspector shows its unbounded inlet pickers.
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
          (clean-id (graph-node-process-add g 1 "expr"))
          (broken-id (graph-node-process-add g 1 "expr"))
          (wide-id (graph-node-process-add g 1 "expr")))
      (do
        (graph-node-process-expr-set g 1 clean-id "(sin (* x rate))")
        (graph-node-process-expr-set g 1 broken-id "(* in k)")
        (eseq.expr-buffer/commit-source g 1 broken-id "(sinn in)")
        (graph-node-process-expr-set g 1 wide-id "(+ a (* b c) d e)")
        (node-cable g 1 "bind" rand-id "wire" clean-id "x")
        (graph-node-process-inlet g 1 clean-id "rate" 0.25)
        (alez.neural.variable-reset/gvr-expand-node g 1)
        (eseq.sequencer/lane-patch-node-select clean-id)))))
