;; Patch-learning target picker in its production standalone buffer. The
;; two-buffer patcher/learn split is asserted separately by the layout test;
;; capture isolates this buffer so typography and spacing remain inspectable.
(capture-project
  (track :instrument "core/drift"))

(effect-buffer "*patch-learn*"
  (box :width :fill :height :fill
    (eseq.patch-learn/panel)))
