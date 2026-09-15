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
;;   click / arrows    put the cursor on a cell (double-click a Note cell toggles it)
;;   RET               toggle the step under the cursor (space stays play/stop)
;;   BS / Delete       clear the step under the cursor
;;   a w s e d f t g y h u j k o l p
;;                     enter a note (same piano layout as musical typing,
;;                     a = C of the current octave), activating the step and
;;                     advancing the cursor by `step-advance`
;;   z / x             octave down / up
;;   - / =             nudge the step's transpose by a semitone
;;   , / .             nudge the step's velocity by 0.1
;;   0-9 a-f           on Vol or a lock column: type the value — one digit for
;;                     integer columns (retrig, duration), two hex digits
;;                     otherwise; a-f are hex there, not notes
;;
;; While a track is record-armed the host's live keyboard owns the note
;; keys (it records what you play), so note entry here needs no armed track.
;;
;; M-x alez.tracker.ui/show and /hide select or drop the tab; C-c t shows it.

(module alez.tracker.ui)
(import eseq.bindings)

(export show
        hide
        handle-key
        note-name
        hex2
        pattern-rows
        cursor-row
        cursor-track
        cursor-col
        select-track
        select-cell
        entry
        toggle-column
        track-columns
        open-column-menu
        octave
        step-advance)

(defstate cursor-row 0)
(defstate cursor-track 0)
(defstate cursor-col 0)
;; Columns the user added or hid per track, keyed by track id so the choice
;; survives track reorders: a list of (dict :track <id> :keys (key …)).
(defstate added-columns (list))
(defstate hidden-columns (list))
;; The open "+" picker: (dict :track t :col c :row r) or nil.
(defstate column-menu nil)
;; Pending first hex digit typed into a two-digit column ("" when none).
(defstate entry "")
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

;; The grid is as tall as the longest pattern. A shorter track repeats down
;; its column as ghosts: row r of a track of length n shows real step
;; (r mod n), drawn muted, and editing a ghost edits that real step.
(def track-len (track)
  (let ((n (nth SEQ.track-num-steps track)))
    (if (or (= n nil) (< n 1)) 1 n)))

(def real-row (track row)
  (wrap-index row (track-len track)))

(def ghost-row? (track row)
  (>= row (track-len track)))

;; First row of each repeat: where the track's loop restarts.
(def loop-start-row? (track row)
  (and (ghost-row? track row) (= (real-row track row) 0)))

;; Cells come from SEQ.tracker-rows, the host's step-major matrix:
;; (nth (nth rows step) track) is (active transpose velocity col-values…),
;; nil past the track's length. Step-major means a row subtree depends on
;; its own step index only, so a step edit re-renders one row.
(def cell (track row)
  (nth (nth SEQ.tracker-rows (real-row track row)) track))

(def step-active? (track row)
  (= (nth (cell track row) 0) true))

(def step-transpose (track row)
  (let ((v (nth (cell track row) 1)))
    (if (= v nil) 0 v)))

(def step-velocity (track row)
  (let ((v (nth (cell track row) 2)))
    (if (= v nil) 1 v)))

(def track-color (track)
  (let ((c (nth SEQ.track-colors track)))
    (if (= c nil) (list 0.4 0.4 0.4) c)))

;; ── columns ────────────────────────────────────────────────────────────────
;;
;; Renoise's effect columns, on Elektron terms: a track's columns after Note
;; and Vol are its parameter locks and process lanes. Three sources feed the
;; list, all merged by :key:
;;
;;   SEQ.track-automation     rows for every parameter that already carries a
;;                            lock (see build_track_automation_value); shown
;;                            unless the user hid them
;;   SEQ.track-lock-targets   every bindable parameter per track, grouped by
;;                            device — the "+" picker; a chosen target with no
;;                            lock yet is an empty column until typed into
;;   SEQ.track-process-lanes  process lane metadata (prob, rand, count, …);
;;                            added through the picker's Lanes group
;;   SEQ.track-process-lane-values  per-track, per-lane step values
;;
;; A column is a dict with :key :label :target :min :max :default :increment
;; and :values (one entry per step, nil = nothing on that step), plus the
;; addressing its writer needs (:slot-idx :param-idx or :instance-id :inlet).

