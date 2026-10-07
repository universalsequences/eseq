;; The *processes* dock (docs/expr-process-spec.md §7; beads eseq-waa9.16,
;; eseq-waa9.22): a neural instance's tab in the main tile with node 1's
;; editor open, so the right column is the dock: the selected expr card's
;; inspector in place of *step*, its code buffer in place of *track*. The
;; browser sidebar is unchanged.
;; Capture the whole window with --buffer "*processes*" --all-panels
;; --width 2400 --height 1400.
(capture-project
  (track :sampler :name "A")
  (track :sampler :name "B"))
(import alez.neural.variable-reset)
(host-command "instance-create" (dict :kind "alez/neural:neural"))
(def capture-after-sync ()
  (let ((g (instance-ref 1)))
    (let ((rand-id (graph-node-process-add g 1 "lane-rand"))
          (expr-id (graph-node-process-add g 1 "expr")))
      (do
        (graph-node-process-expr-set g 1 expr-id "(-> x (* rate) sin (scale -1 1 0 4))")
        (graph-node-process-wire g 1 rand-id "wire" expr-id "x")
        ;; The tab switch swaps the ACTIVE tile's buffer: focus the main
        ;; tile first (capture starts elsewhere).
        (select-window-for "*sequencer*")
        (eseq.seq-step-tabs/seq-select-main-step-tab-by-buffer (eseq.seq-step-tabs/seq-instance-tab-buffer 1))
        (alez.neural.variable-reset/gvr-expand-node g 1)
        (eseq.sequencer/lane-patch-select-lane
          (eseq.sequencer/lane-patch-node-namespace g 1) expr-id)))))
