;; ui/piano-roll.lisp -- step-quantized piano roll for the current track.
;; Renders to *piano-roll* buffer. Loaded by ui/main.lisp.
;;
;; Host state comes from eseq.kinds (docs/kind-bindings-spec.md §14.2j,
;; §14.2r): `piano-roll` is the current track's edit focus (its pinned clip,
;; loop window and playhead) with its `note`s and its `focus-step`s, which
;; the automation lane draws and edits. The note grid's gestures (select,
;; marquee, move, resize, nudge, copy, paste, create, delete) go to the
;; host's piano roll editor, `seq-piano-roll-action`, which addresses a note
;; by `n.item`. The scroll, zoom, cursor, marquee, how the piano roll was
;; entered and the lane's parameter are this view's own state: the `:key ()`
;; singletons below. The scroll, zoom, cursor and playhead are bound (#'), so
;; they repaint without re-rendering; the clip panel, the note grid, the
;; automation row and the lane's value readout are subtrees of their own.
;;
;; Names reached from outside keep their spelling (identity compat aliases,
;; tools/module-compat-aliases.tsv): ui/seq-panels.lisp requests fits and
;; sets the entry mode, ui/arrangement.lisp asks for it,
;; ui/step-grid-interactions.lisp selects all notes, and the host invokes
;; `piano-roll-apply-pending-fit` after each note sync.
(module eseq.piano-roll)

;; Compile-time edge (spec §4): `piano-roll-default-pane-height` lives in
;; eseq.seq-step-tabs (the layout hub).
(import eseq.seq-step-tabs)
(import eseq.track-collapse)
(import eseq.view-kit :refer (nothing listed? color-rgba))
(import eseq.kinds :refer (track piano-roll pitch-min pitch-max focus-step-params
                           focus-step-value set-focus-step! lock-param! unlock-param!
                           lock-rack-macro! unlock-rack-macro!))

(export piano-roll-view
        lane-view
        piano-roll-arrangement-mode?
        set-arrangement-mode!
        piano-roll-request-fit-for-track
        piano-roll-request-fit
        piano-roll-apply-pending-fit
        piano-roll-action
        piano-roll-select-all)

;; The note grid's view: the time window (start, duration, in steps), the
;; first lane shown and the lane height (cells), the cursor, the marquee
;; being drawn (the timeline's event), the length a new note gets.
;; `arrangement`: the piano roll was entered from arrangement clip gestures,
;; so it shows the selected clip (or that none is). `fit`: the track whose
;; notes the view fits to once the piano roll shows them. Re-evaluating this
;; file (activating the buffer does) keeps them.
(def-kind piano-roll-view
  :key ()
  :state ((start 0)
          (duration 10.6667)
          (lane-scroll 36)
          (lane-height 0.5)
          (cursor 0)
          (marquee :any :default nil)
          (create-duration 1)
          (arrangement false)
          (fit track :default nil)))

;; The automation lane's parameter: a step param by name (`step`), or a
;; device param (`param`) or rack macro (`macro`) of the piano roll's track,
;; which shows instead while set and still on that track. `edit-value`: the
;; value being dragged (the axis readout).
(def-kind lane-view
  :key ()
  :state ((step "velocity")
          (param param :default nil)
          (macro rack-macro :default nil)
          (edit-value :number :default nil)))

(def piano-roll-arrangement-mode? () piano-roll-view.arrangement)

;; Entered from arrangement clip gestures (true) or from anywhere else.
(def set-arrangement-mode! (on) (set! piano-roll-view.arrangement on))

(def header-height 2)
;; Automation lane row under the note grid (bead eseq-2k9p.21): a border
;; line over the lane's body.
(def automation-height 3.5)
(def lane-body-height (- automation-height 0.08))
(def view-padding 1)
(def min-view-duration 4)
(def max-view-duration 256)

;; The grid's rows, top to bottom: lane i is pitch-max minus i. A C names
;; its octave.
(def black-keys (list 1 3 6 8 10))
(def lane-row (lane)
  (let ((pitch (- pitch-max lane))
        (pitch-class (mod (+ pitch 60) 12))
        (black (listed? pitch-class black-keys)))
    (dict :id lane
          :label (if (= pitch-class 0) (str "C" (+ 4 (/ pitch 12))) "")
          :sidebar-bg (if black :black :white)
          :label-fg (if black :white :black))))
(def lanes (map lane-row (range 0 (+ (- pitch-max pitch-min) 1))))

;; The timeline gestures the host's piano roll editor handles.
(def native-actions
  (list :select :clear-selection :finish-marquee-select :delete-items :copy-items
        :paste-items :nudge-selection :move-items-absolute :resize-item-absolute
        :finish-move-items :finish-resize-items :finish-create-item))

(def event-num (event key fallback)
  (let ((value (get event key)))
    (if (= value nil) fallback value)))

;; The focus: the live pattern (follow mode), a pinned pattern or a pinned
;; take. A pinned pattern CLIP (whose pattern may also be the live one) has
;; a loop window to slide and a signed offset.
(def live-focus? () (= piano-roll.focus-kind "live"))
(def take-focus? () (= piano-roll.focus-kind "take"))
(def pinned-pattern? () (= piano-roll.clip-kind "pattern"))

(def content-height ()
  (max 1
    (- eseq.seq-step-tabs/piano-roll-default-pane-height
      header-height
      automation-height)))

(def visible-lane-count ()
  (/ (content-height) piano-roll-view.lane-height))

(def max-lane-scroll ()
  (max 0 (- (len lanes) (visible-lane-count))))

(def set-lane-scroll (scroll)
  (set! piano-roll-view.lane-scroll (clamp scroll 0 (max-lane-scroll))))

(def action-duration (event)
  (max 0.03125
    (event-num event :duration
      (- (event-num event :end 1)
         (event-num event :start 0)))))

(def set-cursor-from-event (event)
  (let ((time (get event :time)))
    (unless (= time nil)
      (set! piano-roll-view.cursor time))))

;; The axis is the FOCUS length (clip-edit-target spec 3.5): a pinned
;; pattern's num-steps, a pinned take's playable length, or the live
;; pattern's in follow mode; the track's own until the host has a focus.
(def num-steps ()
  (let ((focus piano-roll.focus-num-steps) (t piano-roll.track))
    (if (or (> focus 0) (= t nil)) focus t.num-steps)))

(def set-view-start (start duration)
  (set! piano-roll-view.start
    (clamp start 0 (max 0 (- (+ (num-steps) 4) duration)))))

(def zoom-view (event)
  (let ((v piano-roll-view)
        (anchor (event-num event :anchor-time v.start))
        (next (clamp (/ v.duration (event-num event :factor 1))
                min-view-duration max-view-duration))
        (ratio (if (<= v.duration 0)
                 0.5
                 (clamp (/ (- anchor v.start) v.duration) 0 1))))
    (set! v.duration next)
    (set-view-start (- anchor (* ratio next)) next)))

(def zoom-lanes (event)
  (let ((v piano-roll-view)
        (anchor (event-num event :anchor-lane v.lane-scroll))
        (height v.lane-height)
        (next (clamp (* height (event-num event :factor 1)) 0.5 6)))
    (set! v.lane-height next)
    (set-lane-scroll (- anchor (* (- anchor v.lane-scroll) (/ height next))))))

;; To an absolute position (`:view-start`, `:lane-scroll`) or by a delta.
(def scroll-view (event)
  (let ((v piano-roll-view))
    (set-view-start
      (event-num event :view-start (+ v.start (event-num event :delta-time 0)))
      v.duration)
    (set-lane-scroll
      (event-num event :lane-scroll (+ v.lane-scroll (event-num event :delta-lanes 0))))))

(def note-end (n) (+ n.start n.length))
(def note-lane (n) (- pitch-max n.pitch))
;; The smallest and largest of xs (`min` / `max` compile to an opcode in
;; call position only, so they cannot be applied).
(def least (xs) (reduce |lo x| (min lo x) (first xs) xs))
(def greatest (xs) (reduce |hi x| (max hi x) (first xs) xs))

(def fit-horizontal (min-start max-end)
  (let ((start (max 0 (- min-start view-padding)))
        (end (max min-start (+ max-end view-padding)))
        (duration (clamp (- end start) min-view-duration max-view-duration)))
    (set! piano-roll-view.duration duration)
    (set-view-start start duration)))

(def fit-vertical (min-lane max-lane)
  (let ((center (/ (+ min-lane max-lane 1) 2)))
    (set-lane-scroll (- center (/ (visible-lane-count) 2)))))

(def fit-notes-to-view ()
  (let ((notes piano-roll.notes))
    (if (empty? notes)
      (let ((middle (floor (/ (- (len lanes) 1) 2))))
        (set! piano-roll-view.duration (clamp (num-steps) min-view-duration max-view-duration))
        (set-view-start 0 piano-roll-view.duration)
        (fit-vertical middle middle))
      (let ((rows (map note-lane notes)))
        (fit-horizontal (least (map |n| n.start notes)) (greatest (map note-end notes)))
        (fit-vertical (least rows) (greatest rows))))))

;; Fit the view to track t's notes: now when the piano roll shows t, else
;; once the host has moved it there (it invokes piano-roll-apply-pending-fit
;; after each note sync).
(def request-fit (t)
  (set! piano-roll-view.fit t)
  (piano-roll-apply-pending-fit))

(def piano-roll-request-fit-for-track (i) (request-fit (track i)))

(def piano-roll-request-fit () (request-fit piano-roll.track))

;; True when it fitted (the host then runs a reactive cycle).
(def piano-roll-apply-pending-fit ()
  (let ((t piano-roll-view.fit))
    (when (and t (= t piano-roll.track))
      (fit-notes-to-view)
      (set! piano-roll-view.fit nil)
      true)))

;; A focus command on the piano roll's track (they address it by position).
(def focus-command (name payload)
  (host-command name (merge payload :track piano-roll.track.index)))

(def set-live-length (length)
  (eseq.seq-core-state/cool-off-follow)
  (seq-set-track-param :num-steps length))

;; The loop bar and the panel's Length edit the FOCUSED source's length
;; (clip-edit-target spec 5, locked decision 3): the live pattern through
;; the track, a pinned pattern through the pattern-addressed write (the
;; SHARED pattern: every clip referencing it), a take's linear axis (which
;; can outgrow one pattern: chunks) through its own resize. Patterns and the
;; live path stay capped at MAX_STEPS.
(def set-focus-length (length)
  (match piano-roll.focus-kind
    ;; Coalesced take resize: grows mint silent chunks, shrinks keep noted
    ;; ones; one undo entry per drag.
    "take" (focus-command "focus-take-set-length" (dict :length length))
    ;; Stage only: frames coalesce into one undo entry, sealed like a
    ;; device-knob gesture (the seal drains the deferred song-row refresh).
    ;; The loop bar's release seals it (finish-focus-length).
    "pattern" (focus-command "focus-set-num-steps" (dict :length length))
    "live" (set-live-length length)))

(def finish-focus-length ()
  (when (= piano-roll.focus-kind "pattern")
    (focus-command "focus-finish-num-steps" (dict))))

(def piano-roll-action (event)
  (match event.type
    :scroll-view (scroll-view event)
    :zoom-view (zoom-view event)
    :zoom-lanes (zoom-lanes event)
    :set-cursor (set! piano-roll-view.cursor event.time)
    ;; A take's band is read-only here (its Length picker edits it).
    :resize-content-length (unless (take-focus?) (set-focus-length event.length))
    :finish-resize-content-length (unless (take-focus?) (finish-focus-length))
    ;; Band-body slide (spec 5): live frames are preview-only; the release
    ;; carries the TOTAL delta and lowers to one undoable phase edit.
    :slide-band nil
    :finish-slide-band
    (let ((delta (round (event-num event :delta-time 0))))
      (when (and (pinned-pattern?) (not (= delta 0)))
        (focus-command "focus-slide-band" (dict :delta-steps delta))))
    :marquee-select (set! piano-roll-view.marquee event)
    :finish-marquee-select (set! piano-roll-view.marquee nil)
    :select (clear-marquee event)
    :clear-selection (clear-marquee event)
    :finish-create-item (set! piano-roll-view.create-duration (action-duration event))
    :resize-item-absolute (set! piano-roll-view.create-duration (action-duration event)))
  (when (listed? event.type native-actions)
    (seq-piano-roll-action event)))

(def clear-marquee (event)
  (set! piano-roll-view.marquee nil)
  (set-cursor-from-event event))

;; Cmd+A: select every NOTE of the focused content, not the step grid. A
;; chord's notes are separate items, so a later Backspace keeps the voices
;; that were not selected: the same :delete-items path a marquee uses. Goes
;; straight to the native so no cursor moves; without notes it does nothing.
(def piano-roll-select-all ()
  (let ((items (map |n| n.item piano-roll.notes)))
    (unless (empty? items)
      (set! piano-roll-view.marquee nil)
      (seq-piano-roll-action (dict :type :select :ids items)))))

;; A note as the timeline draws it, by the id the editor addresses.
(def note-item (n)
  (dict :id n.item :lane (note-lane n) :start n.start :end (note-end n)
        :selected n.selected :label n.label))

(def piano-roll-timeline (t)
  (let ((v piano-roll-view))
    (box :height :fill :flex 1 :width 0
      ;; Flex child of the panel row (the arrangement-lane idiom): the
      ;; timeline absorbs exactly the width left of the clip panel — without
      ;; :width 0 :flex 1 an h-stack child lays out against an INFINITE max
      ;; width.
      (timeline
        :width :fill
        :height :fill
        :focusable true
        :sidebar-width 5
        :sidebar-style :piano
        :header-height header-height
        :time-ruler (dict :mode :bars-beats :beats-per-bar 4)
        :item-color t.color
        :loop-color t.color
        :tool :pointer
        :playhead-time #'piano-roll.playhead
        :cursor-time #'piano-roll-view.cursor
        :lanes lanes
        :items (map note-item piano-roll.notes)
        :selection-rect v.marquee
        :view-start #'piano-roll-view.start
        :view-duration #'piano-roll-view.duration
        :zoom-min-duration min-view-duration
        :zoom-max-duration max-view-duration
        :content-length (num-steps)
        :content-length-min 1
        :content-length-max 256
        ;; Loop-window gestures + overlay (clip-edit-target spec 5): only a
        ;; pinned clip has a window to slide or mark.
        :band-slide (pinned-pattern?)
        :window-marker piano-roll.window-marker
        :window-span piano-roll.window-span
        :window-repeat piano-roll.window-repeat
        :lane-scroll v.lane-scroll
        :lane-height v.lane-height
        :scroll-viewport-height (- eseq.seq-step-tabs/piano-roll-default-pane-height
                                  automation-height)
        :snap 1
        :min-duration 0.03125
        :create-duration v.create-duration
        :move-snap-mode :alignment-helper
        :resize-snap :grid
        :snap-mode :floor
        :resize-snap-mode :alignment-helper
        :scroll-mode :smooth
        :on-action piano-roll-action))))

;; ── Clip panel (clip-edit-target spec 6) ───────────────────────────────────
;; Ableton-style numeric column left of the piano roll: source identity,
;; Start/End (beats), signed start offset, length, loop duality. Start/End/
;; Offset only exist for a pinned clip; in follow mode the column shows the
;; session identity, the live length and the loop row.
;;
;; Widget `:key`s auto-qualify inside a module (hazard a), so the hand-rolled
;; "piano-roll-" prefix is dropped: "panel-start" renders as
;; "eseq.piano-roll/panel-start".

(def clip-panel-width 24)

;; A dim caption left of its control.
(def panel-row (name body &key (gap 0.3))
  (h-stack :gap gap :align :center
    (box :width 4.6 :height 1.0
      (label name :font-size 10 :color :dim :bg :transparent))
    body))

(def panel-static (name text key)
  (panel-row name
    (box :width 5 :height 1.0 :h-align :left
      (label text :key key :font-size 10 :color :white :bg :transparent))))

;; Signed display (spec 6): an offset in the top half of the pattern reads as
;; a negative pickup — offset L−1 shows as −1, exactly Ableton's start = −1
;; with the loop at 0. A take's offset clamps at 0 and never wraps, so it
;; reads raw.
(def shown-offset (offset)
  (let ((steps (num-steps)))
    (if (and (pinned-pattern?) (> offset (/ steps 2)))
      (- offset steps)
      offset)))

(def clip-picker (key value lo hi on-change &key (noui false))
  (number-picker :key key
    :value value
    :min lo :max hi :decimals 0
    :noui noui
    :background-color :buffer-bg
    :on-change on-change
    :width 5 :height 1.0 :font-size 8))

(def resize-clip (start end)
  (focus-command "focus-clip-resize" (dict :start-beat start :end-beat end)))

(def clip-panel-rows (c)
  (v-stack :gap 0.2
    (h-stack
      (panel-row "Start"
        (clip-picker "panel-start" c.start 0 512
          |v| (unless (= v c.start) (resize-clip v c.end))))
      (panel-row "End"
        (clip-picker "panel-end" c.end 0 512
          |v| (unless (= v c.end) (resize-clip c.start v)))))
    (panel-row "Offset"
      (clip-picker "panel-offset" (shown-offset c.offset)
        (if (pinned-pattern?) -256 0) 256
        |v| (unless (= v (shown-offset c.offset))
              (focus-command "focus-set-offset" (dict :offset-steps v)))))))

(def panel-length-row ()
  (panel-row "Length"
    (clip-picker "panel-length" (num-steps) 1 (if (take-focus?) 4096 256)
      |v| (unless (= v (num-steps)) (set-focus-length v))
      :noui true)))

(def clip-panel (t)
  (box :height :fill
    (v-stack
      :height :fill
      :padding 0 :width clip-panel-width
      (box
        :height 1
        :width :fill
        (h-stack
          :gap 0
          (badge (substring t.name 0 15)
            :key (str "pianoroll-label-content-" t.index)
            :icon (eseq.track-collapse/instrument-icon t.instrument-type)
            :width clip-panel-width
            :height 1.0
            :padding 0
            :font-size 10
            :h-align :left
            :background-color (color-rgba t.color 1.0)
            :border-color :transparent
            :highlight-color :transparent
            :shadow-color :transparent
            :color :black
            :bg :transparent)))
      (box :width :fill :height 1.0
        :background-color :mixer-strip-selected-bg
        (h-stack (box :width 0.5)
          (label piano-roll.focus-label
            :key "panel-source"
            :font-size 10 :color :white :bg :transparent)))
      (box :padding 1 :height 5
        :background-color :mixer-strip-bg
        :width clip-panel-width
        (v-stack :gap 0.25
          (let ((c piano-roll.clip))
            (if c (clip-panel-rows c) (nothing)))
          (panel-length-row)
          ;; Informational in v1 (spec 6): states the pattern/take duality;
          ;; layout leaves room for Position/Length when sub-pattern windows
          ;; land (5.1).
          (panel-static "Loop" (if (take-focus?) "off" "on") "panel-loop"))))))

;; ── Automation lane (bead eseq-2k9p.21) ───────────────────────────────────
;; One parameter across the focus axis, Ableton velocity-lane style: a dot at
;; each note's onset spanning the note's duration at the value in force.
;; Locked values draw in the track color; a device parameter's BASE value
;; (what plays when the step carries no lock) draws gray, and dragging it
;; writes a lock. The picker lists the step params plus only the device
;; params and rack macros that carry a lock somewhere in the pattern — never
;; every lockable parameter. A view over the focus steps (spec §14.2r).

;; A device's group in the picker: inst for the instrument, else its name.
(def device-group (d) (if (= d.role "instrument") "inst" d.name))

;; Whether x (a param or a rack macro) is on track t: a stale handle reads
;; no device (and a field of nil is an error).
(def on-track? (x t)
  (let ((d (and x x.device)))
    (and d (= d.track t))))

(def param-label (p) (str (device-group p.device) " " p.name))
(def macro-label (rm) (str "rack " rm.name))

;; What the lane edits and its scale, without its points.
(def step-target (row)
  (dict :kind :step :target (get row :name) :label (get row :label)
        :min (get row :min) :max (get row :max) :default (get row :default)
        :increment (get row :increment) :editable true))

;; The picker names a device param or rack macro by its group while it is
;; listed (it carries a lock), else by its own name. Device locks are the
;; live pattern's: editable on the live focus only.
(def device-target (kind x label lo hi increment)
  (dict :kind kind :target x :label (if x.has-locks label x.name)
        :min lo :max hi :default x.base :increment increment
        :editable (live-focus?)))

(def param-target (p)
  (device-target :param p (param-label p) p.min p.max (if (= p.type "continuous") 0 1)))

(def macro-target (rm)
  (device-target :macro rm (macro-label rm) 0 1 0))

(def step-param-row (name)
  (first (filter |row| (= (get row :name) name) (focus-step-params))))

;; The lane the piano roll shows: the selected device param or rack macro
;; while on its track, else the selected step param (velocity when that is
;; gone; nothing before the host lists them).
(def lane-target (t)
  (let ((p lane-view.param) (rm lane-view.macro))
    (if (on-track? p t)
      (param-target p)
      (if (on-track? rm t)
        (macro-target rm)
        (let ((row (or (step-param-row lane-view.step) (step-param-row "velocity"))))
          (if row
            (step-target row)
            (dict :label "Velocity" :editable false)))))))

(def lane-point (fs value locked)
  (dict :step fs.index :start fs.start :end fs.end :value value :locked locked
        :active fs.active))

;; A device lane's points: each step holding a lock in `locks` ((step value)
;; rows), and each other active step at `base` (gray).
(def lock-points (locks base)
  (filter |point| point
    (map |fs| (let ((lock (first (filter |row| (= (first row) fs.index) locks))))
                (if lock
                  (lane-point fs (nth lock 1) true)
                  (when fs.active (lane-point fs base false))))
         piano-roll.steps)))

