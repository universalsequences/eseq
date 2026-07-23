;; Arrangement timeline capture fixture (docs/arrangement-timeline-ui-spec.md).
;; Three tracks and a committed song whose rows differ via per-track pattern
;; overrides (the headless capture project has a single scene), so the view
;; shows scene-lane spans, sparse track clips, and the override tint.

(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Snare")
  (track :sampler :name "Hat"))

(def-song "capture-demo"
  (at 0 :scene 0)
  (at 16 :scene 0 :patterns ((0 1)))
  (at 32 :scene 0 :patterns ((0 1) (1 1)))
  :end 48)

(def capture-after-sync ()
  (seq-open-arrangement))