(def track-key (track)
  (nth SEQ.track-ids track))

(def keys-for (entries track)
  (let ((hits (filter |e| (= (get e :track) (track-key track)) entries)))
    (if (= (len hits) 0) (list) (get (nth hits 0) :keys))))

(def with-keys (entries track keys)
  (append
    (filter |e| (not (= (get e :track) (track-key track))) entries)
    (list (dict :track (track-key track) :keys keys))))

(def contains? (items key)
  (> (len (filter |k| (= k key) items)) 0))

(def find-by-key (items key)
  (let ((hits (filter |item| (= (get item :key) key) items)))
    (if (= (len hits) 0) nil (nth hits 0))))

(def track-automation (track)
  (or (nth SEQ.track-automation track) (list)))

(def track-targets (track)
  (or (nth SEQ.track-lock-targets track) (list)))

(def track-lanes (track)
  (or (nth SEQ.track-process-lanes track) (list)))

(def track-added (track)
  (keys-for added-columns track))

(def track-hidden (track)
  (keys-for hidden-columns track))

(def lane-key (lane)
  (str "lane:" (get lane :instance-id) ":" (get lane :inlet)))

(def lane-column (lane)
  (dict :key (lane-key lane)
        :label (get lane :label)
        :short (get lane :short-label)
        :target "process-lane"
        :instance-id (get lane :instance-id)
        :inlet (get lane :inlet)
        :min (get lane :min)
        :max (get lane :max)
        :default (get lane :default)
        :decimals (get lane :decimals)
        :increment (if (= (get lane :decimals) 0) 1 0)
        :lane-index (get lane :lane-index)))

;; Every pickable target on a track, flat, with lanes folded in.
(def target-columns (track)
  (append
    (reduce |acc g| (append acc (get g :items)) (list) (track-targets track))
    (map |lane| (lane-column lane) (track-lanes track))))

;; The columns a track shows, in order: locked rows the user has not hidden,
;; then the user's added columns that are not already locked rows.
(def track-columns (track)
  (let ((hidden (track-hidden track))
        (auto (filter |r| (not (contains? hidden (get r :key))) (track-automation track)))
        (targets (target-columns track))
        (extra (filter |c| (not (= c nil))
                 (map |key| (if (find-by-key auto key) nil (find-by-key targets key))
                      (track-added track)))))
    (append auto extra)))

(def column-shown? (track key)
  (not (= (find-by-key (track-columns track) key) nil)))

(def set-track-keys (state-name track keys)
  (if (= state-name :added)
    (set! added-columns (with-keys added-columns track keys))
    (set! hidden-columns (with-keys hidden-columns track keys))))

(def without (items key)
  (filter |k| (not (= k key)) items))

;; Picker toggle: a shown column hides (and drops from the added list); a
;; hidden or new one shows.
(def toggle-column (track key)
  (if (column-shown? track key)
    (do
      (set-track-keys :added track (without (track-added track) key))
      (set-track-keys :hidden track (append (without (track-hidden track) key) (list key))))
    (do
      (set-track-keys :hidden track (without (track-hidden track) key))
      (set-track-keys :added track (append (without (track-added track) key) (list key))))))

