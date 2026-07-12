;; Mixer and sequencer identity-icon regression fixture. It keeps one sampler
;; and one saved instrument visible together so both sidebar icon mappings can
;; be reviewed in the production Metal renderer.
(capture-project
  (track :sampler :name "Sampler - Wing")
  (track :instrument "core/drift" :name "Key - Wing"))
