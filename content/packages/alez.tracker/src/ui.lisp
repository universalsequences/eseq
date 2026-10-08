;; alez.tracker — a tracker-style step editor that installs itself as a tab
;; on the main sequencer tile.
;;
;;   (import alez.tracker.ui)          ; in *scratch* (C-x C-e) or init.lisp
;;
;; That one line is the whole install: the module builds a *tracker*
;; effect-buffer, registers it with the factory step-tab registry
;; (eseq.seq-step-tabs) and selects it, so a "Tracker" tab appears next to
;; "Seq" the moment the form is evaluated. Nothing in the factory UI is
;; overridden; the package reads and edits the project through eseq.kinds
;; (docs/kind-bindings-spec.md §14.2j), the extension surface any user
;; package has.
;;
;; Layout: one column per track, one row per step. Every cell reads
;;   NOTE VV     note name from the step's transpose (C-4 = transpose 0),
;;               velocity as two hex digits (00-7F)
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
;;                     otherwise (Vol 00-7F); a-f are hex there, not notes
;;
;; While a track is record-armed the host's live keyboard owns the note
;; keys (it records what you play), so note entry here needs no armed track.
;;
;; M-x alez.tracker.ui/show and /hide select or drop the tab; C-c t shows it.

(module alez.tracker.ui)
(import eseq.view-kit :refer (listed? index-of color-rgba open-menu! menu-of))
(import eseq.kinds :refer (track tracks selection transport focus-step-params
                           focus-step-value set-step-param! lock-param! unlock-param!
                           lock-rack-macro! unlock-rack-macro! set-lane-steps!))

(export show
        hide
        handle-key
        note-name
        hex2
        compact-label
        pattern-rows
        select-track
        select-cell
        toggle-column
        toggle-collapse
        track-columns
        column-value
        lane-key
        open-column-menu
        tracker-cursor
        tracker-view
        tracker-menu)

;; ── view state ──────────────────────────────────────────────────────────────