;; Column idx (position in the track's column list) → its value on a row.
;; Locked/automation columns read the cell matrix; user-added targets with
;; no lock yet have no values; lanes read their separate per-step projection.
(def column-value (track idx row)
  (let ((col (nth (track-columns track) idx))
        (auto-count (len (track-automation track))))
    (if (lane-col? col) (nth (nth (nth SEQ.track-process-lane-values track)
                              (get col :lane-index)) (real-row track row))
    (if (< idx auto-count) (nth (cell track row) (+ 3 idx))
      nil))))

(def step-param-col? (col)
  (= (get col :target) "step-param"))

(def lane-col? (col)
  (= (get col :target) "process-lane"))

;; Device locks print as two hex digits over the parameter's range, the
;; tracker convention. Step params (duration in steps, retrig count, …) and
;; lanes mean something in their own units, so they print as numbers.
(def format-number (v)
  (let ((r (/ (round (* v 10)) 10)))
    (if (= r (round r)) (str (round r)) (str r))))

(def format-column (col v)
  (if (= v nil) ".."
    (if (or (step-param-col? col) (lane-col? col)) (format-number v)
      (let ((lo (get col :min)) (hi (get col :max)))
        (if (<= hi lo) (format-number v)
          (hex2 (* 255 (/ (- v lo) (- hi lo)))))))))

;; ── writes (all through the factory natives; undo comes for free) ──────────

(def focus-track (track)
  (if (= SEQ.current-track track) nil (seq-set-track track)))

;; The real step under the cursor (a ghost row writes to the step it mirrors).
(def edit-row ()
  (real-row cursor-track cursor-row))

(def toggle-cursor-step ()
  (seq-toggle-track-step cursor-track (edit-row)))

(def clear-cursor-step ()
  (if (step-active? cursor-track cursor-row)
    (seq-toggle-track-step cursor-track (edit-row))
    nil))

(def set-cursor-param (param value)
  (do
    (focus-track cursor-track)
    (seq-set-step-param (edit-row) param value)))

(def enter-note (semi)
  (do
    (if (step-active? cursor-track cursor-row)
      nil
      (seq-toggle-track-step cursor-track (edit-row)))
    (set-cursor-param :transpose (+ (* 12 (- octave 4)) semi))
    (move-row step-advance)))

(def nudge-transpose (delta)
  (set-cursor-param :transpose (+ (step-transpose cursor-track cursor-row) delta)))

(def nudge-velocity (delta)
  (set-cursor-param :velocity (+ (step-velocity cursor-track cursor-row) delta)))

;; A column writes through the same commands as the automation lane: step
;; params by index through the piano roll's history action, device params by
;; the set-track-plock-entry payload the column already carries, lanes by the
;; process-lane native.
(def set-step-param-by-index (col value)
  (host-command "piano-roll-history-action"
    (dict :track cursor-track
      :action (dict :type :set-automation-step-param
        :step (edit-row) :param-idx (get col :param-idx) :value value))))

(def plock-payload (col)
  (dict :target (get col :target)
        :step-idx (edit-row)
        :slot-idx (get col :slot-idx)
        :param-idx (get col :param-idx)))

(def cursor-column ()
  (nth (track-columns cursor-track) (- cursor-col 2)))

(def set-column (col value)
  (let ((v (max (get col :min) (min (get col :max) value))))
    (do
      (focus-track cursor-track)
      (if (step-param-col? col) (set-step-param-by-index col v)
      (if (lane-col? col)
        (seq-set-process-lane-step cursor-track (get col :instance-id) (get col :inlet) (edit-row) v)
        (host-command "set-track-plock-entry" (merge (plock-payload col) :value v)))))))

(def clear-column (col)
  (do
    (focus-track cursor-track)
    (if (or (step-param-col? col) (lane-col? col))
      (set-column col (get col :default))
      (host-command "clear-track-plock-entry" (plock-payload col)))))

;; Typing into a column, tracker style. Integer-stepped columns (retrig
;; count, duration in steps, enum params) take one digit and commit. Everything
;; else is two hex digits over the column's range, like Vol: the first digit
;; waits in `entry`, the second commits. a–f are hex here, not piano keys.
(def hex-digit (key)
  (let ((i (reduce |acc j| (if (= (nth hex-digits j) (upper-key key)) j acc) -1 (range 0 16))))
    i))

(def upper-key (key)
  (if (= key "a") "A" (if (= key "b") "B" (if (= key "c") "C"
  (if (= key "d") "D" (if (= key "e") "E" (if (= key "f") "F" key)))))))

(def two-digit-col? (col)
  (< (get col :increment) 1))

(def commit-hex (col hi lo)
  (let ((n (+ (* 16 hi) lo))
        (lo-v (get col :min))
        (hi-v (get col :max)))
    (set-column col (+ lo-v (* (- hi-v lo-v) (/ n 255))))))

(def type-column (col digit key)
  (if (not (two-digit-col? col))
    ;; The digit is the value itself (clamped to the column's range).
    (if (< digit 10) (set-column col digit) nil)
    (if (= entry "")
      (set! entry (upper-key key))
      (do
        (commit-hex col (hex-digit entry) digit)
        (set! entry "")))))

;; Vol behaves as a two-digit hex column over 0..1.
(def vol-column ()
  (dict :key "vol" :target "velocity" :min 0 :max 1 :default 1 :increment 0))

(def type-cursor (key)
  (let ((digit (hex-digit key)))
    (if (< digit 0) false
    (if (= cursor-col 1)
      (do
        (if (= entry "")
          (set! entry (upper-key key))
          (do
            (set-cursor-param :velocity (/ (+ (* 16 (hex-digit entry)) digit) 255))
            (set! entry "")))
        true)
    (if (automation-col?)
      (do (type-column (cursor-column) digit key) true)
      false)))))

(def nudge-column (col delta)
  (let ((cur (column-value cursor-track (- cursor-col 2) cursor-row))
        (base (if (= cur nil) (get col :default) cur))
        (inc (get col :increment))
        (step (if (>= inc 1) inc (/ (- (get col :max) (get col :min)) 32))))
    (set-column col (+ base (* delta step)))))

;; ── cursor ──────────────────────────────────────────────────────────────────

(def wrap-index (value count)
  (let ((m (- value (* count (floor (/ value count))))))
    m))

;; The cursor is Lisp state for the key handler, but the grid never reads
;; it: cells bind their highlight to channels written here (eseq.bindings),
;; so a cursor move touches two rows' slots and re-renders nothing.
;;   cur-<t>-<r>    one per grid cell row: 1 at the cursor sub-column
;;   cursor-rows    one per grid row: 1 at the cursor row (the gutter)
;;   center-row     cells, the follow target while stopped
(def B (eseq.bindings/scope "alez.tracker"))

(def cursor-ch (track row)
  (eseq.bindings/channel B (str "cur-" track "-" row)))

(def cursor-rows-ch (eseq.bindings/channel B "cursor-rows"))
(def center-row-ch (eseq.bindings/channel B "center-row"))

(def set-cursor (track row col)
  (do
    (eseq.bindings/clear! (cursor-ch cursor-track cursor-row))
    ;; The host's current track follows the cursor (like Renoise), which is
    ;; what the gutter, headers and follow scroll key off.
    (focus-track track)
    (set! cursor-track track)
    (set! cursor-row row)
    (set! cursor-col col)
    (eseq.bindings/one-hot! (cursor-ch track row) (column-count track) col)
    (eseq.bindings/one-hot! cursor-rows-ch (pattern-rows) row)
    (eseq.bindings/write! center-row-ch (* row-h row))))

(def move-row (delta)
  (set-cursor cursor-track (wrap-index (+ cursor-row delta) (pattern-rows)) cursor-col))

(def move-track (delta)
  (set-cursor (wrap-index (+ cursor-track delta) (max 1 (track-count))) cursor-row
              (min cursor-col (- (column-count (wrap-index (+ cursor-track delta) (max 1 (track-count)))) 1))))

(def select-track (track)
  (set-cursor track cursor-row (min cursor-col (- (column-count track) 1))))

;; Mouse: a click puts the cursor on that sub-cell; a double-click on a Note
;; cell toggles the step (writing through a ghost row to its real step).
(def select-cell (track row col)
  (set-cursor track row col))

(def double-click-cell (track row col)
  (do
    (select-cell track row col)
    (if (= col 0) (toggle-cursor-step) nil)))

;; Sub-columns of a track: 0 = Note, 1 = Vol, 2.. = its p-lock columns.
(def column-count (track)
  (+ 2 (len (track-columns track))))

(def automation-col? ()
  (>= cursor-col 2))

;; LEFT/RIGHT walk the sub-columns and spill into the neighbouring track at
;; either edge, like Renoise.
(def move-col (delta)
  (let ((next (+ cursor-col delta))
        (tracks (max 1 (track-count))))
    (if (< next 0)
      (let ((t (wrap-index (- cursor-track 1) tracks)))
        (set-cursor t cursor-row (- (column-count t) 1)))
    (if (>= next (column-count cursor-track))
      (set-cursor (wrap-index (+ cursor-track 1) tracks) cursor-row 0)
      (set-cursor cursor-track cursor-row next)))))

;; -/= nudge whatever the cursor sits on: transpose, velocity or a lock.
(def nudge-cursor (delta)
  (if (= cursor-col 0) (nudge-transpose delta)
  (if (= cursor-col 1) (nudge-velocity (* 0.1 delta))
    (nudge-column (cursor-column) delta))))

(def clear-cursor ()
  (if (automation-col?)
    (clear-column (cursor-column))
    (clear-cursor-step)))

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
  (if (and (not (= entry "")) (not (>= (hex-digit key) 0)))
    (set! entry "")
    nil)
  (if (= key "UP") (do (move-row -1) true)
  (if (= key "DOWN") (do (move-row 1) true)
  (if (= key "LEFT") (do (move-col -1) true)
  (if (= key "RIGHT") (do (move-col 1) true)
  (if (= key "RET") (do (toggle-cursor-step) true)
  (if (or (= key "BS") (= key "Delete")) (do (clear-cursor) true)
  (if (= key "z") (do (set! octave (max 0 (- octave 1))) true)
  (if (= key "x") (do (set! octave (min 8 (+ octave 1))) true)
  (if (= key "-") (do (nudge-cursor -1) true)
  (if (= key "=") (do (nudge-cursor 1) true)
  (if (= key ",") (do (nudge-velocity -0.1) true)
  (if (= key ".") (do (nudge-velocity 0.1) true)
  (if (and (>= cursor-col 1) (type-cursor key)) true
  (let ((semi (note-for-key key)))
    (if (>= semi 0)
      (do (enter-note semi) true)
      false))))))))))))))))

;; :live-keys false + :on-key = this mode owns its bare keys: the host's
;; global step-grid shortcuts (arrows, RET, BS) and live keyboard stand down
;; while the tracker is the active buffer, and the mode's handler outranks
;; global bind-key entries such as "." Modified chords still reach the host.
(define-mode "alez.tracker.ui/tracker-mode"
  :read-only true
  :live-keys false
  :on-key "handle-key")

;; ── rendering ───────────────────────────────────────────────────────────────
;;
;; Renoise-style pattern editor. The layout is one pinned header row and one
;; scrolling body; every track's cells live in the same body rows, so all
;; columns share the scroll. Each row is its own subtree, so a step edit
;; re-renders one row; scrolling and follow never relayout. The playhead never re-renders anything: rows bind
;; their highlight to SEQ.track-grid-playhead-<t> by index (host-published,
;; ghost copies included), which moves two floats per tick.
;;
;; Wide projects pan sideways through the tile's smooth horizontal scroll;
;; a track's header chevron collapses it to Note/Vol.

(def note-w 3.0)
(def vol-w 2.0)
(def auto-w 2.5)
(def sub-gap 0.2)
(def row-h 1.0)
(def gutter-w 2.2)
(def cell-font 10.5)
(def head-font 10.5)
(def sub-font 8)
(def head-extra 2.0)
(def track-gap 0.4)

;; A column is at least auto-w wide and grows to fit its full label, so
;; headers are never truncated.
;; Headers use the host's compact spelling (:short, e.g. voicing.character →
;; vcn.chr); lanes bring their own short-label. The picker keeps full names.
(def column-title (col)
  (let ((name (if (get col :short-field) (reactive-get "SEQ" (get col :short-field)) nil)))
    (if (= name nil) (or (get col :short) (get col :label)) name)))

