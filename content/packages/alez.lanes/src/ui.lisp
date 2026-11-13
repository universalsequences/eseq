;; alez.lanes — the *sequencer* tile as flat lanes of steps, one per track.
;;
;;   (import alez.lanes.ui)          ; in *scratch* (C-x C-e) or init.lisp
;;
;; The package replaces what the Seq tile draws with an override of
;; eseq.sequencer/sequencer-body; content/ui/sequencer.lisp is untouched and
;; comes back when the package is detached (or with M-x alez.lanes.ui/off).
;;
;; Layout: a ruler, then one row per track: a name plate holding record
;; arm, the name, the track's number (its mute) and solo, then the
;; pattern's Steps, Timebase (with steps selected it p-locks them, and
;; shows a selected step's lock) and Swing, then the pattern's steps,
;; sixteen a row. Groups and drum racks keep a header row; their members sit
;; under it behind the group's color bar. The tabs under the grid pick the
;; step parameter the grid edits.
;;
;; There is no expanded step editor: a step always shows its velocity (how
;; far the bright fill rises), plus a mark for the tab's parameter.
;;
;;   Velocity   the fill itself
;;   Delay      the lit square of eight
;;   Duration   a bar (longer than a step tints the steps it holds)
;;   Repeat     one tick per retrig
;;   Pitch      a dash above or below the middle (the step's transpose)
;;   Sync       the name of the grid the step waits for ("1/4", "1 bar")
;;              beside a small watch whose face is one bar: the wedge is
;;              the grid's share of it, rings count extra bars, and a hand
;;              goes round with the transport; the companion of a timebase
;;              p-lock
;;
;; Pointer: exactly the factory grid's gestures (eseq.step-grid-interactions:
;; click empty = on, drag paints; click on = select; drag = move; hold+drag
;; = sweep-select; shift/cmd = range/add; double-click = off), plus one:
;; drag a step UP or DOWN to dial the tab's parameter. The first motion
;; decides: sideways out of the step is the factory gesture, vertical is the
;; dial, which hides the pointer and holds it on the step until release
;; (the mixer fader's way). Dialing a selected step writes the whole
;; selection; cmd-dragging one of several selected steps ramps the others
;; from the first's value to the dialed one and on to the last's.

(module alez.lanes.ui)

(import eseq.sequencer :refer (grid-cursor))
(import eseq.seq-core-state)
(import eseq.step-grid-interactions :as sgi)
(import eseq.effects.param-controls :as pc)
(import eseq.drum-rack-v2)
(import eseq.track-collapse)
(import eseq.export-song)
(import eseq.file-dialogs)
(import eseq.sample-import)
(import eseq.retrospective)
(import eseq.resample)
(import eseq.sample-region)
(import eseq.factory-promote)
(import eseq.view-kit :refer (nothing color-rgba))
(import eseq.kinds :refer (selection transport timebase-options take-governed))

(export on
        off
        lanes-view
        lanes-readout
        set-readout!
        set-mode
        params
        cell-down
        cell-drag
        cell-up
        dial-value)

;; ── knobs ───────────────────────────────────────────────────────────────────

(defcustom step-width 4
  :type :number :min 2 :max 8 :step 0.25
  :doc "Width (cells) of one step in the lane grid.")

(defcustom step-height 1.3
  :type :number :min 0.8 :max 3 :step 0.05
  :doc "Height (cells) of one step, and so of a track row.")

(defcustom fit-steps true
  :type :bool
  :doc "Stretch (or squeeze) a row of sixteen steps to the tile's width, so the grid never scrolls sideways and the track headers stay put. Off: steps are step-width wide and a narrow tile pans, headers and all.")

(defcustom dial-rows 4
  :type :number :min 1 :max 12 :step 0.5
  :doc "How far (cells) a vertical drag on a step travels to sweep a 0..1 parameter's whole range.")

;; ── view state ──────────────────────────────────────────────────────────────

;; `mode`: the tab (an index into `params`); a row reads it, so a tab change
;; re-renders the rows (a step binds the tab's field as its `value`: the
;; shader's state budget has no room for every field).
(def-kind lanes-view
  :key ()
  :state ((mode 0)))

;; One step's readout, keyed by the step (so it goes when the step does): the value a dial is writing,
;; shown in the step itself, "" otherwise; `editing` (1 while it shows)
;; hides the step's mark under the text. Keyed so a dial re-renders the
;; label of the step it started on and nothing else.
(def-kind lanes-readout
  :key (step)
  :state ((text :any :default "")
          (editing 0)))

(def readout-of (s) (lanes-readout s))

(def set-readout! (s text)
  (let ((r (readout-of s)))
    (set! r.text text)
    (set! r.editing (if (= text "") 0 1))))

;; The tabs. `:per-row` is what one cell of vertical drag adds; the 0..1
;; params take theirs from `dial-rows` instead (see `dial-per-row`).
(def params
  (list (dict :name "Velocity" :key :velocity :min 0 :max 1 :quantum 0.01 :unit-range true)
        (dict :name "Delay" :key :delay :min 0 :max 1 :quantum 0.01 :unit-range true)
        (dict :name "Duration" :key :duration :min 0.05 :max 32 :quantum 0.05 :per-row 0.5)
        (dict :name "Repeat" :key :retrig :min 0 :max 16 :quantum 1 :per-row 2)
        (dict :name "Pitch" :key :transpose :min -48 :max 48 :quantum 1 :per-row 3)
        (dict :name "Sync" :key :sync :min 0 :max 7 :quantum 1 :per-row 1)))

(def param () (nth params lanes-view.mode))

(def set-mode (i) (set! lanes-view.mode i))

(def step-value (s)
  (match lanes-view.mode
    0 s.velocity
    1 s.delay
    2 s.duration
    3 s.retrig
    4 s.transpose
    _ s.sync))

;; As the step kind's own setters write a field, by the tab's key.
(def set-step-value! (s v)
  (seq-set-track-step-param s.track.index s.index (get (param) :key) v))

(def dial-per-row (p)
  (if (get p :unit-range)
    (/ (- (get p :max) (get p :min)) (max 0.5 dial-rows))
    (get p :per-row)))

;; A sync value's name as a step shows it, short enough for a narrow step
;; (by value, as project.sync-options; index 4 is half a bar).
(def sync-names (list "Off" "1/16" "1/8" "1/4" "1/2" "1 bar" "2 bar" "4 bar"))
(def sync-name (v) (or (nth sync-names (round v)) "Off"))

(def signed (n) (str (if (> n 0) "+" "") n))

(def readout-text (p v)
  (match lanes-view.mode
    0 (str (round (* 100 v)) "%")
    1 (str (round (* 100 v)) "%")
    2 (str (/ (round (* 100 v)) 100) " st")
    3 (str "x" (+ 1 (round v)))
    4 (str (signed (round v)) " st")
    _ (sync-name v)))

;; ── selection ───────────────────────────────────────────────────────────────

(def governed? (t) (= t.governed take-governed))

(def select-track! (t) (eseq.sequencer/select-track-for-edit t))

(def select-group! (g)
  (when g.bus (set! eseq.seq-core-state/selected-bus g.bus.index)))

;; ── step gestures ───────────────────────────────────────────────────────────

;; One gesture at a time: the track it started on, where it pressed
;; (`press`: the step, the pointer's grid point, whether the step was part of
;; the selection, and once dialing the value it started from and last wrote),
;; and whether it turned into a dial.
(def gesture-track nil)
(def press nil)
(def dialing false)

;; Vertical travel (cells) inside the pressed step that turns a press into a
;; dial. A cell is about twice as tall as wide, hence the 2 below.
(def dial-threshold 0.28)

;; The selected steps of t, in step order.
(def selected-steps (t) (filter (lambda (x) x.selected) t.steps))

;; A cmd-press on one of several selected steps may become a ramp (below),
;; so the factory's cmd-click (take the step out of the selection) waits for
;; the release, or for the pointer to reach another step.
(def ramp-press? (s evt in-selection)
  (and in-selection (sgi/cmd-click? evt) (> (len (selected-steps s.track)) 1)))

(def cell-down (s evt)
  (let ((t s.track))
    (unless (governed? t)
      ;; Read before selecting: a selection applies on the current track only.
      (let ((use-selection t.selected)
            (in-selection (if (and t.selected s.selected) true false)))
        (set! eseq.seq-core-state/selected-bus -1)
        (set! selection.track t)
        (set! gesture-track t)
        (set! dialing false)
        (set! press (dict :step s :row evt.row :col evt.col :selected in-selection))
        (if (ramp-press? s evt in-selection)
          (set! press (merge press :ramp-pending true :down evt))
          (sgi/step-pointer-down-for-track t.index s.index evt use-selection))))))

;; The press has travelled far enough up or down to be a dial.
(def dial-travel? (evt)
  (let ((dr (abs (- evt.row (get press :row))))
        (dc (abs (- evt.col (get press :col)))))
    (and (> dr dial-threshold) (> (* 2 dr) dc))))

(def dial-start? (s evt)
  (and press
       (= s (get press :step))
       (not (sgi/selection-click? evt))
       (not eseq.vanilla/step-hold-select)
       (not eseq.vanilla/step-drag-progressed)
       (dial-travel? evt)))

;; `raw` snapped to the param's quantum and range.
(def dial-value (p raw)
  (let ((q (get p :quantum)))
    (max (get p :min) (min (get p :max) (* q (round (/ raw q)))))))

;; A ramp (cmd-drag on one of several selected steps, the Cirklon's ganged
;; steps): the dialled step takes v, and the selected steps either side run
;; in a line, by step position, from the first selected step's value up to
;; v and from v on to the last's. Dialling the last (or first) selected step
;; is then one straight ramp across the selection.
(def ramp-value (p x v)
  (let ((g (get press :step))
        (lo (get press :lo))
        (hi (get press :hi)))
    (dial-value p
      (if (< x.index g.index)
        (+ (get press :lo-value)
           (* (- v (get press :lo-value)) (/ (- x.index lo.index) (- g.index lo.index))))
        (if (> x.index g.index)
          (+ v (* (- (get press :hi-value) v) (/ (- x.index g.index) (- hi.index g.index))))
          v)))))

;; Writes only the steps whose value moved (`:written`: what each holds).
(def ramp-write (p v)
  (let ((steps (get press :ramp))
        (written (get press :written))
        (values (map (lambda (x) (ramp-value p x v)) steps)))
    (for-each
      (lambda (i)
        (unless (= (nth values i) (nth written i))
          (set-step-value! (nth steps i) (nth values i))))
      (range 0 (len steps)))
    (set! press (merge press :written values))))

(def dial-write (p v)
  (unless (= v (get press :last))
    (set! press (merge press :last v))
    (if (get press :ramp)
      (ramp-write p v)
      (if (get press :selected)
        (seq-set-step-param-plock (get p :key) v)
        (set-step-value! (get press :step) v)))
    (set-readout! (get press :step) (readout-text p v))))

;; A pointer jump (cells) no hand makes between two drag events: the hidden
;; pointer taking over. Such an event only re-bases the dial.
(def dial-jump 2.5)

;; The dial follows the pointer's travel event by event (`:raw` is the
;; unsnapped value, kept inside the param's range so a reversal answers at
;; once).
(def dial-to (evt)
  (when press
    (let ((p (param))
          (travel (- (get press :row) evt.row))
          (raw (if (> (abs travel) dial-jump)
                 (get press :raw)
                 (max (get p :min)
                   (min (get p :max) (+ (get press :raw) (* travel (dial-per-row p))))))))
      (set! press (merge press :row evt.row :raw raw))
      (dial-write p (dial-value p raw)))))

;; The press turns into a dial: no longer a click, a move or a paint. The
;; pointer hides and stays with the step until release (grab-pointer), so
;; the dial runs however far the hand travels and whatever it passes over.
(def begin-dial (evt)
  (let ((s (get press :step)))
    (sgi/step-clear-drag-state)
    (when (and (not (get press :selected)) (seq-has-selection?))
      (seq-clear-selection))
    (set! press (merge press :raw (step-value s) :last nil))
    (set! dialing true)
    (grab-pointer true)
    (dial-to evt)))

;; The waiting cmd-press turns into a ramp over the selection as it stands.
(def begin-ramp (evt)
  (let ((s (get press :step))
        (steps (selected-steps s.track))
        (lo (first steps))
        (hi (nth steps (- (len steps) 1))))
    (set! press (merge press
                  :ramp-pending false :ramp steps :written (map step-value steps)
                  :lo lo :hi hi
                  :lo-value (step-value lo) :hi-value (step-value hi)
                  :raw (step-value s) :last nil))
    (set! dialing true)
    (grab-pointer true)
    (dial-to evt)))

;; The waiting cmd-press was a cmd-click after all: the factory's, late.
(def release-ramp-press ()
  (let ((s (get press :step)))
    (set! press (merge press :ramp-pending false))
    (sgi/step-pointer-down-for-track s.track.index s.index (get press :down) true)))

(def cell-drag (s evt)
  (if dialing
    (dial-to evt)
    (let ((t s.track))
      (when (and (= gesture-track t) (not (governed? t)))
        (if (and press (get press :ramp-pending))
          (if (= s (get press :step))
            (when (dial-travel? evt) (begin-ramp evt))
            ;; Onto another step: a cmd-drag over the steps, as ever.
            (do (release-ramp-press)
                (sgi/step-select-drag-over-for-track t.index s.index evt)))
          (if (dial-start? s evt)
            (begin-dial evt)
            (do (set! selection.track t)
                (sgi/step-select-drag-over-for-track t.index s.index evt))))))))

(def end-gesture ()
  (when press (set-readout! (get press :step) ""))
  (set! gesture-track nil)
  (set! press nil)
  (set! dialing false))

(def cell-up (s evt)
  (let ((t s.track))
    (if dialing
      ;; Resets the factory gesture state; no step is pending, so no click.
      (sgi/step-pointer-up -1 evt)
      (when (and (= gesture-track t) (not (governed? t)))
        (set! selection.track t)
        (when (and press (get press :ramp-pending)) (release-ramp-press))
        (sgi/step-pointer-up s.index evt)))
    (end-gesture)))

(def cell-double-click (s evt)
  (let ((t s.track))
    (unless (governed? t)
      (set! selection.track t)
      (sgi/step-double-click-for-track t.index s.index evt))))

;; ── the step ────────────────────────────────────────────────────────────────

;; One step. Off: a gray cell (`shade`, alternating by beat), lighter under
;; the playhead and tinted while an earlier step's length holds it. On: one
;; orange for every track, bright up to the step's velocity and darker above it,
;; with the mark of the tab's parameter (`mode`) in ink. `selected` pales
;; the fill and rims it. A p-locked step (on or off)
;; carries a small tick at its top in its variant's color. `value` is the tab's field of
;; the step (unused on the Velocity tab).
(defwidget lanes-step
  :width 4 :height 1.3
  :paint-margin 0.1
  :state (step track index selected mode value editing pos)
  :shader
  (let ((on (if (= step.active 1) 1.0 0.0))
        ;; One step color for every track (the track's color stays on its
        ;; name plate), gray while the track is not heard.
        (tc (mix (vec3 0.46 0.46 0.48) (vec3 0.93 0.60 0.29) track.audible))
        ;; Off-cell gray by beat, flipped every two bars.
        (beat (floor (/ index 4.0)))
        (phrase (floor (/ index 32.0)))
        (odd (- (+ beat phrase) (* 2.0 (floor (/ (+ beat phrase) 2.0)))))
        (shade (if (= odd 0.0) 0.32 0.24))
        (w (* width 0.955))
        (h (* height 0.9))
        (body (sdf/rounded-rect w h 0.09))
        (playing (if (= track.playhead index) 1.0 0.0))
        (held (- step.held-pad (* 2.0 (floor (/ step.held-pad 2.0)))))
        (base (+ shade (* 0.2 playing)))
        (off (mix (vec3 base base (* 1.03 base)) tc (* 0.3 held (- 1.0 on))))
        (lit (mix tc (vec3 1 1 1) (min 0.75 (+ (* 0.5 selected) (* 0.3 playing)))))
        (top (* 0.72 lit))
        (level (- h (* 2.0 h (clamp step.velocity 0.0 1.0))))
        (ink (rgba 0.03 0.03 0.04 1.0))
        (faint (rgba (* 0.62 lit) 1.0))
        (none (rgba 0 0 0 0))
        ;; 0 while the step's readout shows: the marks make way for the text.
        (marks (- 1.0 editing))
        ;; the mark strip along the bottom, and 0..1 across it
        (gw (* w 0.8))
        (gh (* h 0.17))
        (strip (sdf/translate 0.0 (* h 0.56) (sdf/rounded-rect gw gh 0.0)))
        (u (/ (+ x gw) (* 2.0 gw)))
        (slot (floor (* u 8.0)))
        (in-slot (let ((f (- (* u 8.0) slot)))
                   (* (if (> f 0.14) 1.0 0.0) (if (< f 0.86) 1.0 0.0))))
        (delay value)
        (dur value)
        (rep value)
        (xp value)
        (show-delay (* on marks (if (= mode 1) 1.0 0.0) (if (> delay 0.001) 1.0 0.0)))
        (show-duration (* on marks (if (= mode 2) 1.0 0.0) (if (> (abs (- dur 1.0)) 0.001) 1.0 0.0)))
        (show-repeat (* on marks (if (= mode 3) 1.0 0.0) (if (> rep 0.5) 1.0 0.0)))
        (show-pitch (* on marks (if (= mode 4) 1.0 0.0) (if (> (abs xp) 0.5) 1.0 0.0)))
        ;; Sync: a small watch at the step's left whose whole face is ONE
        ;; BAR. The dark wedge is the grid the step waits for as a share of
        ;; that bar (a quarter pie for 1/4, a full face for 1 bar); 2 and 4
        ;; bars keep the full face and gain one or two outer rings (laps).
        ;; While the transport runs a white hand goes round with it (`pos`,
        ;; the transport's sixteenth counter): once a bar, or once per grid
        ;; when the grid is longer. The step may fire each time the hand
        ;; crosses a multiple of the wedge; the hand and rim turn white on
        ;; the step the playhead is holding.
        (show-sync (* on marks (if (= mode 5) 1.0 0.0) (if (> value 0.5) 1.0 0.0)))
        (grid (pow 2.0 (- value 1.0)))
        (lap (max 16.0 grid))
        (watch-x (+ (- w) (* h 0.5)))
        (watch-y (* h 0.1))
        (watch-r (* h 0.26))
        (watch (sdf/translate watch-x watch-y (sdf/circle watch-r)))
        (ring-1 (sdf/translate watch-x watch-y (sdf/circle (* 1.3 watch-r))))
        (ring-2 (sdf/translate watch-x watch-y (sdf/circle (* 1.58 watch-r))))
        (turn (/ (mod (+ (atan2 (- x watch-x) (- watch-y y)) 6.2831853) 6.2831853) 6.2831853))
        (running (if (< track.playhead 0) 0.0 1.0))
        (hand-a (* 6.2831853 (/ (mod pos lap) lap)))
        (hand (- (sdf/line watch-x watch-y
                   (+ watch-x (* 0.82 watch-r (sin hand-a)))
                   (- watch-y (* 0.82 watch-r (cos hand-a))))
                 (* 0.07 watch-r)))
        (live (if (> playing 0.5) (rgba 1 1 1 1) ink))
        ;; The p-lock mark, as the factory step's: a small tick (here at the
        ;; top) in the step's variant color, gray for sequencer-only locks;
        ;; on off steps too.
        (lock step.lock-kind)
        (lock-col (if (= lock 2) (rgba step.variant-color 1.0) (rgba 0.545 0.545 0.588 1.0)))
        (tick (sdf/translate 0.0 (* -0.7 h) (sdf/rounded-rect (* w 0.13) (* h 0.075) 0.03))))
    (sdf/layer
      (sdf/fill body (rgba (mix off top on) 1.0))
      (sdf/fill (max body (- level y))
        (if (= on 1.0) (rgba lit 1.0) none))
      ;; Delay: eight squares, the delay's one in ink.
      (sdf/fill strip
        (if (> (* show-delay in-slot) 0.5)
          (if (= slot (floor (* (clamp delay 0.0 0.999) 8.0))) ink faint)
          none))
      ;; Duration: a bar filled to the duration (full for a step or longer).
      (sdf/fill strip
        (if (> show-duration 0.5)
          (if (< u (clamp dur 0.0 1.0)) ink faint)
          none))
      ;; Repeat: a tick per hit, eight at most.
      (sdf/fill strip
        (if (> (* show-repeat in-slot) 0.5)
          (if (< slot (min 8.0 (+ rep 1.0))) ink none)
          none))
      ;; Pitch: a dash above (up) or below (down) the middle, two octaves each way.
      (sdf/fill
        (sdf/translate 0.0 (* -0.66 h (clamp (/ xp 24.0) -1.0 1.0))
          (sdf/rounded-rect (* w 0.62) (* h 0.08) 0.0))
        (if (> show-pitch 0.5) ink none))
      (sdf/fill watch
        (if (> show-sync 0.5)
          (if (< turn (min 1.0 (/ grid 16.0))) ink faint)
          none))
      (sdf/stroke watch 0.035 (if (> show-sync 0.5) live none))
      (sdf/stroke ring-1 0.03 (if (> (* show-sync (if (> value 5.5) 1.0 0.0)) 0.5) live none))
      (sdf/stroke ring-2 0.03 (if (> (* show-sync (if (> value 6.5) 1.0 0.0)) 0.5) live none))
      (sdf/fill hand (if (> (* show-sync running) 0.5) (rgba 1 1 1 1) none))
      (sdf/fill tick (if (> lock 0.5) lock-col none))
      (sdf/stroke body 0.07
        (if (= selected 1) (rgba 1 1 1 0.95) none)))))

;; The step cursor (`cursor`, the grid's shared one, wrapped to the track's
;; length): a thin frame over step `index` of the current track. Its own
;; widget, over the step: the step's shader state has no room for it.
(defwidget lanes-cursor
  :width 4 :height 1.3
  :state (index track cursor)
  :shader
  (let ((n (max 1.0 track.num-steps))
        (at (- cursor (* n (floor (/ cursor n)))))
        (cur (* (if (= index at) 1.0 0.0) track.in-selection)))
    (sdf/layer
      (sdf/stroke (sdf/rounded-rect (* width 0.99) (* height 0.98) 0.1) 0.045
        (if (> cur 0.5) (rgba 0.78 0.82 0.9 0.95) (rgba 0 0 0 0))))))

;; The tab's field of step s, as a binding.
(def mode-ref (s mode)
  (match mode
    0 0
    1 #'s.delay
    2 #'s.duration
    3 #'s.retrig
    4 #'s.transpose
    _ #'s.sync))

;; What step s says in words: the value a dial is writing, else on the Sync
;; tab the grid an active step waits for (a named choice reads better than
;; any mark), else nothing.
(def step-text (s mode)
  (let ((r (readout-of s)))
    (if (= r.text "")
      (if (and (= mode 5) s.active (> s.sync 0.5))
        (sync-name s.sync)
        "")
      r.text)))

(def step-cell (t s mode)
  (box
    :key (str "lanes-step-" t.index "-" s.index)
    :width (if fit-steps 0 step-width) :flex (if fit-steps 1 0) :height step-height
    :background "lanes-step"
    :part "sequencer/step"
    :step s
    :track t
    :index s.index
    :selected #'s.selected
    :mode mode
    :value (mode-ref s mode)
    :editing (let ((r (readout-of s))) #'r.editing)
    :pos (if (= mode 5) #'transport.position 0)
    :on-mouse-down (lambda (evt) (cell-down s evt))
    :on-drag (lambda (evt) (cell-drag s evt))
    :on-mouse-up (lambda (evt) (cell-up s evt))
    :on-double-click (lambda (evt) (cell-double-click s evt))
    (box :width :fill :height step-height
      :background "lanes-cursor"
      :index s.index
      :track t
      :cursor #'grid-cursor.step
      :h-align :center :v-align :center
      (subtree :key (str "lanes-readout-" t.tid "-" s.index)
        (h-stack :gap 0 :align :center
          ;; Room for the Sync tab's watch at the step's left.
          (if (= mode 5) (box :width (if (> s.sync 5.5) 1.3 1.15) :height 0.1) (nothing))
          (label (step-text s mode)
            :font-size (if (= mode 5) 7.5 9) :color '(rgba 0.03 0.03 0.04 1.0) :bg :transparent
            :h-align :center))))))

;; ── the header columns ──────────────────────────────────────────────────────

(def indent 0.7)
(def name-width 15)
(def toggle-width 1.7)
(def steps-width 4)
(def timebase-width 5.0)
(def swing-width 5)
(def column-gap 0.35)
(def header-pad 0.3)
(def scroll-gutter 1.0)

;; Everything left of the steps, the same for every row and the ruler.
(def header-width
  (+ name-width steps-width timebase-width swing-width
     (* 4 column-gap) (* 2 header-pad)))

(def row-height () (+ step-height 0.12))

;; A pattern longer than a bar wraps: `row-steps` steps a row.
(def row-steps 16)
(def track-rows (t) (max 1 (ceil (/ t.num-steps row-steps))))

;; The color bars of the groups a row sits in (`bars`: their colors, the
;; outermost first).
(def group-bars (bars key rows)
  (let ((height (* rows (row-height))))
    (h-stack :gap 0
      (each bars |c i|
        (box :key (str key "-bar-" i)
          :width indent :height height
          (box :width (* indent 0.45) :height height
            :background-color (color-rgba c 0.9)))))))

(def toggle (text key lit on-click &key (lit-bg :sequencer-solo-on-bg) (lit-fg :sequencer-solo-on-fg)
    (bg :sequencer-toggle-off-bg) (fg :gray))
  (button text
    :key key
    :width toggle-width :height 0.8 :padding 0 :font-size 9.5
    :border-color :transparent
    :active lit
    :corner-radius 0
    :background-color bg
    :active-background-color lit-bg
    :color fg
    :active-color lit-fg
    :on-click on-click))

;; A column's caption over the ruler.
(def caption (text width)
  (label text :width width :font-size 9 :color :dim :bg :transparent :h-align :center))

;; The name plate: a strip of the track's color (the group's for a header)
;; down the left of a mixer-strip plate holding the arm dot, the name and
;; `buttons` (mute and solo), as tall as the track's `rows` of steps.
(def strip-width 0.35)
(def name-plate (key text c width rows dimmed selected on-click arm buttons)
  (let ((height (- (* rows (row-height)) 0.12)))
    (box :key key
      :width width :height height
      :corner-radius 3
      :background-color :mixer-strip-bg
      :selected selected
      :selected-background-color :mixer-strip-selected-bg
      :v-align :center
      :on-click on-click
      (h-stack :gap 0.2 :align :center
        (box :key (str key "-strip")
          :width strip-width :height height
          :background-color (color-rgba c (if dimmed 0.4 1.0)))
        (if arm arm (box :width arm-width :height 0.1))
        (label text
          :width (plate-text-width width) :font-size 10.5
          :color (if dimmed :dim :fg) :bg :transparent :h-align :left)
        buttons
        (box :width 0.2 :height 0.1)))))

;; The record-arm dot (the factory grid's), lit by `armed`, a binding.
(def arm-width 1.7)
(def arm-dot (key armed on-click)
  (box :key key
    :width arm-width :height 1.3
    :background "seqv-rec-arm-dot"
    :active armed
    :on-click on-click))

;; The name's share of a plate `width` wide, beside the arm dot and buttons.
(def plate-text-width (width) (- width (* 2 toggle-width) arm-width strip-width 1.3))

;; How many characters of a name fit a plate `width` wide.
(def plate-chars (width) (max 3 (floor (* 1.45 (plate-text-width width)))))

(def fit (name chars)
  (if (> (len name) chars) (str (substring name 0 (- chars 1)) "…") name))

(def set-track-steps! (t n)
  (select-track! t)
  (eseq.seq-core-state/cool-off-follow)
  (seq-set-track-param :num-steps (max 1 (round n))))

;; The Timebase dropdown: with steps selected on t it p-locks them to the
;; timebase (as the track panel's does), otherwise it sets the track's own.
(def set-timebase! (t v)
  (eseq.seq-core-state/cool-off-follow)
  (if (and t.selected (seq-has-selection?))
    (seq-plock-timebase v)
    (set! t.timebase v)))

;; t's timebase p-lock at the displayed step (the current track's selected,
;; else playing, step), or nil.
(def timebase-lock (t)
  (first (filter (lambda (row) (= (get row :name) "timebase")) t.setting-locks)))

;; The dropdown shows that lock over the track's own timebase, in the
;; p-lock accent, as the *track* panel's does. A subtree: a lock coming or
;; going re-renders the dropdown alone.
(def timebase-control (t)
  (subtree :key (str "lanes-timebase-run-" t.tid)
    (let ((lock (timebase-lock t)))
      (h-stack :gap 0
        (dropdown :key (str "lanes-timebase-" t.index)
          :value (if lock (get lock :value) t.timebase)
          :options timebase-options
          :on-change (lambda (v) (set-timebase! t v))
          :plock-active (if lock 1 0)
          :badge-color :transparent
          :corner-radius 0
          :chevron-color :gray
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :width timebase-width :height 0.85 :font-size 8)))))

;; A subtree: a mute or solo (which changes who is audible on every track)
;; re-renders headers, not the rows of steps beside them.
(def track-header (t bars)
  (subtree :key (str "lanes-header-" t.tid)
    (track-header-body t bars)))

(def track-header-body (t bars)
  (let ((i t.index)
      (plate-width (- name-width (* indent (len bars)))))
    (h-stack :gap column-gap :align :start
      (box :width header-pad :height 0.1)
      (box :key (str "lanes-name-" i)
        :height (* (track-rows t) (row-height)) :v-align :center
        :part "sequencer/track-name" :track t
        :drag-type "track-badge"
        :drag-payload (dict :track i)
        (name-plate (str "lanes-name-plate-" i)
          (fit t.name (plate-chars plate-width)) t.color plate-width
          (track-rows t) (not t.audible) #'t.selected
          (lambda (event) (eseq.sequencer/track-click event t))
          (arm-dot (str "arm-" i) #'t.armed
            (lambda (event) (select-track! t) (toggle! t.armed)))
          (h-stack :gap 0.2 :align :center
            ;; The track's number is its mute button, lit while it plays
            ;; (the factory header's).
            (toggle (str (eseq.kinds/track-number t)) (str "mute-" i) #'t.muted
              (lambda (event) (select-track! t) (toggle! t.muted))
              :bg :control-on-bg :fg :control-on-fg
              :lit-bg :sequencer-toggle-off-bg :lit-fg :gray)
            (toggle "S" (str "solo-" i) #'t.soloed
              (lambda (event) (select-track! t) (toggle! t.soloed))))))
      ;; The controls stay on the first row of a wrapped pattern.
      (box :height (row-height) :v-align :center
        (h-stack :gap column-gap :align :center
          (number-picker :key (str "lanes-steps-" i)
            :value t.num-steps :min 1 :max 64 :step 1 :decimals 0
            :border-color :none :font-size 8
            :mode :slider
            :corner-radius 0
            :on-change (lambda (v) (set-track-steps! t v))
            :width steps-width :height 0.85)
          (timebase-control t)
          (number-picker :key (str "lanes-swing-" i)
            :value #'t.swing :min 50 :max 75 :step 0.5 :decimals 1
            :border-color :none :font-size 8
            :unit "%"
            :corner-radius 0
            :mode :slider
            :on-change (lambda (v) (set! t.swing v))
            :width swing-width :height 0.85)))
      (box :width header-pad :height 0.1))))

;; ── rows ────────────────────────────────────────────────────────────────────

;; Which sound payloads a track row takes as a replacement of what it plays
;; (the factory row's rule).
(def sound-drop-types (t)
  (if (eseq.track-collapse/replaceable-type? t.instrument-type)
    (list "sample" "instrument" "instrument-preset" "sound")
    (list "sample")))

;; t's steps, sixteen a row.
(def step-rows (t mode)
  (let ((rows (chunks t.steps row-steps))
        (indices (range 0 (len rows))))
    (v-stack :gap 0 :key (str "track-step-grid-" t.index)
      :width (if fit-steps 0 nil) :flex (if fit-steps 1 0)
      (each indices |row|
        (let ((steps (nth rows row)))
          (h-stack :gap 0 :height (row-height) :align :center
            :width (if fit-steps :fill nil)
            (each steps |s| (step-cell t s mode))
            ;; A short row keeps its steps the width of a full row's.
            (if (and fit-steps (< (len steps) row-steps))
              (box :width 0 :height 0.1 :flex (- row-steps (len steps)))
              (nothing))))))))

(def track-row (t bars)
  (let ((i t.index)
        (mode lanes-view.mode))
    (box :width :fill
      :key (str "track-drop-" i)
      :background-color :buffer-bg
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types (eseq.drum-rack-v2/with-reorder-drop-types (sound-drop-types t))
      :drop-meta (dict :kind "track" :track i)
      :on-drop (lambda (event) (eseq.sequencer/drop-on-track event))
      :on-click (lambda (event) (eseq.sequencer/track-click event t))
      (h-stack :gap 0 :align :start :width :fill
        (group-bars bars (str "lanes-track-" t.tid) (track-rows t))
        (track-header t bars)
        ;; Audio tracks play regions, not steps.
        (if (= t.instrument-type "audio")
          (nothing)
          (step-rows t mode))
        (box :width scroll-gutter :height 0.1)))))

(def group-header (g bars)
  (let ((b g.bus)
        (key (lambda (part) (str (if g.rack "rack" "group") "-" part "-" g.gid)))
        (plate-width (- name-width (* indent (len bars)) 1.9))
        (pick (lambda (event) (select-group! g))))
    (box :width :fill
      :key (key "header-drop")
      :background-color (color-rgba g.color 0.1)
      :selected (eseq.seq-core-state/group-selected-ref g)
      :selected-background-color (color-rgba g.color 0.22)
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types eseq.drum-rack-v2/reorder-drag-types
      :drop-meta (dict :kind "group" :gidx g.index)
      :on-drop (lambda (event) (eseq.drum-rack-v2/drop-reorder-on-group event g))
      :on-click pick
      (h-stack :gap 0 :align :center
        (group-bars bars (key "bars") 1)
        (h-stack :gap column-gap :align :center
          (box :width header-pad :height (row-height))
          (disclosure-button
            :key (key "collapse")
            :width 1.55 :height 1.2
            :collapsed g.collapsed
            :col 0.9 :surface-alpha 1.0 :stroke-width 0.12
            :focusable true
            :on-click (lambda (x y region) (toggle! g.collapsed)))
          (box :key (key "select")
            :drag-type (eseq.drum-rack-v2/group-drag-type g)
            :drag-payload (dict :gid g.gid)
            (name-plate (key "name-plate")
              (fit g.name (plate-chars plate-width)) g.color plate-width
              1 (if (and b b.muted) true false)
              (eseq.seq-core-state/group-selected-ref g) pick
              ;; Arm = a drum rack's pad-play mode; a plain group has none.
              (if g.rack
                (arm-dot (key "arm") #'g.armed
                  (lambda (event) (select-group! g) (toggle! g.armed)))
                nil)
              (h-stack :gap 0.2 :align :center
                (toggle "M" (key "mute") (if b #'b.muted false)
                  (lambda (event) (when b (select-group! g) (toggle! b.muted)))
                  :lit-bg :control-on-bg :lit-fg :control-on-fg)
                (toggle "S" (key "solo") (if b #'b.soloed false)
                  (lambda (event) (when b (select-group! g) (toggle! b.soloed))))))))))))

;; A group: its header row, then (unless collapsed) its members and the
;; drum racks drawn inside it, each one bar deeper.
(def group-block (g viewed bars)
  (let ((inner (append bars (list g.color)))
        (kind (if g.rack "rack" "group")))
    (v-stack :width :fill :gap 0
      :key (str kind "-block-" g.gid)
      (subtree :key (str "seqv-" kind "-header-" g.gid)
        (group-header g bars))
      (if g.collapsed
        (nothing)
        (v-stack :width :fill :gap 0
          (each (eseq.drum-rack-v2/shown-members g viewed) |m|
            (subtree :key (str "sequencer-track-" m.tid)
              (track-row m inner)))
          (each (eseq.drum-rack-v2/grid-racks g viewed) |child|
            (subtree :key (str "sequencer-rack-" child.gid)
              (group-block child viewed inner))))))))

(def render-item (item)
  (if (= (get item :kind) "group")
    (let ((g (get item :group)))
      (subtree :key (str "sequencer-" (if g.rack "rack" "group") "-" g.gid)
        (group-block g (get item :viewed) (list))))
    (let ((t (get item :track)))
      (subtree :key (str "sequencer-track-" t.tid)
        (track-row t (list))))))

;; ── ruler and tabs ──────────────────────────────────────────────────────────

;; The beats of a row.
(def ruler-beats (range 0 (/ row-steps 4)))

(def ruler ()
  (let ((beats ruler-beats))
    (box :width :fill :background-color :mixer-strip-bg
    (h-stack :gap 0 :align :end :key "lanes-ruler" :width :fill
      (box :width header-width
        (h-stack :gap column-gap :align :center
          (box :width (+ header-pad name-width) :height 1)
          (caption "Steps" steps-width)
          (caption "Timebase" timebase-width)
          (caption "Swing" swing-width)))
      (box :width 0.4)
      (each beats |beat|
        (h-stack :gap 0 :align :end
          :key (str "lanes-ruler-beat-" beat)
          :width (if fit-steps 0 (* 4 step-width)) :flex (if fit-steps 1 0)
          (box :width 0.08 :height 0.55 :background-color :dim)
          (label (str (+ 1 (* 4 beat)))
            :width 3 :height 1.1
            :font-size 9 :color :dim :bg :transparent :h-align :left)))
      ;; The rows' scrollbar gutter.
      (box :width scroll-gutter :height 0.1)))))

(def tab (i)
  (let ((p (nth params i)))
    (button (get p :name)
      :key (str "lanes-tab-" i)
      :width 6.5 :height 0.9 :padding 0 :font-size 9
      :corner-radius 0
      :border-color :transparent
      :active (= lanes-view.mode i)
      :background-color :sequencer-toggle-off-bg
      :active-background-color :mixer-strip-selected-bg
      :color :dim
      :active-color :white
      :on-click (lambda (event) (set-mode i)))))

(def tab-indices (range 0 (len params)))

(def tabs ()
  (box :background-color :mixer-strip-bg :width :fill
    :height 1.2
    (h-stack :gap 0.25 :align :center :key "lanes-tabs"
      (box :width header-width :height 0.95)
      (each tab-indices |i| (tab i)))))

;; ── the buffer ──────────────────────────────────────────────────────────────

;; The ruler and the tabs stay put; the rows (and the new-track drop zone
;; under them) scroll between the two.
(def body ()
  (v-stack :key "sequencer-grid" :width :fill :height :fill :padding 0 :gap 0
    ;; The factory body's overlays: each is a closed modal with no footprint.
    (subtree :key "seq-export-song" (eseq.export-song/panel))
    (subtree :key "seq-file-dialogs" (eseq.file-dialogs/panel))
    (subtree :key "seq-sample-import" (eseq.sample-import/panel))
    (subtree :key "seq-retrospective" (eseq.retrospective/panel))
    (subtree :key "seq-resample" (eseq.resample/panel))
    (subtree :key "seq-sample-region" (eseq.sample-region/panel))
    (subtree :key "seq-sample-new-pack" (eseq.sample-region/new-pack-panel))
    (subtree :key "seq-factory-promote" (eseq.factory-promote/panel))
    (subtree :key "lanes-ruler-run" (ruler))
    (scroll :key "lanes-scroll" :width :fill :flex 1
      (v-stack :width :fill :gap 0
        (box :width :fill
          :key "lanes-rows"
          (v-stack :key "sequencer-tracks" :width :fill :gap 0
            (each (eseq.sequencer/grid-items) |item| (render-item item))))
        (box :key "new-track-drop-zone"
          :width :fill :height 2.4
          :background-color :transparent
          :drop-hover-background-color :mixer-control-bg
          :border-width 1
          :border-color :transparent
          :drop-hover-border-color :mixer-strip-selected-border
          :drop-types (list "sample" "instrument" "instrument-preset" "sound")
          :drop-meta (dict :kind "new-sample-track")
          :on-drop (lambda (event) (eseq.sequencer/drop-new-track event))
          :on-double-click (lambda (event) (host-command "add-track-empty" (dict)))
          (label "" :font-size 1 :color :transparent :bg :transparent))))
    (subtree :key "lanes-tabs-run" (tabs))))

(override eseq.sequencer/sequencer-body () (body))

;; M-x alez.lanes.ui/off brings the factory grid back without detaching the
;; package; /on returns to the lanes.
(def off () (disable-module-overrides alez.lanes.ui))
(def on () (enable-module-overrides alez.lanes.ui))
