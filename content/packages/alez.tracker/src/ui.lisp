;; alez.tracker — a tracker-style step editor that installs itself as a tab
;; on the main sequencer tile.
;;
;;   (import alez.tracker.ui)          ; in *scratch* (C-x C-e) or init.lisp
;;
;; That one line is the whole install: the module builds a *tracker*
;; effect-buffer, registers it with the factory step-tab registry
;; (eseq.seq-step-tabs) and selects it, so a "Tracker" tab appears next to
;; "Seq" the moment the form is evaluated. Nothing in the factory UI is
;; overridden; the package only uses exported registry verbs and the SEQ
;; reactive namespace, which is the extension surface any user package has.
;;
;; Layout: one column per track, one row per step. Every cell reads
;;   NOTE VV     note name from the step's transpose (C-4 = transpose 0),
;;               velocity as two hex digits
;;   --- ..      an inactive step
;; The playhead row of each track is tinted, the cursor cell is highlighted.
;;
;; Keys (the buffer's major mode, focus the tile by clicking it):
;;   arrows            move the cursor
;;   RET               toggle the step under the cursor (space stays play/stop)
;;   BS / Delete       clear the step under the cursor
;;   a w s e d f t g y h u j k o l p
;;                     enter a note (same piano layout as musical typing,
;;                     a = C of the current octave), activating the step and
;;                     advancing the cursor by `step-advance`
;;   z / x             octave down / up
;;   - / =             nudge the step's transpose by a semitone
;;   , / .             nudge the step's velocity by 0.1
;;
;; While a track is record-armed the host's live keyboard owns the note
;; keys (it records what you play), so note entry here needs no armed track.
;;
;; M-x alez.tracker.ui/show and /hide select or drop the tab; C-c t shows it.

(module alez.tracker.ui)

(export show
        hide
        handle-key
        note-name
        hex2
        pattern-rows
        cursor-row
        cursor-track
        octave
        step-advance)

(defstate cursor-row 0)
(defstate cursor-track 0)
(defstate octave 4)
(defstate step-advance 1)

(def buffer-name "*tracker*")
(def tab-label "Tracker")
(def max-rows 64)

;; ── formatting ──────────────────────────────────────────────────────────────

(def note-names
  (list "C-" "C#" "D-" "D#" "E-" "F-" "F#" "G-" "G#" "A-" "A#" "B-"))

;; Transposes are semitones relative to C4 (MIDI 60), the convention the drum
;; rack pads and the step grid share.
(def note-name (transpose)
  (let ((n (+ 60 (floor transpose)))
         (oct (- (floor (/ n 12)) 1))
         (semi (- n (* 12 (floor (/ n 12))))))
    (str (nth note-names semi) oct)))

(def hex-digits
  (list "0" "1" "2" "3" "4" "5" "6" "7" "8" "9" "A" "B" "C" "D" "E" "F"))

(def hex2 (n)
  (let ((v (max 0 (min 255 (floor n))))
         (hi (floor (/ v 16)))
         (lo (- v (* 16 hi))))
    (str (nth hex-digits hi) (nth hex-digits lo))))

(def velocity-hex (velocity)
  (hex2 (* 127 velocity)))

;; ── reads ───────────────────────────────────────────────────────────────────

(def track-count ()
  (len SEQ.track-ids))

;; Longest pattern across tracks, so every track has a row for each step.
(def pattern-rows ()
  (min max-rows
    (max 1 (reduce |acc n| (max acc n) 1 SEQ.track-num-steps))))

(def step-active? (track row)
  (= (nth (nth SEQ.track-steps track) row) true))

(def step-transpose (track row)
  (let ((v (nth (nth SEQ.track-transposes track) row)))
    (if (= v nil) 0 v)))

(def step-velocity (track row)
  (let ((v (nth (nth SEQ.track-velocities track) row)))
    (if (= v nil) 1 v)))

;; The host publishes the playhead per (track, step) as a reactive bool while
;; a sequencer view is visible; reading it here subscribes the cell.
(def step-playhead? (track row)
  (and (= SEQ.playing true)
       (= (reactive-get "SEQ" (str "track-playhead-active-" track "-" row)) true)))

(def track-color (track)
  (let ((c (nth SEQ.track-colors track)))
    (if (= c nil) (list 0.4 0.4 0.4) c)))

;; ── writes (all through the factory natives; undo comes for free) ──────────

(def focus-track (track)
  (if (= SEQ.current-track track) nil (seq-set-track track)))

(def toggle-cursor-step ()
  (seq-toggle-track-step cursor-track cursor-row))

(def clear-cursor-step ()
  (if (step-active? cursor-track cursor-row)
    (seq-toggle-track-step cursor-track cursor-row)
    nil))

(def set-cursor-param (param value)
  (do
    (focus-track cursor-track)
    (seq-set-step-param cursor-row param value)))

(def enter-note (semi)
  (do
    (if (step-active? cursor-track cursor-row)
      nil
      (seq-toggle-track-step cursor-track cursor-row))
    (set-cursor-param :transpose (+ (* 12 (- octave 4)) semi))
    (move-row step-advance)))

(def nudge-transpose (delta)
  (set-cursor-param :transpose (+ (step-transpose cursor-track cursor-row) delta)))

(def nudge-velocity (delta)
  (set-cursor-param :velocity (+ (step-velocity cursor-track cursor-row) delta)))

;; ── cursor ──────────────────────────────────────────────────────────────────

(def wrap-index (value count)
  (let ((m (- value (* count (floor (/ value count))))))
    m))

(def move-row (delta)
  (set! cursor-row (wrap-index (+ cursor-row delta) (pattern-rows))))

(def move-track (delta)
  (set! cursor-track (wrap-index (+ cursor-track delta) (max 1 (track-count)))))

;; Same piano layout as the host's musical typing (input.rs note_from_key).
(def note-for-key (key)
  (if (= key "a") 0
  (if (= key "w") 1
  (if (= key "s") 2
  (if (= key "e") 3
  (if (= key "d") 4
  (if (= key "f") 5
  (if (= key "t") 6
  (if (= key "g") 7
  (if (= key "y") 8
  (if (= key "h") 9
  (if (= key "u") 10
  (if (= key "j") 11
  (if (= key "k") 12
  (if (= key "o") 13
  (if (= key "l") 14
  (if (= key "p") 15
    -1)))))))))))))))))

(def handle-key (key text)
  (if (= key "UP") (do (move-row -1) true)
  (if (= key "DOWN") (do (move-row 1) true)
  (if (= key "LEFT") (do (move-track -1) true)
  (if (= key "RIGHT") (do (move-track 1) true)
  (if (= key "RET") (do (toggle-cursor-step) true)
  (if (or (= key "BS") (= key "Delete")) (do (clear-cursor-step) true)
  (if (= key "z") (do (set! octave (max 0 (- octave 1))) true)
  (if (= key "x") (do (set! octave (min 8 (+ octave 1))) true)
  (if (= key "-") (do (nudge-transpose -1) true)
  (if (= key "=") (do (nudge-transpose 1) true)
  (if (= key ",") (do (nudge-velocity -0.1) true)
  (if (= key ".") (do (nudge-velocity 0.1) true)
  (let ((semi (note-for-key key)))
    (if (>= semi 0)
      (do (enter-note semi) true)
      false)))))))))))))))

;; :live-keys false + :on-key = this mode owns its bare keys: the host's
;; global step-grid shortcuts (arrows, RET, BS) and live keyboard stand down
;; while the tracker is the active buffer, and the mode's handler outranks
;; global bind-key entries such as "." Modified chords still reach the host.
(define-mode "alez.tracker.ui/tracker-mode"
  :read-only true
  :live-keys false
  :on-key "handle-key")

;; ── rendering ───────────────────────────────────────────────────────────────

(def cell-width 6.2)
(def cell-height 1.05)
(def row-col-width 2.2)
(def cell-font 10)

(def cell-text (track row)
  (if (step-active? track row)
    (str (note-name (step-transpose track row)) " " (velocity-hex (step-velocity track row)))
    "--- .."))

(def cell (track row)
  (let ((cursor? (and (= cursor-track track) (= cursor-row row)))
        (playhead? (step-playhead? track row))
        (active? (step-active? track row)))
    (box
      :key (str "tracker-cell-" track "-" row)
      :width cell-width
      :height cell-height
      :corner-radius 2
      :background-color (if cursor? :blue (if playhead? :dark-gray :transparent))
      (label (cell-text track row)
        :width cell-width
        :height cell-height
        :font-size cell-font
        :h-align :center
        :color (if cursor? :white (if active? :white :dim))
        :bg :transparent))))

(def column-header (track)
  (box
    :key (str "tracker-head-" track)
    :width cell-width
    :height cell-height
    :corner-radius 2
    :background-color (track-color track)
    (label (str (nth SEQ.track-names track))
      :width cell-width
      :height cell-height
      :font-size cell-font
      :h-align :center
      :color :black
      :bg :transparent)))

(def track-column (track)
  (v-stack :gap 0
    (column-header track)
    (each (range 0 (pattern-rows)) |row|
      (cell track row))))

(def row-number-column ()
  (v-stack :gap 0
    (label "" :key "tracker-head-rows" :width row-col-width :height cell-height :font-size cell-font)
    (each (range 0 (pattern-rows)) |row|
      (label (hex2 row)
        :key (str "tracker-row-" row)
        :width row-col-width
        :height cell-height
        :font-size cell-font
        :h-align :right
        :color (if (= cursor-row row) :white :dim)
        :bg :transparent))))

(def status-line ()
  (label (str "oct " octave "  step " cursor-row "  track " cursor-track
              "   a-p notes  z/x octave  RET toggle  BS clear  -/= transpose  ,/. velocity")
    :key "tracker-status"
    :font-size 9
    :color :dim
    :bg :transparent))

(effect-buffer "*tracker*"
  (v-stack :padding 0.6 :gap 0.4 :width :fill :height :fill
    (status-line)
    (scroll :key "tracker-scroll" :width :fill :flex 1
      (h-stack :gap 0.3
        (row-number-column)
        (each (range 0 (track-count)) |track|
          (subtree :key (str "tracker-col-" track)
            (track-column track)))))))

(set-buffer-mode-for "*tracker*" "alez.tracker.ui/tracker-mode")

;; ── install ─────────────────────────────────────────────────────────────────

(def tab-index ()
  (let ((tabs (eseq.seq-step-tabs/seq-main-step-tabs)))
    (reduce |acc i|
      (if (= (nth (nth tabs i) 1) buffer-name) (+ i 1) acc)
      0
      (range 0 (len tabs)))))

(def show ()
  (do
    (eseq.seq-step-tabs/seq-register-step-sequencer-tab tab-label buffer-name)
    (eseq.seq-step-tabs/seq-select-main-step-tab-by-index (tab-index))))

(def hide ()
  (eseq.seq-step-tabs/seq-unregister-step-sequencer-tab buffer-name))

(bind-key "C-c t" "alez.tracker.ui/show")

;; Importing the module is the install: the tab appears immediately.
(show)