(def column-label (col)
  (let ((name (if (get col :label-field) (reactive-get "SEQ" (get col :label-field)) nil)))
    (if (= name nil) (get col :label) name)))

(def column-w (col)
  (max auto-w (+ 0.6 (* 0.62 (len (column-title col))))))

(def columns-w (cols)
  (reduce |acc c| (+ acc (column-w c) sub-gap) 0 cols))

;; Width of a track's cells; the header adds head-extra for its buttons and
;; rows match it so the two stay aligned.
(def col-w (cols)
  (+ note-w vol-w sub-gap (columns-w cols)))
(def track-w (cols)
  (+ (col-w cols) head-extra))

(def col-panel-border (rgba 1.0 1.0 1.0 0.08))
(def row-beat-bg (rgba 1.0 1.0 1.0 0.045))
(def row-bar-bg (rgba 1.0 1.0 1.0 0.10))
(def row-playhead-bg (rgba 1.0 1.0 1.0 0.20))
(def cursor-bg :primary)
(def vol-color (rgba 0.95 0.78 0.35 1.0))
(def auto-color (rgba 0.55 0.80 1.0 1.0))
(def empty-color :dimmer)

(def beat-row? (row) (= (wrap-index row 4) 0))
(def bar-row? (row) (= (wrap-index row 16) 0))

