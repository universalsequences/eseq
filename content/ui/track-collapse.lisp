;; Shared project-backed track collapse helpers.

(module eseq.track-collapse)

(export collapsed?
        visible-track-indices
        custom-instrument?
        replaceable-instrument?
        sound-replaceable?
        type-icon
        group-type-icon
        toggle-collapsed-ui)

;; Migration compat aliases (spec §10 slice 3): browser.lisp, mixer.lisp,
;; sequencer.lisp and arrangement.lisp all call these bare and are still
;; unconverted. `toggle-collapsed-ui` has no caller today, but it is
;; command-shaped (the kind of name a `bind-key`/`:on-key` string reaches by
;; spelling), so it keeps an alias too.

(def collapsed? (track)
  (and (< track (len SEQ.track-collapsed))
    (nth SEQ.track-collapsed track)))

(def visible-track-indices ()
  (filter
    (lambda (track) (not (collapsed? track)))
    (range 0 SEQ.num-tracks)))

(def custom-instrument? (track)
  (and (>= track 0)
    (< track SEQ.num-tracks)
    (< track (len SEQ.track-instrument-types))
    (= (nth SEQ.track-instrument-types track) "custom")))

(def replaceable-instrument? (track)
  (and (>= track 0)
    (< track SEQ.num-tracks)
    (< track (len SEQ.track-instrument-types))
    (let ((kind (nth SEQ.track-instrument-types track)))
      (or (= kind "custom") (= kind "sampler") (= kind "rack")))))

(def sound-replaceable? (track)
  (and (>= track 0)
    (< track SEQ.num-tracks)
    (< track (len SEQ.track-instrument-types))
    (let ((kind (nth SEQ.track-instrument-types track)))
      (or (= kind "custom") (= kind "sampler") (= kind "rack")))))

;; Track identity icons intentionally share the same icon names as the sound
;; browser tabs. Keeping the mapping here prevents the mixer and sequencer from
;; drifting away from the sidebar's visual language.
(def type-icon (track)
  (if (< track (len SEQ.track-instrument-types))
    (let ((track-type (nth SEQ.track-instrument-types track)))
      (if (= track-type "sampler")
        :waveform
        (if (= track-type "custom")
          :piano
          (if (= track-type "rack")
            :sampler
            (if (= track-type "modulator") :sine nil)))))
    nil))

(def group-type-icon (group)
  ;; The browser lists Drum Rack and Instrument Rack under the same :sampler
  ;; rack glyph, so both the drum-rack group and the slot-based rack use it.
  (if (get group :rack) :sampler nil))

(def toggle-collapsed-ui (track)
  (seq-toggle-track-collapsed track))
