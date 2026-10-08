;; Shared project-backed track collapse helpers, over eseq.kinds tracks.

(module eseq.track-collapse)

(import eseq.kinds :refer (track tracks))

(export collapsed?
        visible-track-indices
        type-icon
        instrument-icon
        replaceable-type?
        toggle-collapsed-ui)

;; Whether the track at position i is collapsed (drum-rack-v2's member rows
;; address tracks by position).
(def collapsed? (i)
  (let ((t (track i)))
    (and t t.collapsed)))

(def visible-track-indices ()
  (map (lambda (t) t.index) (filter (lambda (t) (not t.collapsed)) (tracks))))

;; Whether a track playing instrument type `kind` (track.instrument-type)
;; takes a dropped sound or instrument in place of its own.
(def replaceable-type? (kind)
  (or (= kind "empty") (= kind "custom") (= kind "sampler") (= kind "rack")))

;; Track identity icons intentionally share the same icon names as the sound
;; browser tabs. Keeping the mapping here prevents the mixer and sequencer from
;; drifting away from the sidebar's visual language.
(def instrument-icon (track-type)
  (match track-type
    "sampler" :waveform
    "custom" :piano
    "rack" :sampler
    "modulator" :sine
    "empty" :midi
    _ nil))

;; The icon of the track at position i (the piano roll's header).
(def type-icon (i)
  (let ((t (track i)))
    (if t (instrument-icon t.instrument-type) nil)))

(def toggle-collapsed-ui (i)
  (let ((t (track i)))
    (when t (toggle! t.collapsed))))
