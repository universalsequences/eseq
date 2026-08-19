;; Representative project for visually reviewing the complete light palette.
(capture-project
  (track :sampler
    :name "Sampler"
    :midi-fx ("arp")
    :audio-fx ("filter"))
  (track :instrument "core/drift" :name "Drift")
  (track :modulator :name "Modulator")
  (track :layer-rack :name "Drums"))

(load "@/ui/themes/mac-osx-light-theme.lisp")
