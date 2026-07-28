;; Arrangement timeline capture fixture (docs/arrangement-timeline-ui-spec.md).
;; Three tracks and a committed song whose rows differ via per-track pattern
;; overrides (the headless capture project has a single scene), so the view
;; shows scene-lane spans, sparse track clips, and the override tint.

(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :sampler :name "Snare" :steps (4 12 (14 5)))
  (track :sampler :name "Hat" :steps ((0 12) (2 7) (4 0) (6 -5) (8 12) (10 7) (12 0) (14 -5))))

;; Rows exercise the merged lane projection: the Kick override spans two
;; adjacent rows (one merged clip), the Snare goes explicitly empty at 24
;; (pattern-id 0 = silence, rendering as a gap), and the Hat rides the scene
;; the whole way (one full-width merged clip).
(def-song "capture-demo"
  (at 0 :scene 0)
  (at 16 :scene 0 :patterns ((0 1)))
  (at 24 :scene 0 :patterns ((0 1) (1 0)))
  (at 32 :scene 0 :patterns ((1 0)))
  (at 40 :scene 0)
  :end 48)

(def capture-after-sync ()
  (do
    (seq-open-arrangement)
    ;; Keep the transport-start marker away from the viewport edge so visual
    ;; captures exercise both its ruler triangle and owning-track cursor line.
    (set-arrangement-cursor 20 0)))