;; The cursor: the cell the keys edit, as the track's position, the grid row
;; and the sub-column (0 Note, 1 Vol, 2.. the track's columns). Cells bind
;; these and compare them in their shader (tracker-cursor-lamp), so a move
;; only repaints.
(def-kind tracker-cursor
  :key ()
  :state ((track 0)
          (row 0)
          (col 0)))

;; Columns the user added or hid per track, keyed by the track's id so the
;; choice survives track reorders: lists of (dict :track <tid> :keys (key …)),
;; nil for none. `collapsed`: the ids of the tracks whose columns are folded
;; away. `entry`: a pending first hex digit (0-15) typed into a two-digit
;; column, "" when none.
(def-kind tracker-view
  :key ()
  :state ((added :any :default nil)
          (hidden :any :default nil)
          (collapsed :any :default nil)
          (entry :any :default "")
          (octave 4)
          (step-advance 1)))

;; The "+" column picker: open, where (a pointer event's grid point) and
;; for which track.
(def-kind tracker-menu
  :key ()
  :state ((open false)
          (at :any :default nil)
          (track track :default nil)))

(def buffer-name "*tracker*")
(def tab-label "Tracker")

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

;; The keys that type them (downcased).
(def hex-keys
  (list "0" "1" "2" "3" "4" "5" "6" "7" "8" "9" "a" "b" "c" "d" "e" "f"))

(def hex2 (n)
  (let ((v (max 0 (min 255 (floor n))))
         (hi (floor (/ v 16)))
         (lo (- v (* 16 hi))))
    (str (nth hex-digits hi) (nth hex-digits lo))))

;; Velocity 0-1 as 00-7F, the MIDI range (typing Vol stores n/127, which
;; the host's f32 holds a hair under: the nudge reads it back as n).
(def velocity-hex (velocity)
  (hex2 (+ (* 127 velocity) 0.001)))

;; A tracker-width spelling of a parameter label, built rather than curated:
;; tokens (split on _ . - / and spaces) lose their vowels after the first
;; letter and are cut to three characters, and only the last two tokens
;; survive, joined with a dot. Two dgen spellings are read first: the
;; __dgen_mod_active__<param> flag becomes ~<param>, and
;; "mod <param> slot N amt" becomes <param>~N.
;;
;;   voicing.character → vcn.chr     body.damping → bdy.dmp
;;   lp_freq           → lp.frq      cutoff       → ctf
(def mod-active-prefix "__dgen_mod_active__")
(def label-separators (list "_" "." "-" " " "/"))
(def vowels (list "a" "e" "i" "o" "u"))

(def char-at (s i) (substring s i (+ i 1)))

(def label-tokens (s)
  (filter |token| (not (empty? token))
    (string-split
      (apply str (map |i| (if (listed? (char-at s i) label-separators) "_" (char-at s i))
                      (range 0 (len s))))
      "_")))

(def compact-token (token)
  (if (<= (len token) 3)
    token
    (reduce |kept i|
      (if (or (>= (len kept) 3)
              (and (> i 0) (listed? (string-downcase (char-at token i)) vowels)))
        kept
        (str kept (char-at token i)))
      ""
      (range 0 (len token)))))

(def digits? (s)
  (and (not (empty? s))
       (reduce |all i| (and all (listed? (char-at s i) (list "0" "1" "2" "3" "4" "5" "6" "7" "8" "9")))
               true (range 0 (len s)))))

;; ("<param>" "N") of a "mod <param> slot N amt" label, else nil.
(def mod-slot-label (label)
  (when (string-starts-with? label "mod ")
    (let ((rest (substring label 4))
          (parts (string-split rest " slot "))
          (tail (nth parts (- (len parts) 1)))
          (slot (string-trim (if (string-ends-with? tail " amt")
                               (substring tail 0 (- (len tail) 4))
                               tail))))
      (when (and (> (len parts) 1) (digits? slot))
        (list (substring rest 0 (- (len rest) (len tail) 6)) slot)))))

(def compact-label (label)
  (if (string-starts-with? label mod-active-prefix)
    (str "~" (compact-label (substring label (len mod-active-prefix))))
    (let ((slot (mod-slot-label label))
          (tokens (map compact-token (label-tokens label)))
          (count (len tokens)))
      (if slot
        (str (compact-label (first slot)) "~" (nth slot 1))
        (if (empty? tokens)
          label
          (if (= count 1)
            (first tokens)
            (str (nth tokens (- count 2)) "." (nth tokens (- count 1)))))))))

;; ── reads ───────────────────────────────────────────────────────────────────

(def track-at (i) (nth (tracks) i))

(def track-count () (len (tracks)))

(def track-len (t) (max 1 t.num-steps))

;; Longest pattern across tracks, so every track has a row for each step
;; (the grid track.playhead-row lights; a pattern is at most 256 steps).
(def pattern-rows ()
  (reduce |acc t| (max acc (track-len t)) 1 (tracks)))

;; The grid is as tall as the longest pattern. A shorter track repeats down
;; its column as ghosts: row r of a track of length n shows real step
;; (r mod n), drawn muted, and editing a ghost edits that real step.
(def real-row (t row)
  (wrap-index row (track-len t)))

(def ghost-row? (t row)
  (>= row (track-len t)))

;; First row of each repeat: where the track's loop restarts.
(def loop-start-row? (t row)
  (and (ghost-row? t row) (= (real-row t row) 0)))

;; The step a row of t's column shows (its real step).
(def row-step (t row)
  (nth t.steps (real-row t row)))

(def step-active? (s)
  (and s s.active))

;; ── columns ────────────────────────────────────────────────────────────────
;;
;; Renoise's effect columns, on Elektron terms: a track's columns after Note
;; and Vol are its step params, parameter locks and process lanes. A column
;; is a dict: :key (stable across renders), :kind, :src, :label, :min, :max,
;; :increment, :default where it is fixed and, once shown, :short (the
;; header's compact spelling; a lane brings its own). By kind:
;;
;;   :step   a step param, :src its name (a step field, focus-step-params)
;;   :param  a device param, :src the param (its values: p.step-locks)
;;   :macro  a drum rack macro, :src the rack macro (rm.step-locks)
;;   :lane   a process lane, :src the lane (l.values)
;;
;; A track shows the step params an active step holds off their default
;; (t.step-params-in-use) and the device params and rack macros with a lock
;; (has-locks), unless the user hid them, then the columns the user added
;; (the "+" picker).

(def step-column (d)
  (dict :key (str "step:" (get d :name)) :kind :step :src (get d :name)
        :label (get d :label) :min (get d :min) :max (get d :max) :default (get d :default)
        :increment (get d :increment)))

;; Velocity and transpose are every row's Vol and Note cells.
(def step-columns ()
  (map step-column
       (filter |d| (not (listed? (get d :name) (list "velocity" "transpose")))
               (focus-step-params))))

(def param-column (d p)
  (dict :key (str "param:" d.role ":" d.did ":" p.index) :kind :param :src p
        :label p.name :min p.min :max p.max
        :increment (if (= p.type "continuous") 0 1)))

(def macro-column (rm)
  (dict :key (str "macro:" rm.index) :kind :macro :src rm
        :label rm.name :min 0 :max 1 :increment 0))

(def lane-key (l)
  (str "lane:" l.process.proc-id ":" l.inlet))

(def lane-column (l)
  (dict :key (lane-key l) :kind :lane :src l
        :label l.label :short l.short-label :min l.min :max l.max :default l.default
        :increment (if (= l.decimals 0) 1 0)))

(def device-columns (devices keep?)
  (reduce |acc d| (append acc (map |p| (param-column d p) (filter keep? d.params)))
          (list) devices))

(def rack-macros (t)
  (reduce |acc d| (append acc d.macros) (list) t.devices))

;; The columns t has values in: step params off their default (the host's
;; t.step-params-in-use, so this reads no step), then the locked instrument
;; and effect params, rack macros and MIDI effect params.
(def locked-columns (t)
  (append
    (let ((in-use t.step-params-in-use))
      (filter |col| (listed? (get col :src) in-use) (step-columns)))
    (device-columns t.devices |p| p.has-locks)
    (map macro-column (filter |rm| rm.has-locks (rack-macros t)))
    (device-columns t.midi-devices |p| p.has-locks)))

;; Every column t can show, grouped as the picker lists them (lanes apart).
(def device-group-label (d)
  (if (= d.role "instrument") d.name (str "FX " (+ d.slot 1) " · " d.name)))

(def target-groups (t)
  (filter |g| (not (empty? (get g :items)))
    (append
      (list (dict :group "Step" :items (step-columns)))
      (map |d| (dict :group (device-group-label d) :items (device-columns (list d) |p| true))
           t.devices)
      (map |d| (dict :group (str "MIDI FX " (+ d.slot 1) " · " d.name)
                     :items (device-columns (list d) |p| true))
           t.midi-devices)
      (list (dict :group "Macros" :items (map macro-column (rack-macros t)))))))

(def target-columns (t)
  (append
    (reduce |acc g| (append acc (get g :items)) (list) (target-groups t))
    (map lane-column t.lanes)))

(def keys-for (entries t)
  (let ((hit (first (filter |e| (= (get e :track) t.tid) (or entries (list))))))
    (if hit (get hit :keys) (list))))

(def with-keys (entries t keys)
  (append
    (filter |e| (not (= (get e :track) t.tid)) (or entries (list)))
    (list (dict :track t.tid :keys keys))))

(def find-by-key (cols key)
  (first (filter |col| (= (get col :key) key) cols)))

;; The columns t shows, in order: its locked columns the user has not
;; hidden, then the user's added columns that are not already among them.
(def track-columns (t)
  (let ((hidden (keys-for tracker-view.hidden t))
        (added (keys-for tracker-view.added t))
        (auto (filter |col| (not (listed? (get col :key) hidden)) (locked-columns t)))
        (targets (if (empty? added) (list) (target-columns t))))
    (append auto
      (filter |col| col
        (map |key| (if (find-by-key auto key) false (find-by-key targets key)) added)))))

(def column-shown? (t key)
  (if (find-by-key (track-columns t) key) true false))

(def without (items x)
  (filter |y| (not (= y x)) (or items (list))))

;; Picker toggle: a shown column hides (and drops from the added list); a
;; hidden or new one shows.
(def toggle-column (t key)
  (let ((added (keys-for tracker-view.added t))
        (hidden (keys-for tracker-view.hidden t))
        (shown (column-shown? t key)))
    (set! tracker-view.added
      (with-keys tracker-view.added t
        (if shown (without added key) (append (without added key) (list key)))))
    (set! tracker-view.hidden
      (with-keys tracker-view.hidden t
        (if shown (append (without hidden key) (list key)) (without hidden key))))))

;; Column col's value on row `row` of t's column: a step param's while its
;; step is active (focus-step-value reads a step's params too: a step has a
;; focus step's fields), a lock where its step holds one, a lane's always;
;; nil for nothing there.
(def lock-at (locks step)
  (let ((hit (first (filter |l| (= (first l) step) locks))))
    (when hit (nth hit 1))))

(def column-value (t col row)
  (let ((step (real-row t row))
        (src (get col :src)))
    (match (get col :kind)
      :step (let ((s (nth t.steps step)))
              (when (step-active? s) (focus-step-value s src)))
      :lane (nth src.values step)
      _ (lock-at src.step-locks step))))

;; A device param's or rack macro's default is its base (its own value).
(def column-default (col)
  (let ((src (get col :src)))
    (match (get col :kind)
      :param src.base
      :macro src.base
      _ (get col :default))))

(def dense-col? (col)
  (or (= (get col :kind) :step) (= (get col :kind) :lane)))

;; Device locks print as two hex digits over the parameter's range, the
;; tracker convention. Step params (duration in steps, retrig count, …) and
;; lanes mean something in their own units, so they print as numbers.
(def format-number (v)
  (let ((r (/ (round (* v 10)) 10)))
    (if (= r (round r)) (str (round r)) (str r))))

(def format-column (col v)
  (if (= v nil) ".."
    (if (dense-col? col) (format-number v)
      (let ((lo (get col :min)) (hi (get col :max)))
        (if (<= hi lo) (format-number v)
          (hex2 (* 255 (/ (- v lo) (- hi lo)))))))))

;; ── writes (each one undo entry) ───────────────────────────────────────────

;; The host's current track follows the cursor (like Renoise).
(def focus-track (t)
  (unless (= selection.track t) (set! selection.track t)))

;; The cursor's track position, kept on the tracks (it outlives a deleted
;; track).
(def clamped-index (index)
  (clamp index 0 (max 0 (- (track-count) 1))))

(def cursor-index ()
  (clamped-index tracker-cursor.track))

(def cursor-track () (track-at (cursor-index)))

;; The real step under the cursor (a ghost row writes to the step it mirrors).
(def cursor-step ()
  (row-step (cursor-track) tracker-cursor.row))

(def toggle-cursor-step ()
  (let ((s (cursor-step)))
    (toggle! s.active)))

(def clear-cursor-step ()
  (let ((s (cursor-step)))
    (when s.active (set! s.active false))))

;; A step param's description (focus-step-params), by its name.
(def step-param (name)
  (first (filter |d| (= (get d :name) name) (focus-step-params))))

;; A note sets the step's transpose (within its range) and turns an empty
;; step on in the same undo entry.
(def enter-note (semi)
  (let ((s (cursor-step))
        (bounds (step-param "transpose"))
        (transpose (+ (* 12 (- tracker-view.octave 4)) semi)))
    (set-step-param! s "transpose" (clamp transpose (get bounds :min) (get bounds :max))
                     :activate true)
    (move-row tracker-view.step-advance)))

(def nudge-transpose (delta)
  (let ((s (cursor-step)))
    (set! s.transpose (+ s.transpose delta))))

(def nudge-velocity (delta)
  (let ((s (cursor-step)))
    (set! s.velocity (+ s.velocity delta))))

;; The column the cursor is on, or nil (on Note or Vol, or past the track's
;; columns: a column went away under the cursor).
(def cursor-column ()
  (when (automation-col?)
    (nth (track-columns (cursor-track)) (- tracker-cursor.col 2))))

;; A step param writes the live step the cell shows (set-step-param!, never
;; the piano roll's edit focus, which may be a pinned take).
(def set-column (col value)
  (let ((v (clamp value (get col :min) (get col :max)))
        (s (cursor-step))
        (src (get col :src)))
    (match (get col :kind)
      :step (set-step-param! s src v)
      :lane (set-lane-steps! src (list s) v)
      :param (lock-param! src (list s) v)
      _ (lock-rack-macro! src (list s) v))))

(def clear-column (col)
  (let ((s (cursor-step))
        (src (get col :src)))
    (match (get col :kind)
      :param (unlock-param! src (list s))
      :macro (unlock-rack-macro! src (list s))
      _ (set-column col (get col :default)))))

;; Typing into a column, tracker style. Integer-stepped columns (retrig
;; count, duration in steps, enum params) take one digit and commit. Everything
;; else is two hex digits, like Vol: the first digit waits in `entry`, the
;; second commits. a–f are hex here, not piano keys.

;; The hex digit a key types (0-15), or -1.
(def hex-digit (key)
  (index-of hex-keys (string-downcase key)))

(def two-digit-col? (col)
  (< (get col :increment) 1))

;; Two hex digits: the first waits in `entry`, the second calls commit with
;; the byte they spell.
(def type-hex (digit commit)
  (if (= tracker-view.entry "")
    (set! tracker-view.entry digit)
    (let ((n (+ (* 16 tracker-view.entry) digit)))
      (set! tracker-view.entry "")
      (commit n))))

;; A column's byte (00-FF) spans its range.
(def commit-hex (col n)
  (let ((lo (get col :min))
        (hi (get col :max)))
    (set-column col (+ lo (* (- hi lo) (/ n 255))))))

(def type-column (col digit)
  (if (two-digit-col? col)
    (type-hex digit |n| (commit-hex col n))
    ;; The digit is the value itself (clamped to the column's range).
    (when (< digit 10) (set-column col digit))))

;; Vol is the MIDI range, 00-7F (more saturates at full velocity).
(def type-cursor (key)
  (let ((digit (hex-digit key))
        (col (cursor-column)))
    (if (< digit 0) false
      (if (= tracker-cursor.col 1)
        (let ((s (cursor-step)))
          (type-hex digit |n| (set! s.velocity (min 1 (/ n 127))))
          true)
        (if col (do (type-column col digit) true) false)))))

(def nudge-column (col delta)
  (let ((cur (column-value (cursor-track) col tracker-cursor.row))
        (base (if (= cur nil) (column-default col) cur))
        (inc (get col :increment))
        (step (if (>= inc 1) inc (/ (- (get col :max) (get col :min)) 32))))
    (set-column col (+ base (* delta step)))))

;; ── cursor ──────────────────────────────────────────────────────────────────

(def wrap-index (value count)
  (- value (* count (floor (/ value count)))))

;; Sub-columns of a track: 0 = Note, 1 = Vol, 2.. = its columns.
(def column-count (index)
  (let ((t (track-at index)))
    (if t (+ 2 (len (track-columns t))) 2)))

(def clamped-row (row)
  (clamp row 0 (- (pattern-rows) 1)))

(def clamped-col (index col)
  (clamp col 0 (- (column-count index) 1)))

;; Puts the cursor on a cell of the grid (clamped to the tracks, the rows and
;; the track's sub-columns), the host's current track on its track, and
;; drops a pending hex digit.
(def set-cursor (index row col)
  (let ((i (clamped-index index))
        (t (track-at i)))
    (when t (focus-track t))
    (unless (= tracker-view.entry "") (set! tracker-view.entry ""))
    (set! tracker-cursor.track i)
    (set! tracker-cursor.row (clamped-row row))
    (set! tracker-cursor.col (clamped-col i col))))

;; Keeps the cursor on the grid when a track, a row or a column went away
;; under it, so its lamp always shows the cell a key edits.
(def clamp-cursor! ()
  (let ((i (cursor-index))
        (row (clamped-row tracker-cursor.row))
        (col (clamped-col i tracker-cursor.col)))
    (unless (= i tracker-cursor.track) (set! tracker-cursor.track i))
    (unless (= row tracker-cursor.row) (set! tracker-cursor.row row))
    (unless (= col tracker-cursor.col) (set! tracker-cursor.col col))))

(def automation-col? ()
  (>= tracker-cursor.col 2))

(def move-row (delta)
  (set-cursor (cursor-index) (wrap-index (+ tracker-cursor.row delta) (pattern-rows))
              tracker-cursor.col))

(def select-track (index)
  (set-cursor index tracker-cursor.row tracker-cursor.col))

;; Mouse: a click puts the cursor on that sub-cell; a double-click on a Note
;; cell toggles the step (writing through a ghost row to its real step).
(def select-cell (index row col)
  (set-cursor index row col))

(def double-click-cell (index row col)
  (select-cell index row col)
  (when (= col 0) (toggle-cursor-step)))

;; LEFT/RIGHT walk the sub-columns and spill into the neighbouring track at
;; either edge, like Renoise.
(def move-col (delta)
  (let ((next (+ tracker-cursor.col delta))
        (count (max 1 (track-count)))
        (index (cursor-index)))
    (if (< next 0)
      (let ((prev (wrap-index (- index 1) count)))
        (set-cursor prev tracker-cursor.row (- (column-count prev) 1)))
    (if (>= next (column-count index))
      (set-cursor (wrap-index (+ index 1) count) tracker-cursor.row 0)
      (set-cursor index tracker-cursor.row next)))))

;; -/= nudge whatever the cursor sits on: transpose, velocity or a column.
(def nudge-cursor (delta)
  (if (= tracker-cursor.col 0) (nudge-transpose delta)
  (if (= tracker-cursor.col 1) (nudge-velocity (* 0.1 delta))
    (let ((col (cursor-column)))
      (when col (nudge-column col delta))))))

(def clear-cursor ()
  (if (automation-col?)
    (let ((col (cursor-column)))
      (when col (clear-column col)))
    (clear-cursor-step)))

;; Same piano layout as the host's musical typing (input.rs note_from_key):
;; a key's position is its semitone above the octave's C.
(def piano-keys
  (list "a" "w" "s" "e" "d" "f" "t" "g" "y" "h" "u" "j" "k" "o" "l" "p"))

(def note-for-key (key)
  (index-of piano-keys key))

;; The cursor is clamped first: a key edits the cell its lamp shows.
(def handle-key (key text)
  (when (> (track-count) 0)
    (clamp-cursor!)
    (when (and (not (= tracker-view.entry "")) (< (hex-digit key) 0))
      (set! tracker-view.entry ""))
    (dispatch-key key)))

(def dispatch-key (key)
  (if (= key "UP") (do (move-row -1) true)
  (if (= key "DOWN") (do (move-row 1) true)
  (if (= key "LEFT") (do (move-col -1) true)
  (if (= key "RIGHT") (do (move-col 1) true)
  (if (= key "RET") (do (toggle-cursor-step) true)
  (if (or (= key "BS") (= key "Delete")) (do (clear-cursor) true)
  (if (= key "z") (do (set! tracker-view.octave (max 0 (- tracker-view.octave 1))) true)
  (if (= key "x") (do (set! tracker-view.octave (min 8 (+ tracker-view.octave 1))) true)
  (if (= key "-") (do (nudge-cursor -1) true)
  (if (= key "=") (do (nudge-cursor 1) true)
  (if (= key ",") (do (nudge-velocity -0.1) true)
  (if (= key ".") (do (nudge-velocity 0.1) true)
  (if (and (>= tracker-cursor.col 1) (type-cursor key)) true
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
;; scrolling body: the row numbers, then the rows, every track's cells in
;; the same body rows, so all columns share the scroll. The view computes
;; each track's columns once (from has-locks and t.step-params-in-use, never
;; the steps) and hands them down. Each row is a subtree holding one subtree
;; per track, so a step edit re-renders that track's cells on the rows
;; showing the step, and a lock or lane edit that track's cells. The
;; playhead and a cursor move within a track re-render nothing: a row's
;; lamp lights where track.playhead-row is the row, a cell's where the
;; cursor (tracker-cursor) is the cell, compared in their shaders. A move
;; to another track re-renders the two track headers (t.selected); the row
;; numbers and the scroll follow the current track through a binding.
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

;; A column is at least auto-w wide and grows to fit its header, so headers
;; are never truncated. Headers use the compact spelling (:short, e.g.
;; voicing.character → vcn.chr); lanes bring their own short label. The
;; picker keeps full names.
(def column-title (col)
  (or (get col :short) (get col :label)))

(def column-w (col)
  (max auto-w (+ 0.6 (* 0.62 (len (column-title col))))))

(def columns-w (cols)
  (reduce |acc col| (+ acc (column-w col) sub-gap) 0 cols))

;; Width of a track's cells; the header adds head-extra for its buttons and
;; rows match it so the two stay aligned.
(def col-w (cols)
  (+ note-w vol-w sub-gap (columns-w cols)))
(def track-w (cols)
  (+ (col-w cols) head-extra))

(def col-panel-border (rgba 1.0 1.0 1.0 0.08))
(def vol-color (rgba 0.95 0.78 0.35 1.0))
(def auto-color (rgba 0.55 0.80 1.0 1.0))
(def empty-color :dimmer)

(def beat-row? (row) (= (wrap-index row 4) 0))
(def bar-row? (row) (= (wrap-index row 16) 0))

;; A cell's fill while the cursor is on it (sub-column cell-col of row
;; cell-row of track position cell-track; the cursor-* states bind the
;; tracker-cursor fields). The lamps draw inside a pixel
;; with a one-pixel corner, as a box's 2-pixel corner background does
;; inside its border.
(defwidget tracker-cursor-lamp
  :width 1 :height 1
  :state (cell-track cell-row cell-col cursor-track cursor-row cursor-col)
  :shader
  (if (< (+ (abs (- cursor-track cell-track))
            (abs (- cursor-row cell-row))
            (abs (- cursor-col cell-col)))
         0.5)
    (let ((px (fwidth y)))
      (sdf/layer (sdf/fill (sdf/rounded-rect (- width px) (- height px) px) :primary)))
    (rgba 0 0 0 0)))

;; A track's row lit while the playhead plays it (which repeat of a shorter
;; track: track.playhead-row). `base` is the alpha of the row's own tint
;; under it, which the lamp tops up to the playhead's. The lamps return
;; their white wash as is, its rounded corners as coverage: an sdf/fill's
;; output is premultiplied, and the widget blend multiplies a translucent
;; fill by its alpha again (a 0.16 wash would show as 0.03).
(defwidget tracker-row-lamp
  :width 1 :height 1
  :state (track row base)
  :shader
  (let ((lit (if (= track.playhead-row row) 0.2 0.0))
        (px (fwidth y))
        (d (sdf/rounded-rect (- width px) (- height px) px))
        (cover (clamp (- 0.5 (/ d (max (fwidth d) 0.001))) 0.0 1.0)))
    (rgba 1 1 1 (* cover (max 0.0 (/ (- lit base) (- 1 base)))))))

;; A row number lit on the cursor's row and, a little brighter, on the row
;; the current track plays (`playhead-row` binds selection.playhead-row, -1
;; while stopped or without a current track).
(defwidget tracker-gutter-lamp
  :width 1 :height 1
  :state (row base playhead-row cursor-row)
  :shader
  (let ((lit (max (if (= cursor-row row) 0.16 0.0)
                  (if (= playhead-row row) 0.2 0.0)))
        (px (fwidth y))
        (d (sdf/rounded-rect (- width px) (- height px) px))
        (cover (clamp (- 0.5 (/ d (max (fwidth d) 0.001))) 0.0 1.0)))
    (rgba 1 1 1 (* cover (max 0.0 (/ (- lit base) (- 1 base)))))))

;; ── collapsing ─────────────────────────────────────────────────────────────
;;
;; Wide projects scroll sideways through the tile's own smooth horizontal
;; widget scroll (the body's vertical `scroll` declines sideways swipes), so
;; nothing here re-renders on a pan. A track's « header button folds its
;; columns away when it is not the one being edited.

(def collapsed? (t)
  (listed? t.tid (or tracker-view.collapsed (list))))

(def toggle-collapse (t)
  (set! tracker-view.collapsed
    (if (collapsed? t)
      (without tracker-view.collapsed t.tid)
      (append (or tracker-view.collapsed (list)) (list t.tid)))))

;; A shown column's header spelling (only those: the picker lists every
;; param a track has).
(def with-short (col)
  (if (get col :short) col (merge col :short (compact-label (get col :label)))))

;; What the view draws of a track: the track, the columns it draws (none
;; while collapsed, each with its header spelling) and how many a collapse
;; folds away.
(def track-layout (t)
  (let ((cols (track-columns t))
        (folded (collapsed? t)))
    (dict :track t
          :cols (if folded (list) (map with-short cols))
          :folded (if folded (len cols) 0))))

;; ── cells ───────────────────────────────────────────────────────────────────

;; The alpha of a row's own tint: ghost rows sit on a wash of the track's own
;; color, a little stronger on the row where the loop restarts, so each
;; track's period reads at a glance; bars and beats are white.
(def row-alpha (t row)
  (if (loop-start-row? t row) 0.16
  (if (ghost-row? t row) 0.06
  (if (bar-row? row) 0.10
  (if (beat-row? row) 0.045
    0)))))

(def row-bg (t row)
  (let ((alpha (row-alpha t row)))
    (if (ghost-row? t row) (color-rgba t.color alpha)
      (if (> alpha 0) (rgba 1.0 1.0 1.0 alpha) :transparent))))

(def note-text (s)
  (if (step-active? s) (note-name s.transpose) "---"))

(def vol-text (s)
  (if (step-active? s) (velocity-hex s.velocity) ".."))

(def sub-cell (text t row col width color active?)
  (box
    :key (str "tracker-cell-" t.index "-" row "-" col)
    :width width
    :height row-h
    :corner-radius 2
    :background-color :transparent
    :background "tracker-cursor-lamp" :cell-track t.index :cell-row row :cell-col col
    :cursor-track #'tracker-cursor.track :cursor-row #'tracker-cursor.row
    :cursor-col #'tracker-cursor.col
    :on-click (lambda (event) (select-cell t.index row col))
    :on-double-click (lambda (event) (double-click-cell t.index row col))
    (label text
      :width width
      :height row-h
      :font-size cell-font
      :mono true
      :h-align :center
      :color (if active? (if (ghost-row? t row) :dim color) empty-color)
      :bg :transparent)))

(def column-cell (t row col idx)
  (let ((v (column-value t col row))
        ;; Lanes and step params are dense, so a value still at its default
        ;; draws dim rather than bright.
        (lit? (and (not (= v nil))
                   (not (and (dense-col? col) (= v (get col :default)))))))
    (sub-cell (format-column col v) t row (+ 2 idx) (column-w col) auto-color lit?)))

;; One track's cells on one grid row.
(def track-row (layout row)
  (let ((t (get layout :track))
        (cols (get layout :cols))
        (s (row-step t row))
        (active? (step-active? s)))
    (box
      :key (str "tracker-row-" t.index "-" row)
      :width (track-w cols)
      :height row-h
      :corner-radius 2
      :background-color (row-bg t row)
      :background "tracker-row-lamp" :track t :row row :base (row-alpha t row)
      (h-stack :gap sub-gap
        (sub-cell (note-text s) t row 0 note-w :fg active?)
        (sub-cell (vol-text s) t row 1 vol-w vol-color active?)
        (each (range 0 (len cols)) |idx|
          (column-cell t row (nth cols idx) idx))))))

;; A row number; its lamp follows the cursor and the current track's
;; playhead, both bound (a cross-track move re-renders no row number).
(def row-number (row)
  (let ((base (if (bar-row? row) 0.10 0)))
    (box
      :key (str "tracker-gutter-" row)
      :width gutter-w
      :height row-h
      :corner-radius 2
      :background-color (if (bar-row? row) (rgba 1.0 1.0 1.0 0.10) :transparent)
      :background "tracker-gutter-lamp" :row row :base base
      :playhead-row #'selection.playhead-row :cursor-row #'tracker-cursor.row
      (label (if (< row 10) (str "0" row) (str row))
        :key (str "tracker-row-" row)
        :width gutter-w
        :height row-h
        :font-size cell-font
        :mono true
        :h-align :right
        :color (if (beat-row? row) :fg :dim)
        :bg :transparent))))

;; One grid row across every track, each track's cells a subtree of their
;; own (keyed as the row's box: a subtree's key replaces its root's).
(def grid-row (layouts row)
  (h-stack :gap track-gap
    (each layouts |layout|
      (let ((t (get layout :track)))
        (subtree :key (str "tracker-row-" t.index "-" row)
          (track-row layout row))))))

;; ── headers ─────────────────────────────────────────────────────────────────

(def sub-header (text width key)
  (label text
    :key key
    :width width :height 0.8 :font-size sub-font :mono true :h-align :center
    :color :dim :bg :transparent))

(def open-column-menu (t event)
  (set! tracker-menu.track t)
  (open-menu! tracker-menu event))

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

(def column-header (layout)
  (let ((t (get layout :track))
        (cols (get layout :cols))
        (width (track-w cols))
        (current? t.selected))
    (box
      :key (str "tracker-panel-" t.index)
      :width width
      :corner-radius 4
      :border-width 1
      :border-color (if current? (rgba 1.0 1.0 1.0 0.22) col-panel-border)
      (v-stack :gap 0.15
        (box
          :key (str "tracker-strip-" t.index)
          :width width
          :height 0.4
          :corner-radius 2
          :background-color (color-rgba t.color 1.0))
        (label (fit-name t.name width)
          :key (str "tracker-head-" t.index)
          :width width
          :height 1.1
          :font-size head-font
          :h-align :center
          :color (if current? :white :fg)
          :on-click (lambda (event) (select-track t.index))
          :bg :transparent)
        (h-stack :gap sub-gap
          (sub-header "Note" note-w (str "tracker-sub-note-" t.index))
          (sub-header "Vol" vol-w (str "tracker-sub-vol-" t.index))
          (each (range 0 (len cols)) |idx|
            (sub-header (column-title (nth cols idx))
                        (column-w (nth cols idx)) (str "tracker-sub-col-" t.index "-" idx)))
          ;; « collapses the columns (badge shows how many are folded); +
          ;; opens the column picker.
          (header-button (if (collapsed? t) (str (get layout :folded)) "«")
                         (str "tracker-collapse-" t.index)
                         (lambda (event) (toggle-collapse t)))
          (header-button "+" (str "tracker-add-col-" t.index)
                         (lambda (event) (open-column-menu t event))))))))

;; Full width of the gutter and every track panel; the body scroll is sized
;; to this rather than the viewport so rows past the right edge still lay
;; out and the tile's horizontal scroll can reach them.
(def grid-width (layouts)
  (reduce |acc layout| (+ acc (track-w (get layout :cols)) track-gap)
          (+ gutter-w track-gap)
          layouts))

(def header-row (layouts)
  (h-stack :gap track-gap :key "tracker-headers"
    (box :key "tracker-head-rows" :width gutter-w :height 1)
    (each layouts |layout|
      (let ((t (get layout :track)))
        (subtree :key (str "tracker-h-" t.index)
          (column-header layout))))))

(def chip (text key)
  (box :key key :height 1.1 :padding-left 0.4 :padding-right 0.4 :corner-radius 3
       :background-color (rgba 1.0 1.0 1.0 0.07)
    (label text :height 1.1 :v-align :center :font-size 9.5 :mono true :color :fg :bg :transparent)))

;; A pending first hex digit shows here rather than in the cell, so typing
;; re-renders this one chip and not the grid.
(def entry-chip ()
  (if (= tracker-view.entry "") (box :key "tracker-chip-entry" :width 0 :height 1.1)
    (chip (str (nth hex-digits tracker-view.entry) "_") "tracker-chip-entry")))

(def toolbar (rows)
  (h-stack :gap 0.5 :v-align :center
    (subtree :key "tracker-entry" (entry-chip))
    (chip (str "OCT " tracker-view.octave) "tracker-chip-oct")
    (chip (str "STEP " tracker-view.step-advance) "tracker-chip-step")
    (chip (str "ROWS " rows) "tracker-chip-rows")
    (label "a-p notes · z/x octave · RET toggle · BS clear · -/= nudge · ,/. vol"
      :key "tracker-status" :font-size 9 :color :dim :bg :transparent)))

;; The scrolling body. Follow: the view keeps the cursor row centered while
;; editing and the current track's playhead row while playing, except that
;; the first half-screen of rows stays put (the scroll clamps at the top).
;; Both are bound (the current track's through selection.playhead-row, so
;; the body never reads selection.track), so the cursor, ticks and a move
;; to another track move the view without a re-render; only play and stop
;; re-run it. The rows are a plain stack, not a virtualizing one: a pattern
;; is at most 256 rows, and a fixed row set means scrolling and follow are
;; pure render-time offsets with no relayout.
(def body (layouts rows)
  (let ((width (grid-width layouts)))
    (v-stack :flex 1 :width :fill
      (scroll :key "tracker-scroll" :width width :flex 1
        :center-row (if transport.playing #'selection.playhead-row #'tracker-cursor.row)
        :center-span row-h
        (h-stack :gap track-gap
          (v-stack :key "tracker-gutter" :width gutter-w :gap 0
            (each (range 0 rows) |row| (row-number row)))
          (v-stack :key "tracker-rows" :width (- width gutter-w track-gap) :gap 0
            (each (range 0 rows) |row|
              (subtree :key (str "tracker-r-" row)
                (grid-row layouts row)))))))))

;; ── column picker ───────────────────────────────────────────────────────────
;;
;; A nested context menu: one submenu per device (Step, the instrument, each
;; FX and MIDI FX slot, Macros) plus Lanes for the track's process lanes.
;; Shown columns are checked; selecting toggles them.

(def picker-item (t col prefix shown)
  (let ((key (get col :key)))
    (menu-item (get col :label)
      :key (str prefix key)
      :checked (listed? key shown)
      :on-select (lambda (event) (toggle-column t key)))))

(def picker-group (t label items prefix shown)
  (menu-item label :key (str prefix "group")
    (each items |col| (picker-item t col prefix shown))))

(def picker-groups (t)
  (let ((prefix (str "tracker-pick-" t.index "-"))
        (shown (map |col| (get col :key) (track-columns t))))
    (append
      (map |g| (picker-group t (get g :group) (get g :items) (str prefix (get g :group) "-") shown)
           (target-groups t))
      (if (empty? t.lanes) (list)
        (list (picker-group t "Lanes" (map lane-column t.lanes) (str prefix "lanes-") shown))))))

;; Its items are built only while it is open. The picker's track may have
;; gone (a project load, an undo) while it was open: it then lists nothing.
(def column-picker ()
  (let ((t tracker-menu.track))
    (apply menu-of tracker-menu
      (if (and tracker-menu.open t (listed? t (tracks))) (picker-groups t) (list)))))

(def tracker-root ()
  (let ((layouts (map track-layout (tracks)))
        (rows (pattern-rows)))
    (v-stack :padding 0.6 :gap 0.5 :width :fill :height :fill
      (subtree :key "tracker-toolbar" (toolbar rows))
      (subtree :key "tracker-header-row" (header-row layouts))
      (subtree :key "tracker-body" (body layouts rows))
      (subtree :key "tracker-picker" (column-picker)))))

(effect-buffer "*tracker*"
  (tracker-root))

(set-buffer-mode-for "*tracker*" "alez.tracker.ui/tracker-mode")

;; ── install ─────────────────────────────────────────────────────────────────

;; The tab's position after Seq (0 when it is not registered).
(def tab-index ()
  (let ((buffers (map |tab| (nth tab 1) (eseq.seq-step-tabs/seq-main-step-tabs))))
    (+ (index-of buffers buffer-name) 1)))

(def show ()
  (eseq.seq-step-tabs/seq-register-step-sequencer-tab tab-label buffer-name)
  (eseq.seq-step-tabs/seq-select-main-step-tab-by-index (tab-index))
  (set-cursor tracker-cursor.track tracker-cursor.row tracker-cursor.col))

(def hide ()
  (eseq.seq-step-tabs/seq-unregister-step-sequencer-tab buffer-name))

(bind-key "C-c t" "alez.tracker.ui/show")

;; Importing the module is the install: the tab appears immediately.
(show)
