;; Deleting a clip leaves a genuine SILENT gap
;; (docs/arrangement-lane-model-spec.md 6.2).
;;
;; The headline fix of the backdrop removal. Under the old model this capture
;; would have been indistinguishable from the un-deleted song: the scene cell
;; underneath kept playing the same pattern, so deleting a clip changed the
;; timeline and not the music. Now the Snare's middle clip is gone and that
;; stretch of lane is empty, while the Kick and Hat play straight through.
;;
;; The delete is a REAL gesture, not a declarative hole: the fixture reads
;; SEQ.song-lanes for the clip's id and removes it with the same primitive the
;; Backspace key lowers to.

(capture-project
  (track :sampler :name "Kick" :steps (0 4 8 12))
  (track :sampler :name "Snare" :steps (4 12 (14 5)))
  (track :sampler :name "Hat" :steps ((0 12) (2 7) (4 0) (6 -5) (8 12) (10 7) (12 0) (14 -5))))

;; One scene event stamps every lane; the Snare's own clip over [8, 16) is
;; what the delete below removes.
(def-song "delete-demo"
  (at 0 :scene 0)
  (at 8 :scene 0 :patterns ((1 1)))
  (at 16 :scene 0)
  :end 32)

;; The id of the clip covering `beat` on `track`, read from the same surface
;; the view renders.
(def arrangement-clip-id-at (track beat)
  (let ((hits (filter (lambda (clip)
                        (and (<= (get clip :start-beat) beat)
                          (> (get clip :end-beat) beat)))
                (nth SEQ.song-lanes track))))
    (if (> (len hits) 0) (get (nth hits 0) :clip-id) nil)))

(def capture-after-sync ()
  (seq-open-arrangement)
  (seq-arrangement-clip-delete (arrangement-clip-id-at 1 8)))