;; ── collapsing ─────────────────────────────────────────────────────────────
;;
;; Wide projects scroll sideways through the tile's own smooth horizontal
;; widget scroll (the body's vertical `scroll` declines sideways swipes), so
;; nothing here re-renders on a pan. A track's « header button folds its
;; lock columns away when it is not the one being edited.

(defstate collapsed (list))

(def visible-tracks ()
  (range 0 (track-count)))

(def collapsed? (track)
  (contains? collapsed (track-key track)))

(def toggle-collapse (track)
  (set! collapsed
    (if (collapsed? track)
      (without collapsed (track-key track))
      (append collapsed (list (track-key track))))))

;; The columns a track draws: none while collapsed.
(def shown-columns (track)
  (if (collapsed? track) (list) (track-columns track)))

;; ── cells ───────────────────────────────────────────────────────────────────

(def ghost-tint (track alpha)
  (let ((c (track-color track)))
    (rgba (nth c 0) (nth c 1) (nth c 2) alpha)))

;; Ghost rows sit on a wash of the track's own color, a little stronger on
;; the row where the loop restarts, so each track's period reads at a glance.
(def row-bg (track row)
  (if (loop-start-row? track row) (ghost-tint track 0.16)
  (if (ghost-row? track row) (ghost-tint track 0.06)
  (if (bar-row? row) row-bar-bg
  (if (beat-row? row) row-beat-bg
    :transparent)))))

(def note-text (track row)
  (if (step-active? track row) (note-name (step-transpose track row)) "---"))

(def vol-text (track row)
  (if (step-active? track row) (velocity-hex (step-velocity track row)) ".."))

(def sub-cell (text track row col width color active?)
  (let ((ghost? (ghost-row? track row)))
    (box
      :key (str "tracker-cell-" track "-" row "-" col)
      :width width
      :height row-h
      :corner-radius 2
      :background-color :transparent
      :selected (eseq.bindings/bound-nth (cursor-ch track row) col)
      :selected-background-color cursor-bg
      :on-click (lambda (event) (select-cell track row col))
      :on-double-click (lambda (event) (double-click-cell track row col))
      (label text
        :width width
        :height row-h
        :font-size cell-font
        :mono true
        :h-align :center
        :color (if active? (if ghost? :dim color) empty-color)
        :bg :transparent))))

(def column-cell (track row cols idx)
  (let ((col (nth cols idx))
        (v (column-value track idx row))
        ;; Lanes and step params are dense, so a value still at its default
        ;; draws dim rather than bright.
        (lit? (and (not (= v nil))
                   (not (and (or (lane-col? col) (step-param-col? col))
                             (= v (get col :default)))))))
    (sub-cell (format-column col v) track row (+ 2 idx) (column-w col) auto-color lit?)))

;; One track's cells on one grid row. The playhead highlight is a bound
;; `selected`, not a rendered prop.
(def track-row (track row)
  (let ((cols (shown-columns track))
        (active? (step-active? track row)))
    (box
      :key (str "tracker-row-" track "-" row)
      :width (track-w cols)
      :height row-h
      :corner-radius 2
      :background-color (row-bg track row)
      :selected (bind-seq-nth (str "track-grid-playhead-" track) row)
      :selected-background-color row-playhead-bg
      (h-stack :gap sub-gap
        (sub-cell (note-text track row) track row 0 note-w :fg active?)
        (sub-cell (vol-text track row) track row 1 vol-w vol-color active?)
        (each (range 0 (len cols)) |idx|
          (column-cell track row cols idx))))))

(def row-number (row)
  (box
    :key (str "tracker-gutter-" row)
    :width gutter-w
    :height row-h
    :corner-radius 2
    :background-color (if (bar-row? row) row-bar-bg :transparent)
    :selected (eseq.bindings/bound-nth cursor-rows-ch row)
    :selected-background-color (rgba 1.0 1.0 1.0 0.16)
    ;; The shared gutter follows the cursor's track's playhead.
    (label (if (< row 10) (str "0" row) (str row))
      :key (str "tracker-row-" row)
      :width gutter-w
      :height row-h
      :font-size cell-font
      :mono true
      :h-align :right
      :active (bind-seq-nth "track-grid-playhead-current" row)
      :active-color :white
      :color (if (beat-row? row) :fg :dim)
      :bg :transparent)))

;; One grid row across every visible track.
(def grid-row (row)
  (h-stack :gap track-gap
    (row-number row)
    (each (visible-tracks) |track|
      (track-row track row))))

;; ── headers ─────────────────────────────────────────────────────────────────

(def sub-header (text width key)
  (label text
    :key key
    :width width :height 0.8 :font-size sub-font :mono true :h-align :center
    :color :dim :bg :transparent))

(def open-column-menu (track event)
  (set! column-menu (dict :track track :col (get event :col) :row (get event :row))))

(def header-button (text key on-click)
  (box
    :key key
    :width 0.9 :height 0.8 :corner-radius 2
    :background-color (rgba 1.0 1.0 1.0 0.07)
    :on-click on-click
    (label text :width 0.9 :height 0.8 :font-size sub-font :mono true :h-align :center
      :color :fg :bg :transparent)))

;; Track names longer than the panel are ellipsized; roughly 1.6
;; proportional characters fit per cell at the header size.
(def fit-name (name width)
  (let ((fit (floor (* width 1.6))))
    (if (<= (len name) fit) name
      (str (substring name 0 (max 1 (- fit 1))) "…"))))

(def column-header (track)
  (let ((cols (shown-columns track))
        (hidden-count (if (collapsed? track) (len (track-columns track)) 0))
        (c (track-color track)))
    (box
      :key (str "tracker-panel-" track)
      :width (track-w cols)
      :corner-radius 4
      :border-width 1
      :border-color (if (= SEQ.current-track track) (rgba 1.0 1.0 1.0 0.22) col-panel-border)
      (v-stack :gap 0.15
        (box
          :key (str "tracker-strip-" track)
          :width (track-w cols)
          :height 0.4
          :corner-radius 2
          :background-color (rgba (nth c 0) (nth c 1) (nth c 2) 1.0))
        (label (fit-name (str (nth SEQ.track-names track)) (track-w cols))
          :key (str "tracker-head-" track)
          :width (track-w cols)
          :height 1.1
          :font-size head-font
          :h-align :center
          :color (if (= SEQ.current-track track) :white :fg)
          :on-click (list "alez.tracker.ui/select-track" track)
          :bg :transparent)
        (h-stack :gap sub-gap
          (sub-header "Note" note-w (str "tracker-sub-note-" track))
          (sub-header "Vol" vol-w (str "tracker-sub-vol-" track))
          (each (range 0 (len cols)) |idx|
            (sub-header (column-title (nth cols idx))
                        (column-w (nth cols idx)) (str "tracker-sub-col-" track "-" idx)))
          ;; « collapses the lock columns (badge shows how many are folded);
          ;; + opens the column picker.
          (header-button (if (collapsed? track) (str hidden-count) "«")
                         (str "tracker-collapse-" track)
                         (lambda (event) (toggle-collapse track)))
          (header-button "+" (str "tracker-add-col-" track)
                         (lambda (event) (open-column-menu track event))))))))

;; Full width of the gutter and every track panel; the body scroll is sized
;; to this rather than the viewport so rows past the right edge still lay
;; out and the tile's horizontal scroll can reach them.
(def grid-width ()
  (reduce |acc track| (+ acc (track-w (shown-columns track)) track-gap)
          (+ gutter-w track-gap)
          (visible-tracks)))

(def header-row ()
  (h-stack :gap track-gap :key "tracker-headers"
    (box :key "tracker-head-rows" :width gutter-w :height 1)
    (each (visible-tracks) |track|
      (subtree :key (str "tracker-h-" track)
        (column-header track)))))

(def chip (text key)
  (box :key key :height 1.1 :padding-left 0.4 :padding-right 0.4 :corner-radius 3
       :background-color (rgba 1.0 1.0 1.0 0.07)
    (label text :height 1.1 :v-align :center :font-size 9.5 :mono true :color :fg :bg :transparent)))

;; A pending first hex digit shows here rather than in the cell, so typing
;; re-renders this one chip and not the grid.
(def entry-chip ()
  (if (= entry "") (box :key "tracker-chip-entry" :width 0 :height 1.1)
    (chip (str entry "_") "tracker-chip-entry")))

(def toolbar ()
  (h-stack :gap 0.5 :v-align :center
    (subtree :key "tracker-entry" (entry-chip))
    (chip (str "OCT " octave) "tracker-chip-oct")
    (chip (str "STEP " step-advance) "tracker-chip-step")
    (chip (str "ROWS " (pattern-rows)) "tracker-chip-rows")
    (label "a-p notes · z/x octave · RET toggle · BS clear · -/= nudge · ,/. vol"
      :key "tracker-status" :font-size 9 :color :dim :bg :transparent)))

;; ── column picker ───────────────────────────────────────────────────────────
;;
;; A nested context menu: one submenu per device (Step, the instrument, each
;; FX and MIDI FX slot, Macros) plus Lanes for the track's process lanes.
;; Shown columns are checked; selecting toggles them.

(def picker-item (track col prefix)
  (let ((key (get col :key)))
    (menu-item (column-label col)
      :key (str prefix key)
      :checked (column-shown? track key)
      :on-select (lambda (event) (toggle-column track key)))))

(def picker-group (track label items prefix)
  (menu-item label :key (str prefix "group")
    (each items |col| (picker-item track col prefix))))

(def column-picker ()
  (let ((track (get column-menu :track)))
    (context-menu :is-open (not (= column-menu nil))
      :anchor-col (or (get column-menu :col) 0)
      :anchor-row (or (get column-menu :row) 0)
      :on-close (lambda () (set! column-menu nil))
      (if (= column-menu nil) nil
        (append
          (map |g|
            (picker-group track (get g :group) (get g :items)
                          (str "tracker-pick-" track "-" (get g :group) "-"))
            (track-targets track))
          (if (= (len (track-lanes track)) 0) (list)
            (list
              (picker-group track "Lanes" (map |lane| (lane-column lane) (track-lanes track))
                            (str "tracker-pick-" track "-lanes-")))))))))

(effect-buffer "*tracker*"
  (v-stack :padding 0.6 :gap 0.5 :width :fill :height :fill
    (toolbar)
    (subtree :key "tracker-header-row" (header-row))
    ;; Follow: the view keeps the cursor row centered while editing and the
    ;; cursor track's playhead row while playing, except that the first
    ;; half-screen of rows stays put (the scroll clamps at the top). The
    ;; playing target is a bound float, so ticks move the view without a
    ;; re-render.
    (scroll :key "tracker-scroll" :width (grid-width) :flex 1
      :center-row (if (= SEQ.playing true)
                    (bind-seq "track-grid-playhead-row-current")
                    (eseq.bindings/bound center-row-ch))
      :center-span row-h
      ;; A plain stack, not a virtualizing one: patterns are at most 64
      ;; rows, and a fixed row set means scrolling and follow are pure
      ;; render-time offsets with no relayout. Rows are subtrees, so a
      ;; step edit re-renders one of them.
      (v-stack :key "tracker-rows" :width (grid-width) :gap 0
        (each (range 0 (pattern-rows)) |row|
          (subtree :key (str "tracker-r-" row)
            (grid-row row)))))
    (column-picker)))

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
    ;; Ask the host for SEQ.track-automation (the p-lock columns) and pull
    ;; the first copy now rather than on the next edit.
    (set! eseq.vanilla/track-automation-wanted true)
    (host-command "piano-roll-automation-refresh" (dict))
    (eseq.seq-step-tabs/seq-register-step-sequencer-tab tab-label buffer-name)
    (eseq.seq-step-tabs/seq-select-main-step-tab-by-index (tab-index))
    (set-cursor cursor-track cursor-row cursor-col)))

(def hide ()
  (do
    (set! eseq.vanilla/track-automation-wanted false)
    (eseq.seq-step-tabs/seq-unregister-step-sequencer-tab buffer-name)))

(bind-key "C-c t" "alez.tracker.ui/show")

;; Importing the module is the install: the tab appears immediately.
(show)
