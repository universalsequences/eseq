;; Representative project for visually reviewing the Aura palette and its
;; track-color palette snap.
(capture-project
  (track :sampler
    :name "Sampler"
    :midi-fx ("arp")
    :audio-fx ("filter"))
  (track :instrument "core/drift" :name "Drift")
  (track :modulator :name "Modulator")
  (track :layer-rack :name "Drums"))

(load "@/ui/themes/aura.lisp")
