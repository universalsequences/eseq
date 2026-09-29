;; Expr presets in a `neural` node bay (docs/expr-process-spec.md §6.1,
;; bead eseq-waa9.15): two preset cards (lfsr, bounce) added the way the
;; add menu adds them, the lfsr card selected (inspector shows taps/grain
;; at their starting values), and the add menu clicked open so the
;; "expr presets" heading and its rows show.
;; Capture with --buffer "*neural · neural 1*" --width 2400 --height 1600.
(capture-project
  (track :sampler :name "A")
  (track :sampler :name "B"))
(import alez.neural.variable-reset)
(host-command "instance-create" (dict :kind "alez/neural:neural"))
(def capture-after-sync ()
  (let ((g (instance-ref 1)))
    (let ((lfsr-id (eseq.expr-buffer/add-node-preset g 1 (eseq.expr-buffer/preset-named "lfsr")))
          (bounce-id (eseq.expr-buffer/add-node-preset g 1 (eseq.expr-buffer/preset-named "bounce"))))
      (do
        (alez.neural.variable-reset/gvr-expand-node g 1)
        (eseq.sequencer/lane-patch-node-select lfsr-id)))))
(def capture-click-widgets (list "menu-button"))
