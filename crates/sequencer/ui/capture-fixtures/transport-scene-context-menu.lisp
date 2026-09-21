;; The floating scene menu must not enlarge the scene strip's SDF background.
(capture-project
  (scenes 26)
  (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (eseq.transport/open-scene-bank-menu (dict :col 12 :row 1.6) 0))

(effect-buffer "*transport-bank-preview*"
  (h-stack :padding 1
    (eseq.transport/transport-scene-strip)))
