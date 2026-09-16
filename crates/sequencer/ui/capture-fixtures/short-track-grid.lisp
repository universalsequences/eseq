;; Short patterns retain row width; blank cells are noninteractive spacers.
(capture-project
  (track :sampler :name "Four steps" :num-steps 4 :steps (0 2))
  (track :sampler :name "Twenty steps" :num-steps 20 :steps (0 4 8 12 16)))