;; A step param's points are its values at the active steps; a device
;; param's or rack macro's its locks and base (live focus only).
(def lane-points (target)
  (let ((kind (get target :kind)) (x (get target :target)))
    (if (= kind :step)
      (map |fs| (lane-point fs (focus-step-value fs x) true)
           (filter |fs| fs.active piano-roll.steps))
      (if (and kind (live-focus?)) (lock-points x.step-locks x.base) (list)))))

(def current-lane (t)
  (let ((target (lane-target t)))
    (merge target :points (lane-points target))))

;; What the picker lists: the step params, then the params and rack macros of
;; the track's devices that carry a lock, as the legacy lane ordered them
;; (instrument and effects, rack macros, MIDI effects).
(def locked-params (devices)
  (apply append
    (map |d| (map |p| (dict :label (param-label p) :param p)
                  (filter |p| p.has-locks d.params))
         devices)))

(def lane-options (t)
  (let ((instrument (first t.devices)))
    (append
      (map |row| (dict :label (get row :label) :step (get row :name))
           (focus-step-params))
      (locked-params t.devices)
      (if instrument
        (map |rm| (dict :label (macro-label rm) :macro rm)
             (filter |rm| rm.has-locks instrument.macros))
        (list))
      (locked-params t.midi-devices))))

