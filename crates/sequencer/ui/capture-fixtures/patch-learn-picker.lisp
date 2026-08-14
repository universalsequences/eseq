;; Patch-learning target picker, rendered beside the production patcher widget
;; at the same proportions used by an instrument edit session. Later phases
;; are driven by readonly host state and covered by layout assertions.
(capture-project
  (track :instrument "core/drift"))

(def capture-after-sync ()
  (set! eseq.patch-learn/%open true))

(effect-buffer "*patch-learn*"
  (h-stack :width :fill :height :fill :align :stretch :gap 0.35
    (patcher
      :intent :instrument
      :width 0
      :flex 1
      :height :fill
      :path "instruments/core/drift/dsp.lisp")
    (eseq.patch-learn/panel)))
