;; Arrangement timeline with REAL scene changes
;; (docs/arrangement-lane-model-spec.md 12, phase 5). Sibling of
;; arrangement-timeline.lisp: same three tracks, but the song launches three
;; different scenes, so the scene lane must render three DISTINCT labeled
;; spans. Together the two fixtures pin both halves of the phase-5 contract —
;; one scene = one span (no clip-edge fragmentation), N scene events = N
;; spans.

(capture-project
  ;; Three scenes to launch (the headless capture project starts with one).
  (scenes 3)
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :sampler :name "Snare" :steps (4 12 (14 5)))
  (track :sampler :name "Hat" :steps ((0 12) (2 7) (4 0) (6 -5) (8 12) (10 7) (12 0) (14 -5))))

;; Scene changes at 0/16/32, plus a Kick clip at 8 that deliberately does NOT
;; sit on a scene boundary: in the row model that clip's edges split the scene
;; lane; in the lane model the scene lane is untouched by it.
(def-song "scene-change-demo"
  (at 0 :scene 0)
  (at 8 :scene 0 :patterns ((0 1)))
  (at 16 :scene 1)
  (at 32 :scene 2)
  :end 48)

(def capture-after-sync ()
  (eseq.seq-panels/seq-open-arrangement))