(def select-lane (t label)
  (let ((option (first (filter |o| (= (get o :label) label) (lane-options t)))))
    (when option
      (set! lane-view.step (or (get option :step) "velocity"))
      (set! lane-view.param (get option :param))
      (set! lane-view.macro (get option :macro))
      (set! lane-view.edit-value nil))))

;; The live step `step` of the piano roll's track, as the lock actions take
;; steps (device locks are the live pattern's).
(def live-steps (step) (list (nth piano-roll.track.steps step)))

;; Write value at step: a lock of a device param or rack macro, a step
;; param's value (a drag's frames join one undo entry, per step for a lock).
(def write-lane (target step value)
  (let ((x (get target :target)))
    (match (get target :kind)
      :param (lock-param! x (live-steps step) value)
      :macro (lock-rack-macro! x (live-steps step) value)
      :step (set-focus-step! (nth piano-roll.steps step) x value))))

;; Clear step: a device lock goes, a step param returns to its default.
(def clear-lane (target step)
  (let ((x (get target :target)))
    (match (get target :kind)
      :param (unlock-param! x (live-steps step))
      :macro (unlock-rack-macro! x (live-steps step))
      :step (write-lane target step (get target :default)))))

;; The lane's `on-change` contract: `(:set step value)` per press/drag frame,
;; `(:finish step value)` on release, `(:clear step value)` on double-click or
;; alt-click.
(def automation-action (kind step value)
  (let ((target (lane-target piano-roll.track)))
    ;; Release always clears feedback, even if the focus became read-only.
    (when (= kind :finish) (set! lane-view.edit-value nil))
    (when (get target :editable)
      (match kind
        :set (do (set! lane-view.edit-value value) (write-lane target step value))
        :clear (do (set! lane-view.edit-value nil) (clear-lane target step))))))

