;; Production scene strip with a bank picker opened through pointer input.
(capture-project
  (scenes 26)
  (track :sampler :name "Sampler"))

(def capture-click-widgets (list "dropdown"))

(effect-buffer "*transport-bank-preview*"
  (h-stack :padding 1
    (eseq.transport/transport-scene-strip)))
