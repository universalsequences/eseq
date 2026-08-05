;; Muted-step visual regression: track 1 is soloed, so tracks 2 and 3 are
;; disabled through the production solo-mute path while their active steps,
;; duration spans, and rhythmic four-step shading remain visible.
(capture-project
  (track :sampler :name "Muted active" :steps (0 3 4 7 8 11 12 15))
  (track :sampler :name "Muted sparse" :steps (2 6 10 14))
  (track :sampler :name "Soloed" :solo true :steps (0 4 8 12)))
