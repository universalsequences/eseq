;; Solo keeps the first track lit and dims its neighbors across the panels.
(capture-project
  (track :sampler :name "Solo" :solo true :steps (0 4 8 12))
  (track :sampler :name "Dimmed" :steps (0 2 4 6 8 10 12 14))
  (track :sampler :name "Neighbor" :steps (0 4 8 12)))