(def format-lane-value (v)
  (if (= v nil)
    ""
    (let ((rounded (/ (round (* v 100)) 100)))
      (if (= rounded (round rounded))
        (str (round rounded))
        (str rounded)))))

(def axis-label (text key v-align font-size color)
  (box :width :fill :height 1.0 :h-align :right :v-align v-align
    (label text
      :key key
      :width 4.4 :h-align :right
      :font-size font-size :color color :bg :transparent)))

;; Range axis in the sidebar strip left of the lane: max at the top, min at
;; the bottom, the edited value between them in white (a drag re-renders it
;; alone).
(def automation-axis (lane)
  (box :width 5 :height lane-body-height
    (v-stack :gap 0 :height lane-body-height :width :fill :align :end
      (axis-label (format-lane-value (get lane :max)) "automation-axis-max" :top 8 :dim)
      (subtree :key "automation-axis-readout"
        (axis-label (format-lane-value lane-view.edit-value) "automation-axis-value"
          :center 10 :white))
      (axis-label (format-lane-value (get lane :min)) "automation-axis-min" :bottom 8 :dim))))

;; The line over the lane row.
(def lane-border ()
  (box :width :fill :height 0.08 :background-color :mixer-strip-border))

;; Lane header: mirrors the clip panel's column (dim caption, white value)
;; so the picker reads as part of the panel rather than a floating control.
(def automation-header (t lane)
  (box :width (+ clip-panel-width 5) :height automation-height
    :background-color :mixer-strip-bg
    (v-stack :gap 0 :height :fill :width :fill
      (lane-border)
      (h-stack :gap 0 :height lane-body-height :width :fill
        (box :padding 1 :height lane-body-height :width clip-panel-width
          :v-align :center
          (panel-row "Lane"
            (dropdown
              :key "automation-param"
              :value (get lane :label)
              :options (map |o| (get o :label) (lane-options t))
              :on-change |label| (select-lane t label)
              :badge-color :transparent
              :background-color :buffer-bg
              :width 12 :height 1.3 :font-size 10)
            :gap 0.5))
        (automation-axis lane)))))

