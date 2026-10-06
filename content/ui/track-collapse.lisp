;; Shared project-backed track collapse helpers.

(module eseq.track-collapse)

(export collapsed?
        visible-track-indices
        custom-instrument?
        empty-instrument?
        replaceable-instrument?
        sound-replaceable?
        type-icon
        instrument-icon
        replaceable-type?
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

(def empty-instrument? (track)
  (and (>= track 0)
    (< track SEQ.num-tracks)
    (< track (len SEQ.track-instrument-types))
    (= (nth SEQ.track-instrument-types track) "empty")))

;; Whether a track playing instrument type `kind` (track.instrument-type)
;; takes a dropped sound or instrument in place of its own.
(def replaceable-type? (kind)
  (or (= kind "empty") (= kind "custom") (= kind "sampler") (= kind "rack")))

(def replaceable-instrument? (track)
  (and (>= track 0)
    (< track SEQ.num-tracks)
    (< track (len SEQ.track-instrument-types))
    (replaceable-type? (nth SEQ.track-instrument-types track))))

;; COMPAT(eseq-0l17): replaceable-instrument? under the name
;; ui/sequencer.lisp still calls; goes when that view ports to the kinds.
(def sound-replaceable? (track)
  (replaceable-instrument? track))

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

(def type-icon (track)
  (if (< track (len SEQ.track-instrument-types))
    (instrument-icon (nth SEQ.track-instrument-types track))
    nil))

(def group-type-icon (group)
  ;; The browser lists Drum Rack and Instrument Rack under the same :sampler
  ;; rack glyph, so both the drum-rack group and the slot-based rack use it.
  (if (get group :rack) :sampler nil))

(def toggle-collapsed-ui (track)
  (seq-toggle-track-collapsed track))
