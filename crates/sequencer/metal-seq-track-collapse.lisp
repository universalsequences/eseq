;; Shared project-backed track collapse helpers.

(def seq-track-collapsed? (track)
  (and (< track (len SEQ.track-collapsed))
    (nth SEQ.track-collapsed track)))

(def seq-visible-track-indices ()
  (filter
    (lambda (track) (not (seq-track-collapsed? track)))
    (range 0 (len SEQ.track-names))))

(def seq-toggle-track-collapsed-ui (track)
  (seq-toggle-track-collapsed track))
