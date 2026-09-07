;; No song clips: the grid must reach both viewport edges despite the zero
;; project extent. Capture tall and short to exercise scroll fill.
(capture-project
  (track :sampler :name "Empty track"))

(def capture-after-sync ()
  (do
    (eseq.seq-panels/seq-open-arrangement)))
