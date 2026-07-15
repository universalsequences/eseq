;; Shared project-backed track collapse helpers.

(def seq-track-collapsed? (track)
  (and (< track (len SEQ.track-collapsed))
    (nth SEQ.track-collapsed track)))

(def seq-visible-track-indices ()
  (filter
    (lambda (track) (not (seq-track-collapsed? track)))
    (range 0 SEQ.num-tracks)))

;; Saved-instrument replacement is deliberately narrower than general track
;; selection. Phase 1 only rebinds existing custom-instrument tracks; sampler,
;; rack, and modulator conversion have different teardown requirements.
(def seq-track-custom-instrument? (track)
  (and (>= track 0)
    (< track SEQ.num-tracks)
    (< track (len SEQ.track-instrument-types))
    (= (nth SEQ.track-instrument-types track) "custom")))

;; Track identity icons intentionally share the same icon names as the sound
;; browser tabs. Keeping the mapping here prevents the mixer and sequencer from
;; drifting away from the sidebar's visual language.
(def seq-track-type-icon (track)
  (if (< track (len SEQ.track-instrument-types))
    (let ((track-type (nth SEQ.track-instrument-types track)))
      (if (= track-type "sampler")
        :waveform
        (if (= track-type "custom") :piano nil)))
    nil))

(def seq-toggle-track-collapsed-ui (track)
  (seq-toggle-track-collapsed track))
