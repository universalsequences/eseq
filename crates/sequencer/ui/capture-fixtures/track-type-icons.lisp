;; Mixer and sequencer identity-icon regression fixture. It keeps a sampler,
;; saved instrument, instrument rack, and modulator visible together so their
;; distinct sidebar-language icon mappings can be reviewed in the production
;; renderer.
(capture-project
  (track :sampler :name "Sampler - Wing")
  (track :instrument "core/drift" :name "Key - Wing")
  (track :layer-rack :name "Rack - Wing")
  (track :modulator :name "Modulator - Wing"))