(def automation-row (t)
  (let ((lane (current-lane t)))
    (h-stack :width :fill :gap 0.0 :height automation-height
      (automation-header t lane)
      (box :height automation-height :flex 1 :width 0
        (v-stack :gap 0 :height :fill :width :fill
          (lane-border)
          (automation-lane
            :key "automation-lane"
            :width :fill
            :height lane-body-height
            :points (get lane :points)
            :min (get lane :min)
            :max (get lane :max)
            :default (get lane :default)
            :increment (get lane :increment)
            :view-start #'piano-roll-view.start
            :view-duration #'piano-roll-view.duration
            :color t.color
            :base-color (list 0.55 0.55 0.55)
            :background :buffer-bg
            :on-change automation-action))))))

;; The clip panel, the note grid and the automation row are subtrees: a note
;; edit re-renders the grid (and the lane), a lane pick the row alone.
(def buffer-content ()
  (let ((t piano-roll.track))
    (if (and piano-roll-view.arrangement (not piano-roll.clip))
      (box
        :key "no-clip-selected"
        :width :fill :height :fill
        :h-align :center :v-align :center
        (label "No clip selected"
          :key "no-clip-selected-label"
          :font-size 11 :color :dim :bg :transparent))
      (if t
        (v-stack :width :fill :gap 0.0 :height :fill
          (box :width :fill :flex 1 :height 0
            (h-stack :width :fill :gap 0.0 :height :fill
              (subtree :key "clip-panel" (clip-panel t))
              (subtree :key "note-grid" (piano-roll-timeline t))))
          (subtree :key "automation-row" (automation-row t)))
        (nothing)))))

;; Widget-only buffer: take the shared sequencer keymap (was an implicit host default).
(set-buffer-mode-for "*piano-roll*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*piano-roll*"
  (box :key "piano-roll-panel" :width :fill :height :fill
    (buffer-content)))
