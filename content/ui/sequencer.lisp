;; ui/sequencer.lisp — Project step sequencer view.
;; Renders to *sequencer* buffer. Shows every track's step grid laid out
;; vertically. Loaded by ui/main.lisp.
;;
;; The view's host state comes from eseq.kinds (docs/kind-bindings-spec.md):
;; tracks, their steps, process chains and lanes, groups and their buses,
;; drum rack pads and clips. The step cells, the playhead bars, the expanded
;; editor's slots and the header's meter, fader, arm, mute and solo bind
;; their fields with #' (`step-cell` takes the step and its track), so
;; playback, step edits, mixing and metering repaint without re-running a
;; row. The step cursor, the selection anchor, the expanded editors, the lane
;; patchbay, the pad grid and the menus are this view's own state: `:key ()`
;; singletons below, and the expanded editor's per-track fields on the track
;; (`t.expanded`, `param-mode`, `cursor`, `page`).

(module eseq.sequencer)
;; Compile-time edge (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile.
(import eseq.seq-core-state)

(import eseq.track-collapse)

;; Drum rack v2: the pad grid's note geometry (docs/drum-rack-v2-spec.md).
(import eseq.drum-rack-v2)

(import eseq.seq-panels)
;; Expr cards' edit buffers (the node bay's edit button, the error dot).
(import eseq.expr-buffer)
;; Process-port arm/bind state shared with the fx panel (lane strip map button).
(import eseq.effects.param-controls :as pc :refer (process-map))
(import eseq.seqv-track-params :as tp)
(import eseq.step-grid-interactions :as sgi)

;; Drag-and-drop sample import modal (zero footprint while closed).
(import eseq.sample-import)
(import eseq.retrospective)
(import eseq.resample)
(import eseq.factory-promote)
(import eseq.export-song)
(import eseq.file-dialogs)
(import eseq.view-kit :refer (open-menu! menu-of nothing listed? index-of named rgb-part dimmed-part
                               track-color-part))
(import eseq.kinds :refer (track tracks groups selection transport project process-library
                           launch-rack-clip! save-rack-clip-as! delete-rack-clip! take-none
                           take-governed set-bar-transpose! set-process-enabled! set-inlet!
                           set-fanout! set-lane-steps! move-process! add-process!
                           remove-process! bind-port! add-fanout! unbind-port! clear-port!
                           remove-fanout! graph-of))

(export lane-patchbay-node lane-patch-register-node lane-patch-node-namespace
        harmony-snap-meter process-scope-cells-for
        lane-patch-run-error lane-patch-expr-error lane-patch-hidden-in-port-count
        lane-patch-node-touch lane-patch-node-version-value lane-patch-node-selected-id
        lane-patch-node-select lane-patch-node-host?
        grid-cursor
        grid-select
        seq-view
        clip-menu
        clip-rename
        lane-edit
        patch-view
        card-menu
        lane-add
        pad-view
        pad-menu
        expanded-tracks
        select-track-for-edit
        open-piano-roll-for-track
        show-fx-for-group
        set-track-expanded
        shown-page
        slot-step
        set-expanded-step-param
        set-expanded-current-param
        goto-page
        lane-patch-show
        lane-patch-select-cable
        lane-patch-cable-selected?
        lane-patch-delete-selected
        lane-patch-select-lane
        lane-patch-lane-selected?
        lane-patch-connect
        lane-patch-remove-card
        lane-patch-move-card
        lane-add-pick
        lane-add-options
        lane-add-labels
        lane-patch-add-menu lane-patch-add-menu-grouped
        lane-patch-pending-port
        lane-toggle-edit-scope
        lane-slider-step
        set-lane-slider-step
        track-param-mode
        set-track-param-mode
        track-cursor
        set-track-cursor
        cursor-step-changed
        current-selected-step
        current-param-mode
        current-number-picker-key
        param-mode-for-key
        select-all-current-track-steps
        collapse-all-tracks
        toggle-current-track-expanded
        handle-key
        track-click
        track-menu-click
        drop-sample-on-track
        drop-on-track
        drop-new-track
        step-slider-track-material
        track-header
        grid-items
        grid-step-pointer-down
        grid-step-pointer-up
        process-lane-options
        select-process-lane-option
        pad-grid
        pad-map
        pad-page
        set-pad-page
        pad-cell-note
        pad-at
        pad-cell-drop-meta
        drop-on-pad-cell
        drop-pad-on-note
        focused-pad
        focus-pad!
        open-pad!
        open-pad-menu
        choose-pad-role
        pad-selected?
        selected-pad
        open-pad-member-fx
        rack-pad-grid
        rack-pad-context-menu
        rack-pad-map)

;; ── View state ──

;; The step cursor the grid draws: the shared step cursor
;; (`eseq.seq-core-state/cursor-step-value`), mirrored by
;; `cursor-step-changed` whenever it moves. The selected tracks' cells show
;; it, wrapped to each track's length.
(def-kind grid-cursor
  :key ()
  :state ((step 0)))

;; The stable end of a Finder-style range selection (shift-click); a plain
;; selection starts a new range, cmd-click leaves it alone.
(def-kind grid-select
  :key ()
  :state ((anchor track :default nil)))

;; The expanded step editors, by stable track id (`t.tid`), so they follow
;; their track through a delete and its undo or a reorder: the expanded
;; tracks' tids, and per track its param mode (`(dict :tid tid :mode m)`)
;; and step cursor (`(dict :tid tid :step s)`). No render reads them: rows
;; draw the track's own fields (`t.expanded`, `param-mode`, `cursor`,
;; `page`), so one track's toggle, mode or page re-renders that row alone;
;; `*seq-expand-sync*` keeps those fields in step with these lists.
(def-kind seq-view
  :key ()
  :state ((expanded '())
          (modes '())
          (cursors '())))

;; The rack clip menu (where it opened, the clip it targets) and the inline
;; clip rename in progress (the clip, nil when none is, and the draft).
(def-kind clip-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (clip rack-clip :default nil)))

(def-kind clip-rename
  :key ()
  :state ((clip rack-clip :default nil)
          (draft "")))

;; The lane strip's edit scope (`all`: project lanes edit the shared slot
;; every track inherits, else this track's fork) and the UI-only slider step
;; per lane: `(dict :key k :step s)` entries keyed by the lane's process id
;; and inlet, so a forked lane keeps its own step (0 = free).
(def-kind lane-edit
  :key ()
  :state ((all false)
          (steps '())))

;; The lane patchbay: shown under the strip, the out port being dragged (or
;; armed by a click; -1 when idle: port id 0 is a legal port) and its place
;; as the port shader compares it (`pending-bay`, `pending-slot`: see
;; `port-bay`), the selected cable (a dict, nil for none), the graph node
;; bays registered by the views that expand a node (`(ns graph node)` lists,
;; newest first), a counter their node-process edits bump for readers outside
;; the kinds, and a node bay's selected process (its proc-id; -1 for none).
(def-kind patch-view
  :key ()
  :state ((show true)
          (pending -1)
          (pending-bay -1)
          (pending-slot -1)
          (cable :any :default nil)
          (node-targets '())
          (node-version 0)
          (node-selected -1)))

;; A patchbay card's menu: where it opened, the bay's namespace and the
;; process it targets (its proc-id and name).
(def-kind card-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (bay -1)
          (proc-id -1)
          (name "")))

;; The process the + box just added, waiting for the host to list it so its
;; lane can be selected: the track, the class and the proc-ids the track had
;; before (`*lane-add-sync*` resolves it).
(def-kind lane-add
  :key ()
  :state ((track track :default nil)
          (class "")
          (known '())))

;; The pad grid: the rack whose page was set last and that page (any other
;; rack opens on its own default page), and the pad the rack panel focuses.
(def-kind pad-view
  :key ()
  :state ((group group :default nil)
          (page 0)
          (focus pad :default nil)))

;; The pad grid's role menu: where it opened and the pad it targets.
(def-kind pad-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (pad pad :default nil)))

;; ── Tracks ──

;; Track i, from a position the host hands over (drop meta, a key handler,
;; the step cursor hook); nil when there is none.
(def track-at (i) (if (= i nil) nil (track i)))

(def pointer-volume (event)
  (max 0.0 (min 1.0 (* 0.5 (+ event.sx 1.0)))))

;; x's (a track's or a bus's) volume from a fader click or drag.
(def set-volume! (x event)
  (unless (= event.sx nil)
    (set! x.volume (pointer-volume event))))

;; `text` cut to `max-chars`, ending in ".." when cut.
(def clip-label (text max-chars)
  (let ((s (str text)))
    (if (> (len s) max-chars)
      (str (substring s 0 (- max-chars 2)) "..")
      s)))

(def track-name-max-chars 9)

(def track-name-display (name) (clip-label name track-name-max-chars))

;; Entry `(dict :tid tid …)` of `entries` for t, or nil.
(def entry-for (entries t) (find-by-key entries :tid t.tid))

;; `entries` with t's entry replaced by `(dict :tid t.tid key value)`, less
;; those of tracks no longer listed.
(def with-entry (entries t key value)
  (let ((tids (map (lambda (x) x.tid) (tracks))))
    (cons (dict :tid t.tid key value)
          (filter (lambda (entry)
                    (let ((tid (get entry :tid)))
                      (and (not (= tid t.tid)) (listed? tid tids))))
                  entries))))

(def project-cursor-step (t step)
  (mod step (max 1 t.num-steps)))

(def sync-track-cursor-to-global (t)
  (set-track-cursor t (eseq.seq-core-state/cursor-step-value)))

(def sync-all-track-cursors-to-global ()
  (for-each sync-track-cursor-to-global (tracks)))

;; Selecting an edit target must not change workspace layout: opening an
;; editor is an explicit gesture (a name double-click opens the piano roll).
(def select-track-for-edit (t)
  (set! grid-select.anchor t)
  (set! eseq.seq-core-state/selected-bus -1)
  (unless t.selected (seq-clear-selection))
  (set! selection.track t)
  (sync-track-cursor-to-global t))

(def track-selection-click? (event)
  (or event.shift event.additive-selection))

;; Shift-click = select the range from the anchor in the grid's visual order;
;; cmd-click = toggle membership; a plain click selects the track for edit.
;; A held anchor whose track is gone falls back to the current track.
(def track-click (event t)
  (if event.shift
    (let ((held grid-select.anchor)
          (anchor (if (listed? held (tracks)) held (or selection.track t))))
      (set! grid-select.anchor anchor)
      (set! eseq.seq-core-state/selected-bus -1)
      (seq-select-tracks
        (map (lambda (m) m.index)
          (eseq.seq-core-state/track-range-in-order (visible-tracks) anchor t))
        t.index))
    (if event.additive-selection
      (do
        (set! eseq.seq-core-state/selected-bus -1)
        (seq-toggle-track-selected t.index))
      (select-track-for-edit t)))
  (sync-track-cursor-to-global t))

;; Modified track-control clicks select tracks without also editing a control.
(def track-control-click (event t action)
  (if (track-selection-click? event)
    (track-click event t)
    (action)))

;; A header control's click on t: a modified click selects tracks, a plain
;; one selects t for edit and runs `action`.
(def track-toggle-click (t action)
  (lambda (event)
    (track-control-click event t
      (lambda () (select-track-for-edit t) (action)))))

(def open-piano-roll-for-track (t)
  ;; A badge's first click may arm deletion; double-click is navigation.
  (seq-clear-delete-target)
  (select-track-for-edit t)
  (if (= t.instrument-type "empty")
    (eseq.browser/open-device-picker)
    (if (= eseq.seq-step-tabs/lower-panel-buffer "*piano-roll*")
      (eseq.seq-panels/seq-show-fx-lower-panel)
      (eseq.seq-panels/seq-open-piano-roll-bottom-for-track t.index))))

;; ── Expanded step editors ──

(def expanded-tracks ()
  (filter (lambda (t) t.expanded) (tracks)))

(def set-track-expanded (t expanded)
  (let ((others (filter (lambda (tid) (not (= tid t.tid))) seq-view.expanded)))
    (set! seq-view.expanded (if expanded (cons t.tid others) others)))
  (set! t.expanded expanded))

;; Steps per page (and slots in an expanded editor).
(def slots-per-page eseq.seq-core-state/page-size)

;; Track t's 16-step pages (one at least).
(def track-pages (t)
  (max 1 (floor (/ (+ t.num-steps (- slots-per-page 1)) slots-per-page))))

;; The page step `step` of track t is on.
(def page-of-step (t step)
  (min (floor (/ step slots-per-page)) (- (track-pages t) 1)))

;; t's own step cursor while it holds one (`seq-view.cursors`), else the
;; shared cursor; wrapped to its length.
(def track-cursor (t)
  (let ((entry (entry-for seq-view.cursors t)))
    (project-cursor-step t (if entry (get entry :step) grid-cursor.step))))

(def track-param-mode (t)
  (let ((entry (entry-for seq-view.modes t)))
    (if entry (get entry :mode) 0)))

;; t's editor fields: param mode `mode`, step cursor `cursor` and the page it
;; is on, each written only when it changes.
(def put-editor-state (t mode cursor)
  (unless (= t.param-mode mode) (set! t.param-mode mode))
  (unless (= t.cursor cursor) (set! t.cursor cursor))
  (let ((page (page-of-step t cursor)))
    (unless (= t.page page) (set! t.page page))))

;; The track fields keyed by position (`expanded`, `param-mode`, `cursor`,
;; `page`) follow their track by its tid: when tracks move (a delete, its
;; undo, a reorder) or a length changes, each position takes the state of the
;; track now there. Nil-returning, like `*sel-sync*`; only the fields that
;; change re-render their rows.
(effect-buffer "*seq-expand-sync*"
  (let ((tids seq-view.expanded))
    (for-each (lambda (t)
                (let ((expanded (listed? t.tid tids)))
                  (unless (= t.expanded expanded)
                    (set! t.expanded expanded))
                  (put-editor-state t (track-param-mode t) (track-cursor t))))
      (tracks))
    nil))

(def set-track-param-mode (t mode)
  (set! seq-view.modes (with-entry seq-view.modes t :mode mode))
  (put-editor-state t mode t.cursor))

(def set-track-cursor (t step)
  (let ((cursor (project-cursor-step t step)))
    (set! seq-view.cursors (with-entry seq-view.cursors t :step cursor))
    (put-editor-state t (track-param-mode t) cursor)))

;; The step cursor moved to `step` on the track at position `track` (the
;; grid's cursor hook): the grid draws it there.
(def cursor-step-changed (track step)
  (set! grid-cursor.step step)
  (let ((t (track-at track)))
    (when t (set-track-cursor t step))))

;; Stub-then-override protocol (module spec §10 hazard i). The flat name stays
;; pinned through the §3 cross-module def escape hatch because
;; ui/step-grid-interactions.lisp compiles and calls the stub before this file
;; replaces it. This pair is the S4 `defhook` candidate.
(def eseq.vanilla/sequencer-cursor-step-changed (track step)
  (eseq.sequencer/cursor-step-changed track step))

(def current-selected-step ()
  (let ((t selection.track))
    (if t (track-cursor t) 0)))

(def current-param-mode ()
  (let ((t selection.track))
    (if t (track-param-mode t) 0)))

;; Returns a widget stable key for Rust to look up verbatim
;; (`current_step_param_number_picker_key`, src/ui/input.rs:762, feeds
;; `layout_node_by_stable_key`, an exact match).  Widget `:key`s in a declared
;; module hash as `<module>/<key>`, so a lisp helper that hands a key *out* to
;; Rust has to emit the qualified spelling itself — the module name is part of
;; the value, not just of the def.
(def current-number-picker-key ()
  (let ((t selection.track))
    (if t (str "eseq.sequencer/expanded-param-number-picker-" t.tid) "")))

(def select-current-param-mode (mode)
  (let ((t selection.track))
    (when t (set-track-param-mode t mode))))

(def param-mode-for-key (key)
  (if (not (= (len key) 1))
    -1
    (match (string-downcase key)
      "v" 0
      "d" 1
      "t" 3
      "p" 4
      "s" 5
      "x" (let ((t selection.track))
            (if (and t (not (empty? t.lanes)))
              tp/seqv-process-lane-mode-offset
              -1))
      _ -1)))

;; A selected drum rack keeps its bus selection: Cmd+A then spans its members
;; (step-grid-interactions/select-all-steps).
(def select-all-current-track-steps ()
  (when (< (eseq.drum-rack-v2/rack-of-bus eseq.seq-core-state/selected-bus) 0)
    (set! eseq.seq-core-state/selected-bus -1))
  (sgi/select-all-steps))

(def collapse-all-tracks ()
  (for-each (lambda (t) (set-track-expanded t false)) (expanded-tracks)))

(def toggle-current-track-expanded ()
  (let ((t selection.track))
    (set! eseq.seq-core-state/selected-bus -1)
    (when t (set-track-expanded t (not t.expanded)))))

;; The grid's keys: a param mode's letter picks that mode, the others run
;; their action. Whether the key was handled.
(def handle-key (key text)
  (let ((mode (param-mode-for-key key)))
    (if (>= mode 0)
      (do (select-current-param-mode mode) true)
      (let ((action (match key
                      "LEFT" sgi/cursor-left
                      "RIGHT" sgi/cursor-right
                      "C-a" select-all-current-track-steps
                      "C-h" collapse-all-tracks
                      "C-H" collapse-all-tracks
                      "BS" delete-key
                      "Delete" delete-key
                      "RET" sgi/cursor-toggle
                      _ nil)))
        (when action (action) true)))))

;; BS / Delete: a selected patch-bay cable goes first, else the selected steps.
(def delete-key ()
  (unless (lane-patch-delete-selected)
    (sgi/delete-selected-steps)))

(def track-menu-click (t)
  (select-track-for-edit t)
  (set-track-expanded t (not t.expanded)))

;; ── Drops ──
;; Drop events carry the host's drop meta: a track by position, the address
;; the add/drop host commands take.

(def drop-sample-on-track (event)
  (let ((target (get event :target))
        (t (track-at (get target :track))))
    (if (get (get event :payload) :path)
      (do
        (when (and t (not (get target :from-pad)))
          (select-track-for-edit t))
        (eseq.browser/drop-sample-on-track event))
      (status "Drop a sample file, not a folder"))))

(def drop-on-track (event)
  (if (listed? (get event :drag-type) (list "sound" "instrument" "instrument-preset"))
    (eseq.browser/drop-sound-on-track event)
    (drop-sample-on-track event)))

(def drop-new-track (event)
  (let ((payload (get event :payload))
        (path (get payload :path)))
    (match (get event :drag-type)
      "sound" (if path
                (host-command "add-track-from-sound" (dict :path path))
                (status "Drop a Sound item, not a folder"))
      "instrument" (eseq.browser/drop-instrument-new-track payload)
      "instrument-preset" (eseq.browser/drop-preset-new-track payload nil)
      _ (if path
          (host-command "add-track-sample" (dict :path path :preserve-browser-context true))
          (status "Drop a sample file, not a folder")))))


(defwidget seqv-track-container
  :width 1.5 :height 1.5
  :state ()
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.45)
      (rgba 0 0 0 0))))

(defwidget group-track-indicator
  :width 1.5 :height 1.5
  :state ()
  :shader
  (sdf/layer
    (sdf/fill (sdf/translate -0.4 0 (sdf/circle 0.2))
      :dim)
    (sdf/fill (sdf/translate -0.4 0.5 (sdf/circle 0.2))
      :dim)
    (sdf/fill (sdf/translate -0.4 -0.5 (sdf/circle 0.2))
      :dim)
    (sdf/fill (sdf/translate 0 1.0 (sdf/circle 0.2))
      :dim)
    (sdf/fill (sdf/translate -0.4 1.0 (sdf/circle 0.2))
      :dim)
    (sdf/fill (sdf/translate 0.4 1.0 (sdf/circle 0.2))
      :dim)
    ))

(defwidget seqv-rec-arm-dot
  :width 1.5 :height 1.5
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.8)
      (material
        :lighting (lighting :edge-min -0.35 :edge-max 0.5
          :light (vec3 0.0 -1.0 3.5) :shininess 82.0)
        :color
        (* (if (= active 1) 1.0 (+ 0.2 (smoothstep -0.4 0.1 d)))
          (eseq.materials/color
            (rgba
              (if (= active 1) 0.85 0.5)
              (if (= active 1) 0.05 0.5)
              (if (= active 1) 0.05 0.5)
              1.0)
            (rgba 0.99 0.15 0.15 1.0)))))))

;; A group's color badge (its color by value).
(defwidget seqv-track-color-badge
  :width 0.68 :height 1.5
  :paint-margin 0.08
  :state (track-r track-g track-b)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.28)
      (rgba track-r track-g track-b 1.0)))
  )

;; Shader color c (a vec3) pulled toward gray, as a track that is not heard
;; (or whose lane a take governs) draws it: `eseq.view-kit/dimmed` in a
;; shader. Shaders spell it `eseq.sequencer/silenced` (module spec §10
;; hazard h).
(defmacro silenced (c)
  `(+ (* ,c 0.34) (* (vec3 0.10 0.10 0.11) 0.66)))

;; A track's color badge: its color, pulled toward gray while it is not
;; heard (muted, or silenced by a solo).
(defwidget seqv-track-badge
  :width 0.68 :height 1.5
  :paint-margin 0.08
  :state (track)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.28)
      (rgba
        (if (= track.audible 1) track.color (eseq.sequencer/silenced track.color))
        1.0))))

(defwidget seqv-track-volume-meter
  :width 8.2 :height 1.05
  :paint-margin 0.28
  :state (level volume)
  :shader
  (let ((lvl (min 1.0 (max 0.0 level)))
        (vol (min 1.0 (max 0.0 volume)))
        (green-end (min lvl 0.70))
        (yellow-end (min lvl 0.88))
        (red-end lvl)
        (handle-x (* aspect (- (* 2.0 vol) 1.0)))
        (lane-track
          (min
            (sdf/translate 0.0 -0.23 (sdf/rounded-rect width 0.17 0.0))
            (sdf/translate 0.0 0.23 (sdf/rounded-rect width 0.17 0.0)))))
    (sdf/layer
      (sdf/fill lane-track
        (material
          :lighting (lighting :edge-min -0.45 :edge-max 0.35
            :light (vec3 0.0 -1.4 2.2) :shininess 24.0)
          :color (rgba 0.025 0.030 0.038 0.72)))
      (if (> green-end 0.005)
        (sdf/fill
          (let ((__start 0.0)
                (__end green-end)
                (__half_w (* 0.5 aspect (- __end __start))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end))))))
              (min
                (sdf/translate 0.0 -0.23 (sdf/rounded-rect __half_w 0.17 0.0))
                (sdf/translate 0.0 0.23 (sdf/rounded-rect __half_w 0.17 0.0)))))
          (material :color (rgba 0.12 0.86 0.34 1.0)))
        (rgba 0 0 0 0))
      (if (> (- yellow-end 0.70) 0.005)
        (sdf/fill
          (let ((__start 0.70)
                (__end yellow-end)
                (__half_w (* 0.5 aspect (- __end __start))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end))))))
              (min
                (sdf/translate 0.0 -0.23 (sdf/rounded-rect __half_w 0.17 0.0))
                (sdf/translate 0.0 0.23 (sdf/rounded-rect __half_w 0.17 0.0)))))
          (material :color (rgba 0.96 0.82 0.18 1.0)))
        (rgba 0 0 0 0))
      (if (> (- red-end 0.88) 0.005)
        (sdf/fill
          (let ((__start 0.88)
                (__end red-end)
                (__half_w (* 0.5 aspect (- __end __start))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end))))))
              (min
                (sdf/translate 0.0 -0.23 (sdf/rounded-rect __half_w 0.17 0.0))
                (sdf/translate 0.0 0.23 (sdf/rounded-rect __half_w 0.17 0.0)))))
          (material :color (rgba 0.95 0.18 0.16 1.0)))
        (rgba 0 0 0 0))
      (sdf/fill
        (sdf/translate handle-x 0.0 (sdf/circle 0.72))
        (material
          :lighting (lighting :edge-min -0.24 :edge-max 0.62
            :light (vec3 -0.35 -1.0 2.8) :shininess 54.0)
          :color :sequencer-volume-handle)))))

(defwidget seqv-ellipsis-button
  :width 2.2 :height 1.2
  :state (expanded)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width (* 0.99 height) 0.98)
      :mixer-strip-selected-bg)
    
    (sdf/fill (sdf/rounded-rect (* 0.96 width) (* 0.93 height) 0.98)
      (material
        :lighting (lighting :edge-min -0.45 :edge-max 0.4
          :light (vec3 0.1 -1.2 2.4) :shininess 24.0)
        :color 
        (if expanded 
          (rgba 0.18 0.18 0.20 1.0) 
          :bg) 
        ))
    (sdf/fill
      (sdf/translate -0.48 0
        (sdf/circle 0.12))
      (material :color (rgba 0.60 0.62 0.68 1.0)))
    (sdf/fill (sdf/circle 0.12)
      (material :color (rgba 0.60 0.62 0.68 1.0)))
    (sdf/fill
      (sdf/translate 0.48 0
        (sdf/circle 0.12))
      (material :color (rgba 0.60 0.62 0.68 1.0)))))

;; Module spec §10 hazard (h): the two `:material` props that call this expand
;; much later, at shader-compile time, in a throwaway *implicit-module*
;; compiler, so "current module" there is `eseq.vanilla` and a bare call would
;; not find this macro.  Both call sites spell it `eseq.sequencer/…`.
;; Renamed off `seqv-aqua-slider-track-material` rather than mechanically
;; stripped: bare `aqua-slider-track-material` is ui/materials.lisp's compat
;; alias for `eseq.materials/slider-track-material`, and in that same
;; implicit-module expansion the alias rung would have won.
(defmacro step-slider-track-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.8413
       :light (vec3 -0.1 -0.61 3.5) :shininess 81.0)
     :color
       (* (if (= active 1) 1.0 0.42)
          (eseq.materials/color
            (if (= active 1)
              (rgba (* track-r 0.55) (* track-g 0.55) (* track-b 0.55) 1.0)
              (rgba
                (+ (* track-r 0.36) 0.06)
                (+ (* track-g 0.36) 0.06)
                (+ (* track-b 0.36) 0.08)
                0.85))
            (if (= active 1)
              (rgba track-r track-g track-b 1.0)
              (rgba
                (+ (* track-r 0.30) 0.04)
                (+ (* track-g 0.30) 0.04)
                (+ (* track-b 0.30) 0.08)
                0.85))))))


;; One grid row's playhead bar (row `row`, steps 16·row onward, of `track`):
;; the playing step's column, and an amber underline beneath the step a
;; length lane (`length!`) last set the pattern length to, drawn first so
;; the playhead passes over it.
(defwidget seqv-playhead-row-bar
  :width 48.8 :height 0.24
  :paint-margin 0.18
  :state (track row)
  :shader
  (let ((col (if (= (floor (/ track.playhead 16.0)) row) (- track.playhead (* row 16.0)) -1.0))
        (len-col (if (= (floor (/ track.length-step 16.0)) row)
                   (- track.length-step (* row 16.0))
                   -1.0)))
    (sdf/layer
      (if (< len-col 0)
        (rgba 0 0 0 0)
        (let ((len-start (/ (+ len-col 0.08) 16.0))
              (len-end (/ (+ len-col 0.92) 16.0))
              (len-half-w (* 0.5 aspect (- len-end len-start))))
          (sdf/fill
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ len-start len-end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect len-half-w 0.2 0.06))
            (material :color (rgba 0.94 0.63 0.24 0.95)))))
      (if (< col 0)
        (rgba 0 0 0 0)
        (let ((step-w (/ 1.0 16.0))
              (center (/ (+ col 0.5) 16.0))
              (trail-start (max 0.0 (- center (* step-w 1.55))))
              (start (- center (* step-w 0.46)))
              (end (+ center (* step-w 0.46)))
              (trail-half-w (* 0.5 aspect (- end trail-start)))
              (__half_w (* 0.5 aspect (- end start)))
              (__half_h 0.32)
              (__radius 0.07))
          (sdf/layer
            (sdf/fill
              (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ trail-start end)))))
                    (y (* 0.5 y)))
                (sdf/rounded-rect trail-half-w 0.09 0.06))
              (material
                :color
                (rgba 0.32 0.48 1.0
                  (* 0.42
                    (smoothstep trail-start center (/ (+ x aspect) (* 2.0 aspect)))
                    (smoothstep 0.82 0.0 (abs y))))))
            (sdf/fill
              (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ start end)))))
                    (y (* 0.5 y)))
                (sdf/rounded-rect __half_w __half_h __radius))
              (material
                :color
                (mix
                  (rgba 0.20 0.42 1.0 0.38)
                  (rgba 0.82 0.92 1.0 1.0)
                  (smoothstep 0.85 0.0 (abs y)))
                :shadow (shadow
                  :color (rgba 0.25 0.45 1.0 0.72)
                  :blur 0.12
                  :offset (vec2 0 0))))))))))

;; The lamp behind row `row`'s number in `track`'s grid, the row's
;; background: lit while the playhead plays in that row.
(defwidget seqv-row-lamp
  :width 1 :height 1
  :state (track row)
  :shader
  (if (< track.playhead 0)
    (rgba 0 0 0 0)
    (if (= (floor (/ track.playhead 16.0)) row)
      (sdf/layer
        (sdf/fill
          (let ((x (+ x (- aspect 1.16))))
            (sdf/rounded-rect 0.5 0.5 0.25))
          (material :color (rgba 0.32 0.48 1.0 0.55))))
      (rgba 0 0 0 0))))

;; The step shell's layers, shared by the grid's `seqv-step-shell` and the
;; expanded editor's `seqv-slot-shell`: the gate `active`, the p-lock kind
;; `lock` (0 none, 1 a lock, 2 a p-lock variant) with the variant's color
;; `variant` (a vec3), and `under`, layers drawn first (the grid's duration
;; span). The widget declares `track`, `selected` and `off-fill-r/-g/-b`.
;; Shaders spell it `eseq.sequencer/step-shell-shader` (hazard h).
(defmacro step-shell-shader (active lock variant &rest under)
  `(let ((active ,active)
         (plock-kind ,lock)
         (muted (- 1.0 track.audible))
         (tc (if (= track.governed 1) (eseq.sequencer/silenced track.color) track.color))
         (vcol (rgba ,variant 1.0))
         (seqcol (rgba 0.545 0.545 0.588 0.95))
         (radius (if (= active 1) 1 0.7))
         (border input-color)
         (offcol (rgba off-fill-r off-fill-g off-fill-b 1)))
     (sdf/layer
       ,@under
       ;; border
       (sdf/fill (sdf/circle (* radius 0.8))
         (material
           :lighting (lighting :edge-min -0.12 :edge-max 0.9
             :light (vec3 -0.3 0.7 3.8) :shininess 92.0)
           :color (* (if (= selected 1) 1 (if (= muted 1) 0.6 1.2))
                     (eseq.materials/color border border))))
       (sdf/fill (sdf/circle (* radius (if (= selected 1) 0.64 0.69)))
         (material
           :lighting (lighting :edge-min -0.15 :edge-max 1.0
             :light (vec3 0.3 -2.0 0.8) :shininess 92.0)
           :color (* (if (= muted 1) 0.3 1) (eseq.materials/color offcol offcol))))
       ;; p-lock indicator
       (sdf/fill
         (sdf/translate 0.0 0.89
           (sdf/rounded-rect 0.17 0.08 0.09))
         (material
           ;; The tick tracks p-locks, not the gate: off steps are a
           ;; deliberate p-lock target (warp bpm, sampler ranges) and must
           ;; show the same indicator an on step shows. Only the muted
           ;; neutral fill still keys off `active`.
           :color (if (= plock-kind 0)
                    (if (= active 1)
                      (if (= muted 1) border (rgba 0 0 0 0))
                      (rgba 0 0 0 0))
                    (if (= muted 1)
                      border
                      (if (= plock-kind 2) vcol seqcol)))
           :shadow (shadow
                     :color (if (= muted 1)
                              (rgba 0 0 0 0)
                              (if (= plock-kind 2) (rgba ,variant 0.70) (rgba 0 0 0 0)))
                     :blur (if (= muted 1) 0.0 (if (= plock-kind 2) 0.12 0.0))
                     :offset (vec2 0 0))))
       ;; toggled fill
       (sdf/fill (sdf/circle (if (= selected 1) 0.35 0.5))
         (material
           :lighting (lighting :edge-min -0.15 :edge-max 1.15
             :light (vec3 0.01 -0.4 1.8) :shininess 32.0)
           :color (if (= active 1)
                    (if (= muted 1)
                      (* 0.7 (eseq.materials/color offcol border))
                      (eseq.materials/color
                        (rgba (* tc (vec3 0.72 0.72 0.82)) 1.0)
                        (rgba tc 1.0)))
                    (rgba 0 0 0 0)))))))

;; A step's shell: its gate, selection, p-lock tick (in the variant's color
;; for a p-lock variant) and, while the step is held by an earlier step's
;; duration, the span that duration covers. `selected` is the step's
;; (`#'step.selected`): a scalar state, so the renderer's selected rim
;; (`:selected-color`) follows it. Colored by its track, dimmed while a take
;; governs the lane; a track that is not heard swaps the colored layers for
;; neutral ones.
(defwidget seqv-step-shell
  :width 1.5 :height 2.5
  :paint-margin 1
  :state (step track selected off-fill-r off-fill-g off-fill-b)
  :shader
  (eseq.sequencer/step-shell-shader step.active step.lock-kind step.variant-color
    ;; duration span
    (sdf/fill
      (sdf/translate 0.0 0.0
        (sdf/rounded-rect (* 3.0 width) (* 1.0 height) 0))
      (material
        :lighting (lighting :edge-min -0.32 :edge-max 1.293
          :light (vec3 0.8 -0.8 3.5) :shininess 92.0)
        :color (* 0.7 (if (= step.held 1)
                        (if (= muted 1)
                          (rgba 0 0 0 0)
                          (eseq.materials/color
                            (mix border (rgba (* tc 0.85) 0.5) (if (= selected 1) 0.8 0.6))
                            (if (= selected 1) border (rgba tc 0.6))))
                        (rgba 0 0 0 0)))))))

;; The step cursor's frame around step `index` of `track`: drawn while the
;; track is selected and `cursor` (bound: the grid's `grid-cursor.step`, an
;; expanded editor's `t.cursor`), wrapped to the track's length, is on it.
(defwidget seqv-step-cursor
  :width 1 :height 1
  :state (index track cursor)
  :shader
  (let ((n (max 1.0 track.num-steps))
        (at (- cursor (* n (floor (/ cursor n))))))
    (sdf/layer
      (sdf/stroke (sdf/rounded-rect (* width 0.94) (* height 0.99) 0.10)
        0.055
        (rgba 0.72 0.76 0.84 (* 0.95 (if (= index at) 1 0) track.in-selection))))))

;; A track's take state (`track.governed`, takes spec 10 UX): take-none, the
;; lane plays its pattern and stays fully editable (jam with the step
;; sequencer while the arrangement plays); take-governed, a take plays on the
;; lane (dimmed steps, non-interactive grid, lit Back-to-Song play button);
;; take-latched, a take lane the performer manually latched away (editable
;; again; the grey play button returns it to the song).
(def song-governed? (t)
  (= t.governed take-governed))

;; Per-track take-lane indicator / Back-to-Song button: a play triangle that
;; sits lit green while a take governs the lane and grey while the lane is
;; manually latched (clicking then hands it back to the song). `take-state`
;; is `track.governed`; at take-none the triangle renders fully transparent
;; — the box is ALWAYS in the layout so lanes flipping between pattern and
;; take never trigger a re-layout, only a repaint.
(defwidget seqv-back-to-song-icon
  :width 1.5 :height 1.5
  :state (take-state)
  :shader
  (sdf/layer
    (sdf/fill
      (let ((p1x -0.32) (p1y -0.44) (p2x -0.32) (p2y 0.44) (p3x 0.5) (p3y 0.0))
        (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
            (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
            (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
          (max (max d1 d2) d3)))
      (material :color
        (if (= take-state 1)
          (rgba 0.35 0.82 0.40 1.0)
          (if (= take-state 2)
            :mixer-control-bg
            (rgba 0 0 0 0)))))))


(defwidget seqv-back-to-song-bg
  :width 1.5 :height 1.5
  :state (take-state)
  :shader
        (if (= take-state 1)
          (rgba 0.3 0.3 0.3 0.5)
          (if (= take-state 2)
            (rgba 0.32 0.33 0.37 0.5)
            (rgba 0 0 0 0))))

;; Compact mixer track row — the common track actions plus an inline
;; meter/fader so the sequencer remains usable when the mixer is hidden.
;; Names and structural edits rebuild only this header. Mute/solo styling
;; uses bindings throughout, including the name, so it never rebuilds a tree.
;; The arrangement draws its track headers with this one (`bare` true).
(def track-header (t bare)
  (subtree :key (str "seqv-track-header-" t.tid)
    (track-header-body t bare)))

;; A fader over x's (a track's or a bus's) level meter, keyed `key`.
(def volume-meter (key x on-click on-drag)
  (v-stack (box :height 0.13)
    (box
      :key key
      :width 8.2 :height 1.25
      :background "seqv-track-volume-meter"
      :level #'x.peak
      :volume #'x.volume
      :on-click on-click
      :on-drag on-drag)))

(def track-volume-control (t)
  (volume-meter (str "track-volume-control-" t.index) t
    (lambda (event)
      (track-control-click event t
        (lambda () (select-track-for-edit t) (set-volume! t event))))
    (lambda (event) (select-track-for-edit t) (set-volume! t event))))

;; Step-grid sizing knob (content-tiers spec: customize tier). Every step
;; cell, the ghost filler cells that pad short rows, and the track colour
;; badge in the header derive their heights from this one number, so it sets
;; how tall (or how dense) the whole sequencer reads. No shader change is
;; needed: the step widgets scale their artwork with the box.
(defcustom step-cell-width 3.05
  :type :number :min 1.5 :max 6 :step 0.05
  :doc "Width (cells) of each step in the sequencer grid; the playhead bar under each row spans the same width.")

(defcustom step-cell-height 1.55
  :type :number :min 1 :max 4 :step 0.05
  :doc "Height (cells) of each step in the sequencer grid; track rows and the colour badge scale with it.")

;; Header colour badge: stock 2.0 at the stock 1.55 step height.
(def track-color-badge-height ()
  (* step-cell-height 1.29))

;; Row number label beside each grid row: stock 1.1 at the stock 1.55 step.
(def track-row-label-height ()
  (* step-cell-height 0.71))

;; A header toggle (mute, solo) lit by `on`, a binding.
(def header-toggle (text key on &key (bg :sequencer-toggle-off-bg)
                    (active-bg :sequencer-solo-on-bg) (color :gray)
                    (active-color :sequencer-solo-on-fg) (on-click nil))
  (button text
    :key key
    :width 1.55 :height 1.2 :padding 0 :font-size 10
    :border-color :transparent
    :active on
    :background-color bg
    :active-background-color active-bg
    :color color
    :active-color active-color
    :on-click on-click))

(def track-header-body (t bare)
  (let ((i t.index)
        (c t.color))
    (box :background "seqv-track-container"
      :padding 0.1

      :on-click (lambda (event) (track-click event t))
      (h-stack :gap 0.4 :align :center
        (box
          :key (str "color-badge-" i)
          :width 0.68 :height (track-color-badge-height)
          :background "seqv-track-badge"
          :track t
          :on-click (lambda (event) (track-click event t)))
        (box :width 2 :height 1.5
          :background "seqv-rec-arm-dot"
          :key (str "arm-" i)
          :active #'t.armed
          :on-click (track-toggle-click t (lambda () (toggle! t.armed))))
        (if bare (box :width 1.55))
        (header-toggle (str (+ i 1)) (str "mute-" i) #'t.muted
          :bg :control-on-bg :active-bg :sequencer-toggle-off-bg
          :color :control-on-fg :active-color :gray
          :on-click (track-toggle-click t (lambda () (toggle! t.muted))))
        (header-toggle "S" (str "solo-" i) #'t.soloed
          :on-click (track-toggle-click t (lambda () (toggle! t.soloed))))
        (box :width 8.6 :height 1
          :key (str "select-" i)
          :background-color :transparent
          :on-click (lambda (event) (track-click event t))
          :on-double-click (lambda (evt) (open-piano-roll-for-track t))
          ;; Lit (the plain look) while the track is heard: a binding
          ;; cannot be negated, so the silenced look is `:color`.
          (badge (track-name-display t.name)
            :key (str "track-name-label-" i)
            :icon (eseq.track-collapse/instrument-icon t.instrument-type)
            ;; The track color fills the device glyph (Logic-style),
            ;; dimmed while the track is silent.
            :track-r (dimmed-part c 0 true)
            :track-g (dimmed-part c 1 true)
            :track-b (dimmed-part c 2 true)
            :muted-track-r (rgb-part c 0)
            :muted-track-g (rgb-part c 1)
            :muted-track-b (rgb-part c 2)
            :font-size 11 :width 8.6 :height 1 :padding 0
            :h-align :left
            :background-color :transparent
            :border-color :transparent
            :highlight-color :transparent
            :shadow-color :transparent
            :muted #'t.audible
            :color (rgba 0.4 0.4 0.4 0.6)
            :muted-color :dim
            :bg :transparent))
        (box :width 0.5)
        (track-volume-control t)
        ;; Take-lane indicator (takes spec 10 UX): green = a take governs
        ;; the lane (steps dim, grid read-only); grey = the performer
        ;; latched the lane away — click returns it to the song; invisible
        ;; on pattern lanes. Always laid out — the bound take state only
        ;; repaints the widget, so pattern<->take flips never re-layout.
        (box :width 2 :height :fill
          :background "seqv-back-to-song-bg"
          :take-state #'t.governed
          (box :width 2 :height 1.5
            :background "seqv-back-to-song-icon"
            :key (str "back-to-song-" i)
            :take-state #'t.governed
            :on-click (lambda (event)
              (when (> t.governed take-none)
                (set! t.latched false)))))))))

(def track-actions (t)
  (h-stack :gap 0.35 :padding 0.85
    (box
      :key (str "expand-" t.index)
      :width 3.5 :height 1.0
      :background "seqv-ellipsis-button"
      :expanded (if t.expanded 1 0)
      :on-click (lambda (event)
        (track-control-click event t (lambda () (track-menu-click t)))))
    (box :width 0.1 :height 0.0)))

(def row-width 16)

;; The expanded step editor is a fixed-format 16-column control grid. Keeping
;; the dimensions named here lets the grid use an explicit row height without
;; duplicating geometry across the widget tree.
(def expanded-step-column-padding 0.25)
(def expanded-step-column-gap 0.5)
(def expanded-step-slider-height 4)
(def expanded-step-toggle-height 1.5)
(def expanded-step-label-height 1)
(def expanded-step-playhead-height 0.7)
(def expanded-step-row-height
  (+ (* 2 expanded-step-column-padding)
    expanded-step-slider-height
    expanded-step-toggle-height
    expanded-step-label-height
    expanded-step-playhead-height
    (* 3 expanded-step-column-gap)))

;; A step-cell gesture's track (nil between gestures) and, while dragging a
;; step's duration edge, that step.
(def drag-track nil)
(def duration-drag-source nil)

(def duration-edge? (evt)
  (let ((sx (get evt :sx)))
    (and (not (= sx nil)) (> sx 0.48))))

(def set-duration-from-drag (source step)
  (set! selection.track source.track)
  (seq-set-step-param source.index :duration (max 1 (min 32 (+ (- step source.index) 1)))))

;; Song-governed lanes are non-interactive (takes spec 10 UX): while the
;; arrangement holds launch authority the Seq grid is a dimmed read-only view
;; of the session pattern — edits would silently target a pattern the lane is
;; not playing.
(def grid-step-pointer-down (s evt)
  (let ((t s.track))
    (unless (song-governed? t)
      ;; Read before selecting: a selection applies on the current track only.
      (let ((use-selection t.selected)
            (edge-drag (and s.active
                            (not (sgi/selection-click? evt))
                            (duration-edge? evt))))
        (set! eseq.seq-core-state/selected-bus -1)
        (set! selection.track t)
        (set! drag-track t)
        (if edge-drag
          (do
            (set! duration-drag-source s)
            (sgi/step-clear-drag-state)
            (eseq.seq-core-state/cool-off-follow)
            (sgi/set-track-cursor-step s.index)
            (set-duration-from-drag s s.index))
          (sgi/step-pointer-down-for-track
            t.index s.index evt use-selection))))))

(def grid-step-drag (s evt)
  (let ((t s.track))
    (when (and (= drag-track t) (not (song-governed? t)))
      (if duration-drag-source
        (set-duration-from-drag duration-drag-source s.index)
        (do
          (set! selection.track t)
          (sgi/step-select-drag-over-for-track t.index s.index evt))))))

(def grid-step-double-click (s evt)
  (let ((t s.track))
    (unless (song-governed? t)
      (set! selection.track t)
      (sgi/step-double-click-for-track t.index s.index evt))))

(def grid-step-pointer-up (s evt)
  (let ((t s.track))
    (when (and (= drag-track t) (not duration-drag-source) (not (song-governed? t)))
      (set! selection.track t)
      (sgi/step-pointer-up s.index evt))
    (set! drag-track nil)
    (set! duration-drag-source nil)))

(def step-odd (step)
  (let ((odd1 (mod (floor (/ step 4)) 2))
      (odd2 (mod (floor (/ step 32)) 2)))
    (if (= odd2 1) (if (= odd1 1) 0 1) odd1)))

;; Step s of track t: the cursor frame around the step's shell.
(def step-cell (t s)
  (box
    :width step-cell-width :height step-cell-height
    :key (str "step-cell-" t.index "-" s.index)
    :on-mouse-down (lambda (evt) (grid-step-pointer-down s evt))
    :on-drag (lambda (evt) (grid-step-drag s evt))
    :on-mouse-up (lambda (evt) (grid-step-pointer-up s evt))
    :on-double-click (lambda (evt) (grid-step-double-click s evt))
    :background "seqv-step-cursor"
    :index s.index
    :track t
    :cursor #'grid-cursor.step
    (box
      :width step-cell-width :height step-cell-height
      :align :center
      :step s
      :track t
      :selected #'s.selected
      :color :sequencer-step-border
      :selected-color :sequencer-step-selected-border
      :off-fill (if (= (step-odd s.index) 1)
        :sequencer-step-off-fill-alt
        :sequencer-step-off-fill)
      :background "seqv-step-shell")))

(def playhead-row (t row)
  (box
    :key (str "playhead-row-" t.tid "-" row)
    :width (* row-width step-cell-width) :height 0.24
    :background "seqv-playhead-row-bar"
    :track t
    :row row))

;; Row `row`'s number beside a grid of `rows` rows (hidden for one row).
(def row-label (row rows)
  (box :height (track-row-label-height) :v-align :center
    (v-stack :gap 0
      (label (+ row 1)
        :color (if (> rows 1) :dim :buffer-bg)
        :v-align :center
        :width 0.1 :bg :transparent :font-size 8)
      (box :width 0.2 :height (* (track-row-label-height) 0.02)))))

;; Track t's step grid: 16 cells a row, the last row padded with plain
;; spacers (hit testing reaches the track row through them), a playhead bar
;; under each row. A row's number lights by its lamp (a repaint), so the
;; playhead moving on never re-renders a row.
(def track-grid (t)
  (let ((rows (chunks t.steps row-width))
        (count (max 1 (len rows))))
    (box :key (str "track-step-grid-" t.index) :padding 0.15
      (box :background-color :buffer-bg
        (v-stack :gap -0.04
          (box :width 0.1 :height 0.342 :bg :transparent)
          (each (range 0 count) |row|
            (let ((steps (if (< row (len rows)) (nth rows row) (list))))
              (v-stack :gap -0.16
                (box :background "seqv-row-lamp" :track t :row (if (> count 1) row -1)
                  (h-stack :align :center
                    (box :width 0.1)
                    (row-label row count)
                    (h-stack :gap 0.0
                      (each steps |s| (step-cell t s))
                      (each (range (len steps) row-width) |col|
                        (box :width step-cell-width :height step-cell-height)))))
                (h-stack (box :width 1)
                  (playhead-row t row))))))))))

;; ── The expanded step editor ──
;; One track's editor: a tab per built-in step param and a selector over its
;; process lanes (the param mode, `t.param-mode`), sixteen slots showing the
;; steps of one page (`shown-page`), the cursor step's value, the pages with
;; their bar transposes, and for a lane mode its strip and patchbay. Slots
;; bind their step's fields; the slot grid, the cursor picker and the pages
;; are subtrees of their own, so a page turn, a cursor move or a bar
;; transpose re-runs one of them, never the row.

;; The expanded editor's twin of `seqv-step-shell`: its gate, selection and
;; p-lock tick, without the duration span.
(defwidget seqv-slot-shell
  :width 1.5 :height 2.5
  :paint-margin 1
  :state (step track selected off-fill-r off-fill-g off-fill-b)
  :shader
  (eseq.sequencer/step-shell-shader step.active step.lock-kind step.variant-color))

;; A slot past the pattern's end: an off step's shell.
(defwidget seqv-slot-shell-off
  :width 1.5 :height 2.5
  :paint-margin 1
  :state (track selected off-fill-r off-fill-g off-fill-b)
  :shader
  (eseq.sequencer/step-shell-shader 0 0 (vec3 0 0 0)))

;; Expanded view twin of the grid's length underline: amber bar beneath the
;; number of step `index`, while a length lane last set `track`'s pattern
;; length to it.
(defwidget seqv-slot-length-mark
  :width 2.8 :height 0.3
  :state (track index)
  :shader
  (if (= track.length-step index)
    (sdf/layer
      (sdf/fill (sdf/rounded-rect (* 0.62 aspect) 0.5 0.2)
        (material :color (rgba 0.94 0.63 0.24 0.95))))
    (rgba 0 0 0 0)))

;; The page t's editor shows: the playhead's while the transport plays and
;; the editor follows it (no step selection), else its cursor's.
(def shown-page (t)
  (if (and transport.playing selection.auto-follow (empty? selection.steps)
           (>= t.playhead-page 0))
    (min t.playhead-page (- (track-pages t) 1))
    t.page))

;; Step `index` of track t, or nil past its pattern's end (`nth` reads nil
;; out of range).
(def step-at (t index) (nth t.steps index))

;; The step slot i shows on page `page` of track t, or nil.
(def slot-step (t page i)
  (step-at t (+ (* page slots-per-page) i)))

;; The current track's selected steps (instances of t, the current track).
(def selected-steps (t)
  (let ((steps t.steps))
    (map (lambda (i) (nth steps i))
      (filter (lambda (i) (< i (len steps))) (seq-selected-step-indexes-native)))))

(def drop-step-selection ()
  (when (seq-has-selection?) (seq-clear-selection)))

;; Put the shared step cursor on `step` of t. Not through
;; `sgi/set-track-cursor-step`, whose hook names the current track: right
;; after `select-track-for-edit` that read is still the old track until the
;; host pushes the new one, whose cursor would move too.
(def set-expanded-cursor (t step)
  (eseq.seq-core-state/set-cursor-step-value step)
  (cursor-step-changed t.index step))

;; Select t for edit and put its cursor on `step`, as every editor edit does.
(def focus-step (t step)
  (select-track-for-edit t)
  (eseq.seq-core-state/cool-off-follow)
  (set-expanded-cursor t step))

(def expanded-step-click (t s evt)
  (focus-step t s.index)
  (if (sgi/selection-click? evt)
    (sgi/step-select-drag-start s.index evt)
    (seq-clear-selection)))

(def expanded-step-drag (t s evt)
  (sgi/step-select-drag-over-for-track-no-cursor t.index s.index evt))

(def expanded-step-pointer-down (t s evt)
  ;; Read before selecting: a selection applies on the current track only.
  (let ((use-selection t.selected))
    (select-track-for-edit t)
    (set-expanded-cursor t s.index)
    (sgi/step-pointer-down-for-track t.index s.index evt use-selection)))

(def expanded-step-pointer-up (t s evt)
  (select-track-for-edit t)
  (set-expanded-cursor t s.index)
  (sgi/step-pointer-up s.index evt))

(def expanded-step-double-click (t s evt)
  (select-track-for-edit t)
  (sgi/step-double-click-for-track t.index s.index evt))

;; ── Lane slider steps ──
;; UI-only slider step per process lane. The process keeps floats; this only
;; quantizes what the expanded sliders and the row picker write, so a lane
;; whose range is wider than 1 (acc, tacc) moves in whole numbers by default.
;; 0..1 lanes stay floats (int/gate lanes already round via :decimals 0).

(def lane-step-key (lane) (str lane.process.proc-id "-" lane.inlet))

(def lane-stepped? (lane) (> (- lane.max lane.min) 1))

(def lane-slider-step (lane)
  (if (lane-stepped? lane)
    (let ((entry (find-by-key lane-edit.steps :key (lane-step-key lane))))
      (if entry (get entry :step) 1))
    (if (= lane.decimals 0) 1 0)))

(def set-lane-slider-step (lane step)
  (let ((key (lane-step-key lane)))
    (set! lane-edit.steps
      (cons (dict :key key :step step)
            (filter (lambda (entry) (not (= (get entry :key) key))) lane-edit.steps)))))

(def lane-slider-step-quantize (lane value)
  (let ((step (lane-slider-step lane))
        (lo lane.min)
        (hi lane.max))
    (if (> step 0)
      (min hi (max lo (+ lo (* step (round (/ (- value lo) step))))))
      value)))

;; Display precision that matches the step: 1 -> "3", 0.5 -> "1.5", 0.25 -> "0.75".
(def lane-slider-step-decimals (step)
  (if (= step 0) 2
    (if (= step (round step)) 0
      (if (= (* step 10) (round (* step 10))) 1 2))))

;; The row picker's step and precision in mode `mode` of t (`lane` its lane,
;; or nil).
(def expanded-param-step (lane)
  (if lane (lane-slider-step lane) 0))

(def expanded-param-decimals (t mode lane)
  (if lane
    (lane-slider-step-decimals (lane-slider-step lane))
    (tp/seqv-track-param-decimals t mode)))

;; ── Editor edits ──

;; A slot's slider moved to `position` on step s of t: the step's own value,
;; or every selected step's when s is one of them.
(def set-expanded-step-param (t s mode position)
  (focus-step t s.index)
  (let ((lane (tp/seqv-track-process-lane t mode)))
    (if (tp/seqv-process-lane-mode? mode)
      (when lane
        (let ((v (lane-slider-step-quantize lane
                   (tp/seqv-track-step-slider-param-value t mode position))))
          (if (sgi/step-selected? s.index)
            (set-lane-steps! lane (selected-steps t) v)
            (do (drop-step-selection)
                (set-lane-steps! lane (list s) v)))))
      (let ((v (tp/seqv-track-step-slider-param-value t mode position)))
        (if (sgi/step-selected? s.index)
          (seq-set-step-param-plock (tp/seqv-param-keyword mode) v)
          (do (drop-step-selection)
              (tp/seqv-set-step-value! s mode v)))))))

;; The row's number picker is one control for the whole row: with a step
;; selection it writes every selected step, otherwise the cursor step.
(def set-expanded-current-param (t mode value)
  (let ((s (step-at t (track-cursor t)))
        (lane (tp/seqv-track-process-lane t mode)))
    (when s
      (focus-step t s.index)
      (if (tp/seqv-process-lane-mode? mode)
        (when lane
          (set-lane-steps! lane
            (if (seq-has-selection?) (selected-steps t) (list s))
            (lane-slider-step-quantize lane (tp/seqv-track-step-param-value t mode value))))
        (let ((v (tp/seqv-track-step-param-value t mode value)))
          (if (seq-has-selection?)
            (seq-set-step-param-plock (tp/seqv-param-keyword mode) v)
            (tp/seqv-set-step-value! s mode v)))))))

(def goto-page (t page)
  (focus-step t (min (* page slots-per-page) (- (max 1 t.num-steps) 1))))

(def resize-pattern (t action)
  (select-track-for-edit t)
  (eseq.seq-core-state/cool-off-follow)
  (action)
  (sync-all-track-cursors-to-global))

;; ── Param tabs and the lane selector ──

(def param-header-name (t mode)
  (if (tp/seqv-process-lane-mode? mode)
    (clip-label (tp/seqv-track-param-name t mode) 28)
    (tp/seqv-track-param-name t mode)))

(def param-header-width (mode)
  (if (tp/seqv-process-lane-mode? mode) 17.8 6.4))

;; Step-param name a process port binds to when a tab is clicked while a
;; process map is armed. Names resolve through the scheduler's step-param
;; table, so they must be the long forms.
(def param-tab-step-param (mode)
  (match mode
    0 "velocity"
    1 "duration"
    3 "transpose"
    4 "pan"
    5 "sync"
    6 "delay"
    7 "retrig"
    8 "rate"
    _ ""))

;; The process of track t whose proc-id is `id`, or nil.
(def process-of (t id)
  (first (filter (lambda (p) (= p.proc-id id)) t.processes)))

;; The port the process map has armed, while it is on track t, or nil.
;; COMPAT(eseq-0l17.14): eseq.effects.param-controls' process map holds the
;; port by track position, proc-id and name.
(def armed-port (t)
  (when (and (pc/process-map-active?) (= process-map.track t.index))
    (let ((p (process-of t process-map.instance-id)))
      (when p (named p.ports process-map.port)))))

;; A process map armed on t whose port can take a step param.
(def param-tab-map-armed? (t)
  (and (pc/process-map-active?)
       (= process-map.track t.index)
       (or (= process-map.target-kind "")
           (= process-map.target-kind "step-param"))))

;; Whether a lane edit on p writes the shared project slot: the scope chip
;; says all tracks and p is a project lane.
(def edit-all? (p)
  (and lane-edit.all p.project))

;; Bind the armed port to mode's step param (a fan-out entry when the port is
;; bound already). Disarm first: a failing bind must never leave the tabs
;; stuck in the armed tint; its error reaches the status line.
(def param-tab-bind (t mode)
  (let ((pt (armed-port t))
        (param (param-tab-step-param mode)))
    (pc/process-map-clear)
    (when pt
      (let ((add (= pt.status "bound"))
            (all (edit-all? pt.process)))
        (if add
          (add-fanout! pt param :all all)
          (bind-port! pt param :all all))
        (status (str (if add "Added fan-out → " "Mapped process port → ") param
                     (if all " (all tracks)" "")))))))

(def param-tab (t mode tab-label)
  (let ((armed (param-tab-map-armed? t))
        (current (= t.param-mode mode)))
    (box :width 7.8 :height 2
      :key (str "expanded-param-tab-" t.tid "-" mode)
      :bg (if current (tp/seqv-param-color mode) :dark-gray)
      :background-color (if armed :process-map-arm-bg (rgba 0 0 0 0))
      :corner-radius (eseq.seq-core-state/radius 6)
      :on-click (lambda (event)
        (if armed
          (param-tab-bind t mode)
          (do (select-track-for-edit t) (set-track-param-mode t mode))))
      (label tab-label :font-size 12
        :color (if armed :process-lane-accent (if current :primary :dim))
        :bg :transparent))))

;; Lane l's selector label. Default project lanes read like the built-in
;; step params ("prob", "acc A"); a lane the user added to this track reads
;; as its minted instance name ("grab 2"), qualified by the inlet only when
;; the process carries more than one lane. Anything unnamed keeps the
;; numbered class/inlet form.
(def process-lane-option-label (l)
  (let ((p l.process))
    (if p.default-lane
      l.short-label
      (if (and p.roster (not (= p.instance-name "")))
        (if (> (len p.lanes) 1) (str p.instance-name " " l.inlet) p.instance-name)
        (str (+ l.position 1) " " l.short-label)))))

(def process-lane-options (t)
  (cons "none" (map process-lane-option-label t.lanes)))


(def select-process-lane-option (t label)
  (let ((lane (first (filter (lambda (l) (= (process-lane-option-label l) label)) t.lanes))))
    (select-track-for-edit t)
    (if lane
      (set-track-param-mode t (+ tp/seqv-process-lane-mode-offset lane.position))
      (when (tp/seqv-process-lane-mode? (track-param-mode t))
        (set-track-param-mode t 3)))))

;; t's lane selector, on `lane` (the lane its editor shows, or nil).
(def process-lane-selector (t lane)
  (dropdown
    :value (if lane (process-lane-option-label lane) "none")
    :key (str "expanded-process-lane-selector-" t.tid)
    :options (process-lane-options t)
    :on-change (lambda (v) (select-process-lane-option t v))
    :width 10.8 :height 1.45 :font-size 10))

;; ---------------------------------------------------------------------------
;; Lane strip: the selected process lane's two ends (docs/default-process-lanes-spec.md).
;;
;; IN   the lane's own values, or a wire from another lane's `wire` port
;; MODE accumulate | pass, for slots with a `mode` inlet
;; OUT  the mappable port's target plus the map button (arms the same
;;      process-map state the fx panel uses; step tabs, other lanes and
;;      device params all light up as targets)
;; plus the slot's scalar inlets (source, lag, lo, hi) and reorder buttons.
;;
;; Edit scope for project lanes (`lane-edit.all`): "this track" forks the
;; slot for this track only (bindings, mode, lo/hi, lane values); "all
;; tracks" writes the shared slot that every track inherits. Cirklon's
;; per-track aux config by default, the global effect on purpose.

(def lane-toggle-edit-scope ()
  (toggle! lane-edit.all))

(def lane-scope-chip ()
  (button (if lane-edit.all "all tracks" "this track")
    :key "lane-edit-scope-toggle"
    :height 1.0 :padding 0.2 :font-size 7.5
    :background-color (if lane-edit.all :process-lane-accent :transparent)
    :border-color :process-lane-accent
    :color (if lane-edit.all :black :dim)
    :on-click (lambda (event) (lane-toggle-edit-scope))))

;; Bypass toggle for process `id` of bay ns (`enabled` its state now). A
;; project lane forks this track only (the shared slot keeps running
;; elsewhere) unless the scope chip says "all tracks"; roster lanes and a
;; graph node's processes are their owner's alone. Per pattern, undoable,
;; like every other slot edit.
(def lane-toggle-enabled (ns id enabled)
  (if (lane-patch-node? ns)
    (node-edit! ns (lambda (graph node) (graph-node-process-enable graph node id (not enabled))))
    (let ((p (bay-process ns id)))
      (when p (set-process-enabled! p (not enabled) :all (edit-all? p))))))

(def lane-strip-enable-button (p)
  (button (if p.enabled "on" "off")
    :key (str "lane-enable-" p.proc-id)
    :width 2.2 :height 1.0 :padding 0.2 :font-size 7.5
    :background-color (if p.enabled :process-lane-accent :transparent)
    :border-color :process-lane-accent
    :color (if p.enabled :black :dim)
    :on-click (lambda (event) (lane-toggle-enabled p.track.index p.proc-id p.enabled))))

;; The dot in a patch-bay card: filled while the process runs. An SDF circle
;; rather than a rounded box, which reads as a square at this size (the same
;; accent literal as `lane-chip`).
(defwidget lane-patch-enable-dot-shape
  :width 1.4 :height 0.8
  :state (active)
  :shader
  (sdf/fill (sdf/circle 0.62)
    (material :color
      (if (> active 0.5)
        (rgba 0.94 0.63 0.24 1.0)
        (rgba 0.5 0.5 0.5 0.84)))))

(def lane-patch-enable-dot (ns entry)
  (box :width 1.4 :height 0.8 :padding 0
    :background "lane-patch-enable-dot-shape"
    :key (str "lane-patch-enable-" (get entry :instance-id))
    :active (if (get entry :enabled) 1 0)
    :on-click (lambda (event)
      (lane-toggle-enabled ns (get entry :instance-id) (get entry :enabled)))))

;; The process of p's track whose `wire` port is wired to lane l, or nil.
(def lane-writer (t l)
  (first (filter (lambda (q)
                   (let ((wire (named q.ports "wire")))
                     (and wire
                          (= wire.target-process l.process)
                          (= wire.target-inlet l.inlet))))
           t.processes)))

;; What port (or fan-out entry) x writes, as the strip names it: a step
;; param, the process it wires into, or its own label.
(def target-label (x)
  (if (not (= x.target-step-param ""))
    x.target-step-param
    (if x.target-process x.target-process.name x.target)))

(def lane-chip (text filled dim)
  ;; No border: a thin SDF border on a small rounded box floods it with the
  ;; border color.
  (box :height 1.1 :padding 0.25 :corner-radius (eseq.seq-core-state/radius 6)
    :background-color (if filled
                        (if dim (rgba 0.94 0.63 0.24 0.45) :process-lane-accent)
                        (rgba 0.94 0.63 0.24 0.14))
    (label text :font-size 9
      :color (if filled :black (if dim :dim :process-lane-accent))
      :bg :transparent)))

(def lane-strip-row-label (text)
  (label text :v-align :center :width 2.4 :font-size 8 :color :dim :bg :transparent))

;; Chain order is fire order: a writer below its reader lands next fire.
(def lane-strip-in-row (t l)
  (let ((writer (lane-writer t l)))
    (h-stack :width :fill :gap 0.3 :align :center
      (lane-strip-row-label "IN")
      (if writer
        (lane-chip (str "← " writer.name) true (> writer.index l.process.index))
        (lane-chip "lane" false false)))))

(def lane-strip-mode-button (p text value)
  (let ((mode (named p.inlets "mode"))
        (current (if (> mode.value 0.5) 1 0)))
    (button text
      :key (str "lane-mode-" p.proc-id "-" value)
      :flex 1 :height 1.1 :padding 0 :font-size 8.5
      :background-color (if (= current value) :process-lane-accent :transparent)
      :border-color :transparent
      :color (if (= current value) :black :dim)
      :on-click (lambda (event) (set-inlet! mode value :all (edit-all? p))))))

(def lane-strip-mode-row (p)
  (if (named p.inlets "mode")
    (h-stack :width :fill :gap 0.15 :padding 0.1 :corner-radius (eseq.seq-core-state/radius 6)
      :background-color (rgba 0 0 0 0.25)
      (lane-strip-mode-button p "accumulate" 0)
      (lane-strip-mode-button p "pass" 1))
    (nothing)))

;; Fan-out rows: extra targets on the OUT port, each with its own lo..hi
;; that the port value is rescaled into (the slot's lo/hi is the source).
(def lane-fanout-range-picker (p pt fo bound)
  (number-picker
    :key (str "lane-fanout-" p.proc-id "-" pt.name "-" fo.index "-" bound)
    :value (if (= bound "lo") fo.lo fo.hi)
    :min -1000 :max 1000 :decimals 1
    :noui true :font-size 8.5 :text-color :white :text-align :right
    :on-change (lambda (value) (set-fanout! fo bound value :all (edit-all? p)))
    :width 3.5 :height 1.0))

(def lane-fanout-row (p pt fo)
  (h-stack :width :fill :gap 0.25 :align :center
    :key (str "lane-fanout-row-" p.proc-id "-" pt.name "-" fo.index)
    (box :width 2.4 :height 0.1)
    (lane-chip (str "→ " (target-label fo)) true false)
    (box :flex 1 :height 0.1)
    (lane-fanout-range-picker p pt fo "lo")
    (lane-fanout-range-picker p pt fo "hi")
    (button "×"
      :key (str "lane-fanout-remove-" p.proc-id "-" pt.name "-" fo.index)
      :width 1.2 :height 1.0 :padding 0 :font-size 9
      :background-color :transparent :border-color :transparent :color :dim
      :on-click (lambda (event) (remove-fanout! fo :all (edit-all? p))))))

(def mappable-port (p)
  (first (filter (lambda (pt) pt.mappable) p.ports)))

(def lane-fanout-rows (p)
  (let ((pt (mappable-port p)))
    (when pt (each pt.fanout |fo| (lane-fanout-row p pt fo)))))

;; × on a port that writes somewhere (its own binding or its class hint):
;; disconnects it outright, so the lane drives nothing until it is mapped
;; again. Fan-out rows have their own ×.
(def lane-port-unbind-button (p pt)
  (if (and (not pt.disconnected) (not (= pt.status "unbound")))
    (button "×"
      :key (str "lane-unbind-" p.proc-id "-" pt.name)
      :width 1.2 :height 1.0 :padding 0 :font-size 9
      :background-color :transparent :border-color :transparent :color :dim
      :on-click (lambda (event)
        (unbind-port! pt :all (edit-all? p))
        (pc/process-map-clear)
        (status (str "Disconnected " p.name " " pt.name
                     (if (edit-all? p) " (all tracks)" "")))))
    (box :width 1.2 :height 1.0)))

;; COMPAT(eseq-0l17.14): eseq.effects.param-controls' process map takes the
;; legacy slot and port shapes.
(def map-slot (p) (dict :instance-id p.proc-id))
(def map-port (pt) (dict :name pt.name :target-kind pt.target-kind))

(def lane-strip-out-row (t p)
  (let ((pt (mappable-port p)))
    (if pt
      (let ((armed (pc/process-map-port-active? t.index (map-slot p) (map-port pt))))
        (h-stack :width :fill :gap 0.3 :align :center
          (lane-strip-row-label "OUT")
          (lane-chip (str "→ " (if (= pt.status "unbound") "unbound" (target-label pt)))
            (not (= pt.status "unbound"))
            (= pt.status "hint"))
          (box :flex 1 :height 0.1)
          (button (if armed "mapping" "map")
            :key (str "lane-map-" p.proc-id)
            :width 5.2 :height 1.1 :padding 0 :font-size 8.5
            :background-color (if armed :process-lane-accent :transparent)
            :border-color :process-lane-accent
            :color (if armed :black :process-lane-accent)
            :on-click (lambda (event)
              (pc/process-map-arm-port t.index (map-slot p) (map-port pt))))
          (lane-port-unbind-button p pt)))
      (nothing))))

;; The `wire` port is what a lane-to-lane map binds (OTHER LANES while
;; mapping): the raw value feeds another lane's inlet. It has no hint and
;; is not the mappable OUT port, so it gets its own row while bound.
(def lane-strip-wire-row (p)
  (let ((pt (named p.ports "wire")))
    (if (and pt (= pt.status "bound"))
      (h-stack :width :fill :gap 0.3 :align :center
        (lane-strip-row-label "WIRE")
        (lane-chip (str "→ " (target-label pt)) true false)
        (box :flex 1 :height 0.1)
        (lane-port-unbind-button p pt))
      (nothing))))

;; Scalar inlets the strip edits in place. `mode` has its own row.
(def lane-strip-inlet-row (p i)
  (h-stack :width :fill :gap 0.3 :align :center
    :key (str "lane-inlet-" p.proc-id "-" i.name)
    (label i.name :flex 1 :font-size 8.5 :color :white :bg :transparent)
    (number-picker
      :key (str "lane-inlet-control-" p.proc-id "-" i.name)
      :value i.value
      :min i.min
      :max i.max
      :decimals i.decimals
      :noui true :font-size 9 :text-color :white :text-align :right
      :on-change (lambda (value) (set-inlet! i value :all (edit-all? p)))
      :width 4.6 :height 1.0)))

;; Track-typed inlets (grab's `source`) pick from the track list by name.
(def lane-track-option (index)
  (let ((t (track-at index)))
    (str (+ index 1) " " (if t t.name ""))))

(def lane-track-options ()
  (map (lambda (t) (lane-track-option t.index)) (tracks)))

(def lane-strip-track-inlet-row (p i)
  (h-stack :width :fill :gap 0.3 :align :center
    :key (str "lane-inlet-" p.proc-id "-" i.name)
    (label i.name :flex 1 :font-size 8.5 :color :white :bg :transparent)
    (dropdown
      :key (str "lane-inlet-track-" p.proc-id "-" i.name)
      :value (lane-track-option (floor i.value))
      :bg-color :mixer-strip-bg
      :badge-color :mixer-strip-selected-bg
      :border-color :mixer-strip-selected-bg
      :options (lane-track-options)
      :on-change (lambda (label)
        (set-inlet! i (max 0 (index-of (lane-track-options) label)) :all (edit-all? p)))
      :width 7.5 :height 1.1 :font-size 8.5)))

;; Enum inlets (cmp's `op`, roll's `rate`) pick from the definition's option
;; list. The inlet value is the option index, so the row maps label <-> index.
(def lane-enum-option (i index)
  (or (nth i.options index) ""))

(def lane-strip-enum-inlet-row (p i)
  (h-stack :width :fill :gap 0.3 :align :center
    :key (str "lane-inlet-" p.proc-id "-" i.name)
    (label i.name :flex 1 :font-size 8.5 :color :white :bg :transparent)
    (dropdown
      :key (str "lane-inlet-enum-" p.proc-id "-" i.name)
      :value (lane-enum-option i (floor i.value))
      :options i.options
      :on-change (lambda (label)
        (set-inlet! i (max 0 (index-of i.options label)) :all (edit-all? p)))
      :width 7.5 :height 1.1 :font-size 8.5)))

(def lane-strip-inlets (p)
  ;; `mode` has its own row; `reset` is written by the shared reset lane's
  ;; wire, never typed.
  (each (filter (lambda (i) (not (or (= i.name "mode") (= i.name "reset")))) p.inlets) |i|
    (match i.type
      "track" (lane-strip-track-inlet-row p i)
      "enum" (lane-strip-enum-inlet-row p i)
      _ (lane-strip-inlet-row p i))))

;; UI-only slider step (see lane-edit.steps): only lanes wider than 1 get
;; one. 0 reads "free" and hands the sliders back their floats.
(def lane-strip-step-row (l)
  (if (lane-stepped? l)
    (h-stack :width :fill :gap 0.3 :align :center
      :key (str "lane-step-row-" (lane-step-key l))
      (label "step" :flex 1 :font-size 8.5 :color :dim :bg :transparent)
      (number-picker
        :key (str "lane-step-control-" (lane-step-key l))
        :value (lane-slider-step l)
        :min 0 :max 16 :step 0.25 :drag-rows 64
        :decimals (lane-slider-step-decimals (lane-slider-step l))
        :value-labels '((0 "free"))
        :noui true :font-size 9 :text-color :dim :text-align :right
        :on-change (lambda (value) (set-lane-slider-step l value))
        :width 4.6 :height 1.0))
    (nothing)))

;; Reorder within the chain: move before the previous process, or after the
;; next.
(def chain-neighbor (t p delta)
  (nth t.processes (+ p.index delta)))

(def lane-strip-move-button (t p text delta)
  (let ((neighbor (chain-neighbor t p delta)))
    (button text
      :key (str "lane-move-" p.proc-id "-" delta)
      :width 1.6 :height 1.0 :padding 0 :font-size 9
      :background-color :transparent :border-color :transparent
      :color (if neighbor :dim (rgba 1 1 1 0.15))
      :on-click (lambda (event)
        (when neighbor
          (move-process! p (if (< delta 0) neighbor (chain-neighbor t p 2))))))))

;; Scope: the lane's state history on this track (one sample per fire): its
;; class's first state cell. Accumulators show their running value, rand its
;; held roll, count its count. Nothing until the lane has fired on this
;; track. A subtree of its own, so a fire re-runs the scope alone; it carries
;; the strip's row gap above it, so an empty scope takes no room.
(def lane-scope-bound (p name fallback)
  (let ((i (named p.inlets name)))
    (if i i.value fallback)))

(def lane-strip-scope-row (t p)
  (subtree :key (str "lane-scope-row-" t.tid "-" p.proc-id)
    (if (= p.class-name "lane-harmony")
      (v-stack :gap 0
        (box :height lane-strip-gap)
        (harmony-snap-meter (str "lane-harmony-meter-" t.index "-" p.proc-id) p 13.5))
      (let ((cell (first p.cells))
            (values (if cell cell.values '())))
        (if (> (len values) 0)
          (v-stack :width :fill :gap 0.15
            (box :height (- lane-strip-gap 0.15))
            (h-stack :width :fill :gap 0.3 :align :center
              (lane-strip-row-label "NOW")
              (label (fmt "{:.2}" (nth values (- (len values) 1)))
                :v-align :center
                :font-size 10 :color :process-lane-accent :bg :transparent)
              (box :flex 1 :height 0.1)
              (label cell.name :font-size 8 :color :dim :bg :transparent :v-align :center))
            (box :width :fill :height 2.6 :corner-radius (eseq.seq-core-state/radius 6) :padding 0.2
              :background-color (rgba 0 0 0 0.3)
              (linegraph
                :key (str "lane-scope-" t.index "-" p.proc-id)
                :width :fill :height :fill
                :values values
                :total-points 64
                :min (lane-scope-bound p "lo" 0)
                :max (lane-scope-bound p "hi" 1)
                :line-color :process-lane-accent
                :area true)))
          (nothing))))))


;; ── Harmony snap meter ──────────────────────────────────────────────────────
;; What `lane-harmony` did on its last fires, from its scope cells (see the
;; process's :state): a compressor-style bar of the move (±6 semitones from
;; the centre), the scale degree it came from and went to over the chord
;; root, the twelve degrees above that root (chord tones solid, key tones
;; faint, the landing lit, a moved-from note outlined), the tier of each side
;; and where the chord came from. Empty cells read as before the first fire.

(def harmony-degree-names (list "R" "b9" "9" "b3" "3" "11" "#11" "5" "b13" "13" "b7" "7"))
(def harmony-bit-values (list 1 2 4 8 16 32 64 128 256 512 1024 2048))
(def harmony-meter-miss (rgba 0.92 0.42 0.36 1))

(def harmony-bit? (mask pc)
  (>= (mod (floor (/ mask (nth harmony-bit-values pc))) 2) 1))

;; The history of process p's state cell `name` (newest last), empty for
;; none.
(def cell-history (p name)
  (let ((cell (named p.cells name)))
    (if cell cell.values '())))

;; The newest value of p's cell `name`, or `fallback`.
(def harmony-cell (p name fallback)
  (let ((values (cell-history p name)))
    (if (> (len values) 0) (nth values (- (len values) 1)) fallback)))

(def harmony-degree-name (pc root)
  (nth harmony-degree-names (mod (+ (- pc root) 12) 12)))

(def harmony-tier (score)
  (if (>= score 0.99) "chord" (if (>= score 0.6) "key" (if (>= score 0.3) "color" "clash"))))

(def harmony-source-label (kind)
  (match kind
    1 "src: pattern"
    2 "src: output"
    3 "src: neuron"
    _ "no source"))

(def harmony-signed (snap)
  (if (> snap 0) (str "+" (fmt "{:.0}" snap)) (fmt "{:.0}" snap)))

;; [      |===>    ]: a fill from the centre tick toward the move, 6 semitones
;; to each edge.
(def harmony-snap-bar (snap width)
  (let ((tick 0.1)
        (fill (* (min 6 (abs snap)) (/ (- width 0.1) 12)))
        (half (/ (- width 0.1) 2)))
    (box :width width :height 0.9 :padding 0 :corner-radius 3
      :background-color (rgba 0 0 0 0.35)
      (h-stack :gap 0 :align :center
        (box :width (- half (if (< snap 0) fill 0)) :height 0.9 :bg :transparent)
        (when (< snap 0)
          (box :width fill :height 0.56 :corner-radius 2 :background-color :process-lane-accent))
        (box :width tick :height 0.9 :background-color (rgba 1 1 1 0.45))
        (when (> snap 0)
          (box :width fill :height 0.56 :corner-radius 2 :background-color :process-lane-accent))))))

(def harmony-degree-cell (key i root chord key-mask in-pc out-pc snap)
  (let ((pc (mod (+ root i) 12))
        (landed (= pc out-pc))
        (moved-from (and (not (= snap 0)) (= pc in-pc))))
    (box
      :key (str key "-deg-" i)
      :width 1.06 :height 0.95 :padding 0 :corner-radius 2
      :h-align :center :v-align :center
      :background-color (if landed :process-lane-accent
                          (if (harmony-bit? chord pc) (rgba 1 1 1 0.30)
                            (if (harmony-bit? key-mask pc) (rgba 1 1 1 0.10)
                              (rgba 0 0 0 0.30))))
      :border-color (if moved-from harmony-meter-miss :transparent)
      :border-width (if moved-from 0.1 0)
      (label (nth harmony-degree-names i)
        :font-size 6.5 :h-align :center :v-align :center :bg :transparent
        :color (if landed :black
                 (if (harmony-bit? chord pc) :foreground :dim))))))

;; Process p's meter (a lane-harmony process, or nil before it is listed).
(def harmony-snap-meter (key p width)
  (let ((fired (and p (> (len (cell-history p "source-kind")) 0)))
        (kind (if fired (harmony-cell p "source-kind" 0) 0))
        (history (if fired (cell-history p "snap") '())))
    (if (= kind 0)
      (label (if fired "no source sounding: nothing to follow yet" "waiting for a fire")
        :width width :height 1.0 :font-size 8 :color :dim :bg :transparent)
      (let ((snap (harmony-cell p "snap" 0))
            (root (harmony-cell p "root" 0))
            (in-pc (harmony-cell p "in-pc" 0))
            (out-pc (harmony-cell p "out-pc" 0))
            (chord (harmony-cell p "chord-mask" 0))
            (key-mask (harmony-cell p "key-mask" 0))
            (in-score (harmony-cell p "in-score" 1))
            (out-score (harmony-cell p "out-score" 1)))
        (v-stack :gap 0.25 :width width
          (h-stack :gap 0.3 :align :center
            (label (if (= snap 0)
                     (str (harmony-degree-name out-pc root) " held")
                     (str (harmony-degree-name in-pc root) " -> " (harmony-degree-name out-pc root)))
              :width (- width 3.2) :height 1.1 :font-size 10 :bg :transparent
              :color (if (= snap 0) :foreground :process-lane-accent))
            (label (harmony-signed snap)
              :width 3.0 :height 1.1 :font-size 11 :h-align :right :bg :transparent
              :color (if (= snap 0) :dim :process-lane-accent)))
          (harmony-snap-bar snap width)
          (box :width width :height 1.5 :padding 0.1 :corner-radius 3
            :background-color (rgba 0 0 0 0.3)
            (linegraph
              :key (str key "-history")
              :width :fill :height :fill
              :values history
              :total-points (max 8 (len history))
              :min -6 :max 6
              :line-color :process-lane-accent
              :area false))
          (h-stack :gap 0.05 :align :center
            (each (range 0 12) |i|
              (harmony-degree-cell key i root chord key-mask in-pc out-pc snap)))
          (h-stack :gap 0.3 :align :center
            (label (if (= snap 0)
                     (harmony-tier out-score)
                     (str (harmony-tier in-score) " -> " (harmony-tier out-score)))
              :width (/ width 2) :height 0.9 :font-size 7.5 :color :dim :bg :transparent)
            (label (harmony-source-label kind)
              :width (- (/ width 2) 0.3) :height 0.9 :font-size 7.5 :h-align :right
              :color :dim :bg :transparent)))))))

;; ---------------------------------------------------------------------------
;; Lane patchbay (docs/default-process-lanes-spec.md, patchbay): every process
;; of a chain in fire order, cables between their connectable ports; the
;; cables, the drag and the cable click are the generic patch-port machinery
;; the mixer's mod ports use. A bay is addressed by its cable namespace `ns`:
;; a track's position, or a registered graph node's namespace (from
;; `lane-patch-node-base` up). Its cards are patch entries: a track's derived
;; from its processes (`track-bay-entries`), a node's from the
;; `graph-node-lane-patch` native. Cable ids: an out port is
;; `(ns * 4096 + process index) * 16 + ordinal` (its place among the
;; process's connectable ports), an in port (process index, ordinal among
;; `p.in-ports`). The namespace is folded in because every expanded track's
;; patchbay shares one layout and the cable renderer keys sources by that
;; number alone.

(def lane-patch-show (on) (set! patch-view.show on))

;; Node patches (docs/graph-node-processes-spec.md §6): the same patchbay
;; drawn over a graph node's process chain. A node bay's namespace is
;; registered by the view that expands the node, so the bay finds the graph
;; node behind it. The host derives a node's namespace from its graph
;; (instance) id and the node (`graph-node-patch-namespace`), so node k of two
;; graphs never shares one (docs/instance-kinds-spec.md §7).
(def lane-patch-node-base 1024)
(def lane-patch-node? (ns) (>= ns lane-patch-node-base))
(def lane-patch-node-namespace (graph node) (graph-node-patch-namespace graph node))

(def lane-patch-node-touch () (set! patch-view.node-version (+ patch-view.node-version 1)))
(def lane-patch-node-version-value () patch-view.node-version)
(def lane-patch-node-selected-id () patch-view.node-selected)
(def lane-patch-node-select (id) (set! patch-view.node-selected id))

;; Call from the event that expands node `node` of instance `graph` (never
;; from a render): returns the namespace to draw the bay with.
(def lane-patch-register-node (graph node)
  (let ((ns (lane-patch-node-namespace graph node)))
    (set! patch-view.node-targets
      (cons (list ns graph node)
            (filter (lambda (target) (not (= (nth target 0) ns))) patch-view.node-targets)))
    (patch-idle!)
    ns))

;; Whether `graph` (an instance) has registered a node bay: its kind hosts
;; node editors, with the `expanded-node` view field the *processes* dock
;; reads (ui/processes-buffer.lisp).
(def lane-patch-node-host? (graph)
  (listed? graph (map (lambda (target) (nth target 1)) patch-view.node-targets)))

(def node-target (ns)
  (first (filter (lambda (target) (= (nth target 0) ns)) patch-view.node-targets)))

;; The instance and node index behind node bay ns.
(def node-bay-graph (ns) (let ((target (node-target ns))) (when target (nth target 1))))
(def node-bay-index (ns) (let ((target (node-target ns))) (if target (nth target 2) 0)))

;; Edit node bay ns's chain with `edit` (called with the instance and the
;; node index). COMPAT(eseq-0l17.20): a node bay reads and edits its chain
;; through the graph-node-process-* natives (applied at once) and bumps
;; `patch-view.node-version`, as the *processes* dock and alez.neural's node
;; inspector read the chain through them too.
(def node-edit! (ns edit)
  (edit (node-bay-graph ns) (node-bay-index ns))
  (lane-patch-node-touch))

;; The graph node instance behind node bay ns, or nil.
(def node-of (ns)
  (let ((target (node-target ns))
        (g (when target (graph-of (nth target 1)))))
    (when g (nth g.nodes (nth target 2)))))

;; The node process with proc-id `id` among the registered node bays, or nil.
(def node-process (id)
  (first (reduce |found target|
           (if (empty? found)
             (let ((n (node-of (nth target 0))))
               (if n (filter (lambda (p) (= p.proc-id id)) n.processes) found))
             found)
           '()
           patch-view.node-targets)))

;; Process `id` of track bay ns, or nil (a node bay's processes are edited
;; through the natives).
(def bay-process (ns id)
  (unless (lane-patch-node? ns)
    (let ((t (track-at ns)))
      (when t (process-of t id)))))

(def port-id (ns slot ordinal) (+ (* (+ (* ns 4096) slot) 16) ordinal))
(def lane-patch-port-ns (port-id) (floor (/ port-id (* 16 4096))))
(def lane-patch-port-slot (ns port-id) (- (floor (/ port-id 16)) (* ns 4096)))
(def lane-patch-port-ordinal (port-id) (- port-id (* 16 (floor (/ port-id 16)))))

;; Port `id`'s place as the port shader compares it with the pending port
;; (`patch-view.pending-bay` / `pending-slot`): its bay's namespace and its
;; slot and ordinal in the bay (`id mod 4096·16`). A shader holds floats, so
;; a whole port id (past 2^24 in a node bay) would round onto its
;; neighbours: the namespace is folded below 2^24 (only node bays of graphs
;; 4096 apart share a fold).
(def port-bay (id) (mod (lane-patch-port-ns id) 16777216))
(def port-slot (id) (mod id 65536))

(def connectable-ports (p) (filter (lambda (pt) pt.connectable) p.ports))

;; The inlet (a lane or a scalar inlet) of process p named `name`: what a
;; cable into its in port binds.
(def wire-target (p name)
  (let ((lane (first (filter (lambda (l) (= l.inlet name)) p.lanes))))
    (if lane lane (named p.inlets name))))

;; The reader entries of port pt (its own wire unless disconnected, then its
;; fan-out entries' wires), as patch entries list them.
(def port-readers (pt)
  (append
    (if (and pt.target-process (not pt.disconnected))
      (let ((r pt.target-process))
        (list (dict :slot-index r.index :instance-id r.proc-id :inlet pt.target-inlet
                    :source "primary" :fanout-index nil)))
      '())
    (map (lambda (fo)
           (let ((r fo.target-process))
             (dict :slot-index r.index :instance-id r.proc-id :inlet fo.target-inlet
                   :source "fanout" :fanout-index fo.index)))
      (filter (lambda (fo) fo.target-process) pt.fanout))))

;; Process p's out ports in bay ns: its connectable ports, each with its
;; cable id, whether a new cable takes its own binding (none of its own, or
;; disconnected) and its readers.
(def process-out-ports (ns p)
  (let ((ports (connectable-ports p)))
    (map (lambda (ordinal)
           (let ((pt (nth ports ordinal)))
             (dict :name pt.name :ordinal ordinal :port-id (port-id ns p.index ordinal)
                   :primary-free (or pt.disconnected (not pt.manual))
                   :readers (port-readers pt))))
      (range 0 (len ports)))))

;; Track t's bay entries: its processes in fire order, in the patch entry
;; shape a node bay's `graph-node-lane-patch` gives (the cards draw both).
;; The cables are listed once, `(reader slot, inlet, out port id)` in out
;; port order, and each in port takes the out ports of its own.
(def track-bay-entries (t)
  (let ((ns t.index)
        (processes t.processes)
        (outs (map (lambda (p) (process-out-ports ns p)) processes))
        (cables (reduce |acc ports|
                  (reduce |acc port|
                    (append acc (map (lambda (r) (list (get r :slot-index) (get r :inlet)
                                                       (get port :port-id)))
                                  (get port :readers)))
                    acc ports)
                  '() outs))
        (writers-of (lambda (index inlet)
                      (map (lambda (c) (nth c 2))
                        (filter (lambda (c) (and (= (nth c 0) index) (= (nth c 1) inlet)))
                          cables)))))
    (map (lambda (p)
           (dict :slot-index p.index :instance-id p.proc-id :name p.name
                 :class p.class-name :project p.project :enabled p.enabled
                 :expr p.expr :expr-line p.expr-line
                 :compile-error (if (= p.compile-error "") nil p.compile-error)
                 :in-ports (map (lambda (ordinal)
                                  (let ((inlet (nth p.in-ports ordinal)))
                                    (dict :name inlet :ordinal ordinal
                                          :writers (writers-of p.index inlet))))
                             (range 0 (len p.in-ports)))
                 :out-ports (nth outs p.index)))
      processes)))

;; Bay ns's entries (none while its owner is gone).
(def bay-entries (ns)
  (if (lane-patch-node? ns)
    (let ((graph (node-bay-graph ns)))
      (if graph
        (do patch-view.node-version (graph-node-lane-patch graph (node-bay-index ns)))
        '()))
    (let ((t (track-at ns)))
      (if t (track-bay-entries t) '()))))

(def lane-patch-list (entry key)
  (or (and entry (get entry key)) '()))

;; Bay entries and their ports by position (`nth` reads nil out of range).
(def lane-patch-out-port (entries ns port-id)
  (nth (lane-patch-list (nth entries (lane-patch-port-slot ns port-id)) :out-ports)
       (lane-patch-port-ordinal port-id)))

(def lane-patch-in-port (entries slot ordinal)
  (nth (lane-patch-list (nth entries slot) :in-ports) ordinal))

;; The reader entry of an out port that lands on (reader slot, inlet), or nil.
(def lane-patch-reader (out-port reader-slot inlet)
  (first (filter (lambda (reader) (and (= (get reader :slot-index) reader-slot)
                                       (= (get reader :inlet) inlet)))
           (lane-patch-list out-port :readers))))

;; A cable pointing up the chain lands next fire (writes only flow forward
;; within one fire).
(def lane-patch-in-port-backward? (ns slot port)
  (not (empty? (filter (lambda (writer) (> (lane-patch-port-slot ns writer) slot))
                 (lane-patch-list port :writers)))))

(def lane-patch-port-style
  (ui/style
    :hover (dict
      :brightness 1.45
      :transition (dict :brightness 0.08 :ease :smoothstep))))

;; A patchbay port. An out port draws pending while it is the pending port:
;; `armed-bay` / `armed-slot` bind the pending port's place
;; (`patch-view.pending-bay` / `pending-slot`), compared with its own
;; (`bay-key`, `slot-key`), so arming a port repaints the ports alone.
;; `pending-port` (bound to `patch-view.pending`) is for the patch machinery,
;; which compares it with the port's id (`:track`) exactly; the shader does
;; not read it (a float would round a node bay's ids together).
(defwidget lane-patch-port
  :width 1.5 :height 1.0
  :paint-margin 0.012
  :state (active output selected pending-port armed-bay armed-slot bay-key slot-key)
  :shader
  (let ((outer (if active
          (if selected
            :mod-port-selected
            (if output
              (if (= armed-slot slot-key)
                (if (= armed-bay bay-key) :mod-port-pending :mod-port-output)
                :mod-port-output)
              :mod-port-input))
          :mod-port-inactive))
      (inner (if active
          (if output :mod-port-output-inner :mod-port-input-inner)
          :mod-port-inactive-inner)))
    (sdf/layer
      (sdf/fill (sdf/circle width)
        (material :color outer))
      (sdf/fill (sdf/circle (* width 0.53))
        (material :color inner)))))

;; Make port `id` the pending one (-1: none), each field written only when
;; it changes.
(def set-pending! (id)
  (let ((bay (if (< id 0) -1 (port-bay id)))
        (slot (if (< id 0) -1 (port-slot id))))
    (unless (= patch-view.pending id) (set! patch-view.pending id))
    (unless (= patch-view.pending-bay bay) (set! patch-view.pending-bay bay))
    (unless (= patch-view.pending-slot slot) (set! patch-view.pending-slot slot))))

(def clear-cable! ()
  (when patch-view.cable (set! patch-view.cable nil)))

;; No port pending and no cable selected.
(def patch-idle! ()
  (set-pending! -1)
  (clear-cable!))

(def lane-patch-clear-pending () (set-pending! -1))

(def lane-patch-arm (port-id)
  (set-pending! port-id)
  (clear-cable!))

;; Wire out port `port-id` into (reader slot, in-port ordinal) of bay ns. The
;; first cable out of a port takes its primary binding (when it has none of
;; its own, or is disconnected); every further cable is a fan-out entry on
;; the port (identity range, so the value passes through).
(def lane-patch-connect (ns port-id reader-slot ordinal)
  (let ((entries (bay-entries ns))
        (writer-slot (lane-patch-port-slot ns port-id))
        (writer (nth entries writer-slot))
        (out-port (lane-patch-out-port entries ns port-id))
        (reader (nth entries reader-slot))
        (in-port (lane-patch-in-port entries reader-slot ordinal)))
    (set-pending! -1)
    (if (not (= (lane-patch-port-ns port-id) ns))
      (status "Cables stay within a track")
      (if (not (and writer out-port reader in-port))
        (status "Nothing to wire")
        (if (= writer-slot reader-slot)
          (status "A lane cannot feed itself")
          (if (lane-patch-reader out-port reader-slot (get in-port :name))
            (status "Already wired")
            (let ((writer-id (get writer :instance-id))
                  (reader-id (get reader :instance-id))
                  (port (get out-port :name))
                  (inlet (get in-port :name))
                  (primary (get out-port :primary-free))
                  (p (bay-process ns writer-id))
                  (r (bay-process ns reader-id))
                  (all (and p (edit-all? p))))
              (if (lane-patch-node? ns)
                (node-edit! ns (lambda (graph node)
                                 (if primary
                                   (graph-node-process-wire graph node writer-id port reader-id inlet)
                                   (graph-node-process-fanout-add graph node writer-id port
                                     reader-id inlet))))
                (let ((pt (when p (named p.ports port)))
                      (target (when r (wire-target r inlet))))
                  (when (and pt target)
                    (if primary
                      (bind-port! pt target :all all)
                      (add-fanout! pt target :all all)))))
              (status (str "Wired " (get writer :name) " → " (get reader :name) " " inlet
                           (if (< reader-slot writer-slot) " (next fire)" "")
                           (if all " (all tracks)" ""))))))))))

(def lane-patch-in-click (ns slot ordinal)
  (when (>= patch-view.pending 0)
    (lane-patch-connect ns patch-view.pending slot ordinal)))

;; Select the cable from out port `port-id` into (reader slot, ordinal) of
;; bay ns.
(def lane-patch-select-cable (ns port-id reader-slot ordinal)
  (let ((entries (bay-entries ns))
        (writer-slot (lane-patch-port-slot ns port-id))
        (out-port (when (= (lane-patch-port-ns port-id) ns)
                    (lane-patch-out-port entries ns port-id)))
        (in-port (lane-patch-in-port entries reader-slot ordinal))
        (reader (when (and out-port in-port)
                  (lane-patch-reader out-port reader-slot (get in-port :name)))))
    (when reader
      (set-pending! -1)
      (set! patch-view.cable
        (dict :ns ns
              :port-id port-id
              :writer-slot writer-slot
              :writer-id (get (nth entries writer-slot) :instance-id)
              :port (get out-port :name)
              :reader-slot reader-slot
              :ordinal ordinal
              :inlet (get in-port :name)
              :source (get reader :source)
              :fanout-index (get reader :fanout-index)))
      (status "Cable selected: × or Backspace removes it"))))

(def lane-patch-selected-sources (ns slot ordinal)
  (let ((cable patch-view.cable))
    (if (and cable
             (= (get cable :ns) ns)
             (= (get cable :reader-slot) slot)
             (= (get cable :ordinal) ordinal))
      (list (get cable :port-id))
      '())))

;; Remove `cable` (a patch-view.cable dict) while its writer still holds it.
;; The × chip passes the cable it rendered with: its click lands on
;; mouse-down, after the host's on-patch-miss for that same press has already
;; cleared the selection.
(def lane-patch-remove-cable (cable)
  (when cable
    (clear-cable!)
    (let ((ns (get cable :ns))
          (id (get cable :writer-id))
          (port (get cable :port))
          (index (get cable :fanout-index))
          (fanout (= (get cable :source) "fanout")))
      (if (lane-patch-node? ns)
        (node-edit! ns (lambda (graph node)
                         (if fanout
                           (graph-node-process-fanout-remove graph node id port index)
                           (graph-node-process-unwire graph node id port))))
        (let ((p (bay-process ns id))
              (pt (when p (named p.ports port))))
          (when pt
            (if fanout
              (let ((fo (nth pt.fanout index)))
                (when fo (remove-fanout! fo :all (edit-all? p))))
              (clear-port! pt :all (edit-all? p)))))))))

(def lane-patch-cable-selected? () (if patch-view.cable true false))
(def lane-patch-pending-port () patch-view.pending)

;; Backspace / Delete in the sequencer buffer: a selected cable goes first,
;; otherwise the keys keep deleting steps. Returns true when a cable went.
(def lane-patch-delete-selected ()
  (if patch-view.cable
    (do (lane-patch-remove-cable patch-view.cable) true)
    false))

(def lane-patch-out-port-widget (ns entry port)
  (let ((id (get port :port-id)))
    (lane-patch-port
      :key (str "lane-patch-out-" (get entry :instance-id) "-" (get port :name))
      :patch-port true
      :direction :out
      :track id
      :active true
      ;; The patch machinery's drag source (bound: compared with :track).
      :pending-port #'patch-view.pending
      :armed-bay #'patch-view.pending-bay
      :armed-slot #'patch-view.pending-slot
      :bay-key (port-bay id)
      :slot-key (port-slot id)
      :output true
      :selected false
      :style lane-patch-port-style
      :on-mouse-down (lambda (event) (lane-patch-arm id))
      :on-click (lambda (event) (lane-patch-arm id))
      :on-patch-cancel (lambda (source) (lane-patch-clear-pending))
      :on-patch-miss (lambda () (clear-cable!)))))

(def lane-patch-in-port-widget (ns entry port)
  (let ((slot (get entry :slot-index))
        (ordinal (get port :ordinal))
        (selected (lane-patch-selected-sources ns slot ordinal))
        (backward (lane-patch-in-port-backward? ns slot port)))
    (h-stack :height 1.1 :gap 0.1 :align :center
      :key (str "lane-patch-in-" (get entry :instance-id) "-" (get port :name))
      (lane-patch-port
        :key (str "lane-patch-in-port-" (get entry :instance-id) "-" (get port :name))
        :patch-port true
        :direction :in
        :dest-kind "lane"
        :dest slot
        :input ordinal
        :connected-sources (lane-patch-list port :writers)
        :selected-sources selected
        :active true
        :output false
        :selected (not (empty? selected))
        :style lane-patch-port-style
        :on-patch-drop (lambda (source dest input) (lane-patch-connect ns source dest input))
        :on-cable-click (lambda (source dest input) (lane-patch-select-cable ns source dest input))
        :on-click (lambda (event) (lane-patch-in-click ns slot ordinal))
        :on-mouse-up (lambda (event) (lane-patch-in-click ns slot ordinal)))
      (label (str (get port :name) (if backward " ↑" ""))
        :height 1.1 :font-size 8.5 :v-align :center
        :color (if backward :process-lane-accent :dim) :bg :transparent))))

;; Bay ns's selected process (its proc-id; nil for none): a node bay's
;; selected process, or the process of the lane a track's editor shows.
(def lane-patch-selected-id (ns)
  (if (lane-patch-node? ns)
    patch-view.node-selected
    (let ((t (track-at ns))
          (lane (when t (tp/seqv-track-process-lane t t.param-mode))))
      (when lane lane.process.proc-id))))

;; Whether process `id` is bay ns's selection.
(def lane-patch-lane-selected? (ns id)
  (= (lane-patch-selected-id ns) id))

;; Clicking a card selects its lane in the strip, the same as picking it in
;; the dropdown, so the card and the painted lane follow the patchbay; a node
;; bay's selection drives the *processes* dock (its inspector, and an expr
;; card's code tile; expr spec §7).
(def lane-patch-select-lane (ns id)
  (if (lane-patch-node? ns)
    (set! patch-view.node-selected id)
    (let ((t (track-at ns))
          (lane (when t (first (filter (lambda (l) (= l.process.proc-id id)) t.lanes)))))
      (when lane
        (select-track-for-edit t)
        (set-track-param-mode t (+ tp/seqv-process-lane-mode-offset lane.position))))))

;; ── Expr cards (docs/expr-process-spec.md §2) ──────────────────────────
;; The same uniform box as every card: the preview of its body rides the
;; out-port row, and the title row gains the error dot and an edit button
;; that opens the body's text buffer (eseq.expr-buffer).

;; COMPAT(eseq-0l17.20): alez.neural's node inspector addresses a node's
;; process by its id. The node process whose proc-id is `id`, for
;; `harmony-snap-meter` (its scope cells), or nil.
(def process-scope-cells-for (id) (node-process id))

;; The latest run error of the node process with proc-id `id` (a node slot
;; runs under its own id: `p.error`), or nil.
(def lane-patch-run-error (id)
  (let ((p (node-process id)))
    (when (and p (not (= p.error ""))) p.error)))

;; Why an expr card shows its error dot, or nil: a body with no compiled
;; class, a failed commit from its edit buffer, or a failed run. Commit and
;; run errors are node-bay only until track expr cards (eseq-waa9.18).
(def lane-patch-expr-error (ns entry)
  (let ((id (get entry :instance-id))
        (compile-error (get entry :compile-error)))
    (if compile-error
      compile-error
      (when (lane-patch-node? ns)
        (let ((committed (eseq.expr-buffer/commit-error (node-bay-graph ns) (node-bay-index ns) id)))
          (if committed committed (lane-patch-run-error id)))))))

;; Red, where the enable dot is orange and ports are orange/blue.
(defwidget lane-patch-error-dot-shape
  :width 1.2 :height 0.8
  :state (active)
  :shader
  (sdf/fill (sdf/circle 0.6)
    (material :color (if (> active 0.5) :toast-error (rgba 0 0 0 0)))))

;; Its own subtree: a run error changing on the scheduler repaints the dot,
;; not the bay.
(def lane-patch-expr-error-dot (ns entry)
  (subtree :key (str "lane-patch-expr-error-" (get entry :instance-id))
    (box :width 1.2 :height 0.8 :padding 0
      :background "lane-patch-error-dot-shape"
      :active (if (lane-patch-expr-error ns entry) 1 0))))

(def lane-patch-expr-edit-button (ns entry)
  (button "edit"
    :key (str "lane-patch-expr-edit-" (get entry :instance-id))
    :width 2.2 :height 0.8 :padding 0.05 :font-size 6.5
    :background-color :transparent :border-color :process-lane-accent
    :color :process-lane-accent
    :on-click (lambda (event)
      (lane-patch-select-lane ns (get entry :instance-id))
      (eseq.expr-buffer/open-node-slot (node-bay-graph ns) (node-bay-index ns)
        (get entry :instance-id) (get entry :slot-index)))))

;; The body as one clipped line (the host collapses its whitespace).
(def lane-patch-expr-preview-chars 13)
(def lane-patch-expr-preview (entry)
  (let ((line (get entry :expr-line)))
    (if (or (not line) (= line ""))
      "(empty)"
      (if (> (len line) lane-patch-expr-preview-chars)
        (str (substring line 0 (- lane-patch-expr-preview-chars 1)) "…")
        line))))

;; ── In-port overflow (expr spec §3.2, proposal (a)) ───────────────────────
;; A card fits three in ports. With more, it shows two plus a `+n` badge; the
;; rest are set in the selected-slot inspector only. A port a cable lands on
;; always shows, so no cable loses its end.
(def lane-patch-in-port-capacity 3)
(def lane-patch-port-wired? (port) (> (len (lane-patch-list port :writers)) 0))

(def lane-patch-visible-in-ports (entry)
  (let ((ports (lane-patch-list entry :in-ports)))
    (if (<= (len ports) lane-patch-in-port-capacity)
      ports
      ;; Every wired port, and the first unwired ones up to the capacity
      ;; less the badge.
      (let ((unwired (filter (lambda (port) (not (lane-patch-port-wired? port))) ports))
            (room (max 0 (- (- lane-patch-in-port-capacity 1) (- (len ports) (len unwired)))))
            (shown (map (lambda (i) (get (nth unwired i) :name))
                     (range 0 (min room (len unwired))))))
        (filter (lambda (port) (or (lane-patch-port-wired? port) (listed? (get port :name) shown)))
          ports)))))

(def lane-patch-hidden-in-port-count (entry)
  (- (len (lane-patch-list entry :in-ports)) (len (lane-patch-visible-in-ports entry))))

(def lane-patch-overflow-badge (entry)
  (let ((hidden (lane-patch-hidden-in-port-count entry)))
    (when (> hidden 0)
      (label (str "+" hidden)
        :key (str "lane-patch-in-overflow-" (get entry :instance-id))
        :height 1.1 :font-size 8 :v-align :center :color :dim :bg :transparent))))

;; ── Card delete + drag reorder ─────────────────────────────────────────
;; Right-click a card for its menu; drag a card onto another to move it
;; there (it takes the drop target's position, like the scene pills). Both
;; the track bay and graph-node bays route through here.

(def lane-patch-open-card-menu (ns entry event)
  (set! card-menu.bay ns)
  (set! card-menu.proc-id (get entry :instance-id))
  (set! card-menu.name (get entry :name))
  (open-menu! card-menu event))

(def lane-patch-entry-ids (ns)
  (map (lambda (entry) (get entry :instance-id)) (bay-entries ns)))

;; A track's lane selector is a position in t.lanes, so a reorder or delete
;; would leave it on whichever lane slides into its place. Point it at the
;; lane it was on in the new process order `ids` (proc-ids), at its
;; neighbour when that lane's process is gone, or back at transpose when no
;; lane is left (as picking "none" does). Node bays select by proc-id and
;; need nothing here.
(def lane-patch-reselect-lane (t ids)
  (let ((lane (tp/seqv-track-process-lane t t.param-mode)))
    (when lane
      (let ((moved (reduce |acc id|
                     (append acc (filter (lambda (l) (= l.process.proc-id id)) t.lanes))
                     '() ids))
            (index (index-of moved lane)))
        (set-track-param-mode t
          (if (empty? moved)
            3
            (+ tp/seqv-process-lane-mode-offset
               (if (>= index 0) index (min lane.position (- (len moved) 1))))))))))

(def lane-patch-remove-card (ns id)
  (patch-idle!)
  (if (lane-patch-node? ns)
    (do
      (when (= patch-view.node-selected id) (set! patch-view.node-selected -1))
      (node-edit! ns (lambda (graph node) (graph-node-process-remove graph node id))))
    (let ((p (bay-process ns id)))
      (when p
        (lane-patch-reselect-lane p.track
          (filter (lambda (other) (not (= other id))) (lane-patch-entry-ids ns)))
        (remove-process! p)))))

;; Move process `id` of bay ns to the place of process `target-id`.
;; (No core take / drop: the new order is spliced by index.)
(def lane-patch-move-card (ns id target-id)
  (let ((ids (lane-patch-entry-ids ns))
        (from (index-of ids id))
        (to (index-of ids target-id)))
    (unless (or (< from 0) (< to 0) (= from to))
      (patch-idle!)
      (if (lane-patch-node? ns)
        (node-edit! ns (lambda (graph node) (graph-node-process-move graph node id (- to from))))
        (let ((p (bay-process ns id))
              (rest (filter (lambda (other) (not (= other id))) ids))
              (moved (append (map (lambda (i) (nth rest i)) (range 0 to))
                             (list id)
                             (map (lambda (i) (nth rest i)) (range to (len rest))))))
          (when p
            (lane-patch-reselect-lane p.track moved)
            ;; Before the process now after it, or last.
            (move-process! p
              (when (< (+ to 1) (len moved)) (bay-process ns (nth moved (+ to 1)))))))))))

(def lane-patch-card-drop (ns event)
  (lane-patch-move-card ns
    (get (get event :payload) :instance-id)
    (get (get event :target) :instance-id)))

;; Each bay drags its own type, so a card never drops into another bay.
(def lane-patch-card-drag-type (ns) (str "lane-patch-card-" ns))

;; Mounted inside each bay; only the bay the menu was opened from draws it.
;; A subtree of its own: opening and closing it re-runs the menus alone.
(def lane-patch-card-context-menu (ns)
  (subtree :key (str "lane-patch-card-menu-run-" ns)
    (lane-patch-card-context-menu-body ns)))

(def lane-patch-card-context-menu-body (ns)
  (context-menu
    :is-open (and card-menu.open (= card-menu.bay ns))
    :anchor card-menu.at
    :on-close (lambda () (set! card-menu.open false))
    (menu-item (str "Delete " (if (= card-menu.name "") "process" card-menu.name))
      :key (str "lane-patch-card-menu-delete-" ns)
      :on-select (lambda (event)
        (set! card-menu.open false)
        (lane-patch-remove-card ns card-menu.proc-id)))))

;; One card; `selected-id` is the bay's selected proc-id
;; (`lane-patch-selected-id`).
(def lane-patch-column (ns entry selected-id)
  (let ((id (get entry :instance-id))
        (expr (get entry :expr)))
    (box :padding 0.4 :corner-radius (eseq.seq-core-state/radius 12)
      :key (str "lane-patch-col-" id)
      :drag-type (lane-patch-card-drag-type ns)
      :drag-modifier :none
      :drag-payload (dict :instance-id id)
      :drop-types (list (lane-patch-card-drag-type ns))
      :drop-meta (dict :instance-id id)
      :drop-hover-border-color :mixer-strip-selected-border
      :on-drop (lambda (event) (lane-patch-card-drop ns event))
      :on-right-click (lambda (event) (lane-patch-open-card-menu ns entry event))
      :background-color (if (get entry :enabled) (rgba 1 1 1 0.04) (rgba 1 1 1 0.015))
      :selected-background-color :mixer-strip-selected-bg
      :selected (= selected-id id)
      :selected-border-color :mixer-strip-selected-border
      :height 3
      :on-click (lambda (event) (lane-patch-select-lane ns id))
      (v-stack :width 10.0 :gap 0.0 :align :start
        (h-stack :width :fill :gap 0.3 :align :center
          (label (get entry :name) :flex 1 :font-size 8 :v-align :center
            :color (if (get entry :enabled) :process-lane-accent :dim) :bg :transparent)
          (when expr (lane-patch-expr-error-dot ns entry))
          (when (and expr (lane-patch-node? ns)) (lane-patch-expr-edit-button ns entry))
          (lane-patch-enable-dot ns entry))
        (h-stack :width :fill :height 0.8 :gap 0.4 :align :center
          (each (lane-patch-visible-in-ports entry) |port|
            (lane-patch-in-port-widget ns entry port))
          (lane-patch-overflow-badge entry))
        (h-stack :width :fill :height 0.8 :gap 0.4 :align :center
          (each (lane-patch-list entry :out-ports) |port|
            (lane-patch-out-port-widget ns entry port))
          (when expr
            (label (lane-patch-expr-preview entry)
              :key (str "lane-patch-expr-preview-" id)
              :flex 1 :height 1.1 :font-size 7.5 :v-align :center :color :dim :bg :transparent)))))))

;; ---------------------------------------------------------------------------
;; The patch bay's + box (eseq-53y7.3): appends one process instance to THIS
;; track's roster, so the added lane exists on the track in every scene while
;; its per-step values stay scene-locked. The box is a filterable
;; menu-button: typing narrows the classes and picking one adds it at once.

;; The default lane classes, in project-layer order (process.rs DEFAULT_LANES),
;; then lane classes that are only ever added per track (length).
(def lane-add-default-classes
  (list "lane-prob" "lane-reset" "lane-rand" "lane-count" "lane-acc"
        "lane-grab" "lane-cmp" "lane-veto" "lane-roll" "lane-length"))

;; The default lanes read better without the `lane-` prefix every
;; def-process name needs.
(def lane-add-label (name)
  (if (= (substring name 0 5) "lane-") (substring name 5) name))

;; Default lane classes first, in project-layer order, then every other
;; def-process in the library.
(def lane-add-options ()
  (append
    (filter (lambda (c) c)
      (map (lambda (name) (named process-library.classes name)) lane-add-default-classes))
    (filter (lambda (c) (not (listed? c.name lane-add-default-classes))) process-library.classes)))

;; The menu's rows, parallel to `lane-add-options`.
(def lane-add-labels ()
  (map (lambda (c) (lane-add-label c.name)) (lane-add-options)))

;; The menu hands back the row's label; map it back to its class.
(def lane-add-class-for-label (label)
  (first (filter (lambda (c) (= (lane-add-label c.name) label)) (lane-add-options))))

;; Add a process of class c to t's own lanes, and remember it so its lane is
;; selected (its strip opens) once the host lists it.
(def lane-add-pick (t c)
  (when c
    (patch-idle!)
    (set! lane-add.known (map (lambda (p) p.proc-id) t.processes))
    (set! lane-add.class c.name)
    (set! lane-add.track t)
    (add-process! t c)
    (status (str "Added " (lane-add-label c.name) " to this track (every scene)"))))

;; Once the track lists the process `lane-add-pick` added, stops waiting and
;; selects its lane, if it has one (inert while nothing is pending).
(effect-buffer "*lane-add-sync*"
  (let ((t lane-add.track)
        (added (when t (first (filter (lambda (p) (and (= p.class-name lane-add.class)
                                                         (not (listed? p.proc-id lane-add.known))))
                                  t.processes)))))
    (when added
      (set! lane-add.track nil)
      (unless (empty? added.lanes)
        (lane-patch-select-lane t.index added.proc-id)))
    nil))

;; The patch bay's add control, shared with graph-node patch bays: same
;; footprint as a lane cell, and a filterable menu so typing narrows the
;; classes and picking a row adds it at once. `on-pick` gets the row label.
(def lane-patch-add-menu (key text labels placeholder on-pick)
  (lane-patch-add-menu-grouped key text labels (list) placeholder on-pick))

;; `lane-patch-add-menu` with section headers: `headers` lists the indices
;; of `labels` that are headings (drawn dim, never picked), e.g. the node
;; bay's "expr presets" group (docs/expr-process-spec.md §6.1).
(def lane-patch-add-menu-grouped (key text labels headers placeholder on-pick)
  (menu-button
    :key key
    :debug-name "lane-patch-add"
    :icon text
    :options labels
    :headers headers
    :filterable true
    :filter-placeholder placeholder
    :width 10.8 :height 3 :font-size 10 :menu-min-width 22
    :corner-radius (eseq.seq-core-state/radius 12)
    :bg-color (rgba 0.28 0.20 0.11 1)
    :text-color :process-lane-accent
    :menu-bg :dropdown-menu-bg
    :menu-border-color :dropdown-menu-border
    :hover-bg :dropdown-hover-bg
    :on-change on-pick))

(def lane-patch-add-cell (t)
  (lane-patch-add-menu (str "lane-patch-add-" t.tid) "+  add lane"
    (lane-add-labels) "Filter lanes…"
    (lambda (label) (lane-add-pick t (lane-add-class-for-label label)))))

(def lane-patch-remove-button ()
  (let ((cable patch-view.cable))
    (button "× cable"
      :key "lane-patch-remove-cable"
      :height 1.0 :padding 0.2 :font-size 7.5
      :background-color :transparent :border-color :process-lane-accent
      :color :process-lane-accent
      :on-click (lambda (event) (lane-patch-remove-cable cable)))))

;; The patchbay sits under the step sliders and the lane strip, spanning the
;; expanded track: one box per lane in fire order (left fires first), so a
;; cable pointing left lands next fire (marked ↑ on the in port). Six cells a
;; row, read left to right then top to bottom; the + box (`add-cell`) counts
;; as a cell and follows the last lane onto a new row when needed. A subtree
;; of its own, so a cable, card or inlet edit re-runs the bay and not the
;; row it sits in; a port being armed only repaints.
(def lane-patch-grid (ns key add-cell)
  (subtree :key (str "lane-patchbay-run-" key)
    ;; An unkeyed root: the subtree's key would replace the bay's, which
    ;; tests find.
    (v-stack :width :fill
      (lane-patch-grid-body ns key add-cell))))

(def lane-patch-grid-body (ns key add-cell)
  (let ((entries (bay-entries ns))
        (columns 6)
        (cable patch-view.cable)
        (selected-id (lane-patch-selected-id ns))
        (count (+ (len entries) 1)))
    (v-stack :width :fill :gap 0.1 :padding 0.3
      :key (str "lane-patchbay-" key)
      (each (range 0 (ceil (/ count columns))) |row|
        (h-stack :width :fill :gap 0.2 :align :start
          :key (str "lane-patch-grid-row-" key "-" (* row columns))
          (each (range (* row columns) (min count (* (+ row 1) columns))) |index|
            (if (< index (len entries))
              (lane-patch-column ns (nth entries index) selected-id)
              add-cell))))
      (when (and cable (= (get cable :ns) ns)) (lane-patch-remove-button))
      (lane-patch-card-context-menu ns))))

;; A graph node's patch: the same cards, ports and cables over the node's
;; chain (docs/graph-node-processes-spec.md §6). `ns` comes from
;; `lane-patch-register-node`; `add-cell` is the host panel's add-process box.
(def lane-patchbay-node (ns add-cell)
  (lane-patch-grid ns ns add-cell))

;; Track t's patchbay, under a lane mode (`lane` its lane) while the bay
;; shows.
(def lane-patchbay-under (t lane)
  (when (and patch-view.show lane)
    (lane-patch-grid t.index t.tid (lane-patch-add-cell t))))

(def lane-strip-gap 0.35)

;; Track t's strip for `lane` (the lane its editor shows, or nil). A subtree
;; of its own: an inlet, wiring or mapping edit re-runs the strip, not the
;; row.
(def lane-strip (t lane)
  (when lane
    (subtree :key (str "lane-strip-run-" t.tid)
      ;; An unkeyed root: the subtree's key would replace the strip's, which
      ;; tests find.
      (h-stack (lane-strip-body t lane)))))

(def lane-strip-body (t lane)
  (let ((p lane.process))
    (box :width 20 :padding 0.5 :corner-radius (eseq.seq-core-state/radius 10)
      :key (str "lane-strip-" t.tid "-" p.proc-id)
      :background-color (rgba 1 1 1 0.04)
      :border-width 0.08 :border-color (rgba 1 1 1 0.08)
      (v-stack :width :fill :gap lane-strip-gap
        (h-stack :width :fill :gap 0.3 :align :center
          (label p.name
            :v-align :center
            :font-size 11 :color :process-lane-accent :bg :transparent)
          (box :flex 1 :height 0.1)
          (lane-strip-enable-button p)
          (if p.project
            (lane-scope-chip)
            (label "track lane" :v-align :center :font-size 7.5 :color :dim :bg :transparent))
          (lane-strip-move-button t p "▲" -1)
          (lane-strip-move-button t p "▼" 1))
        (lane-strip-in-row t lane)
        (lane-strip-mode-row p)
        (lane-strip-out-row t p)
        (lane-fanout-rows p)
        (v-stack :width :fill :gap 0
          (lane-strip-wire-row p)
          (lane-strip-scope-row t p))
        (lane-strip-inlets p)
        (lane-strip-step-row lane)))))

;; While a lane's map button is armed, the other lanes on the track offer
;; their inlets as wire targets (bound through the writer's `wire` port).
(def armed-wire-port (t)
  (let ((pt (armed-port t)))
    (when pt (named pt.process.ports "wire"))))

(def other-lane-chip (t wire l)
  (button l.short-label
    :key (str "other-lane-" t.index "-" l.process.proc-id "-" l.inlet)
    :height 1.1 :padding 0.25 :font-size 9
    :background-color :process-map-arm-bg
    :border-color :process-lane-accent
    :color :process-lane-accent
    :on-click (lambda (event)
      (let ((all (edit-all? wire.process)))
        (pc/process-map-clear)
        (bind-port! wire l :all all)
        (status (str "Wired → " l.short-label (if all " (all tracks)" "")))))))

;; The OTHER LANES row under t's editor, while a map is armed on t. A subtree
;; of its own, so arming a map re-runs it and not the expanded rows. It
;; carries the editor's row gap above it (the editor stacks it at gap 0), so
;; it takes no room while hidden.
(def other-lanes-row (t)
  (subtree :key (str "other-lanes-run-" t.tid)
    (v-stack :width :fill :gap 0
      (let ((wire (armed-wire-port t)))
        (when wire
          (v-stack :width :fill :gap 0
            (box :height 0.1)
            (h-stack :width :fill :gap 0.3 :align :center :padding 0.2
              (label "OTHER LANES" :font-size 8 :color :dim :bg :transparent)
              (each (filter (lambda (l) (not (= l.process wire.process))) t.lanes) |l|
                (other-lane-chip t wire l)))))))))

;; ── The editor ──

;; What every slot's slider shares in mode `mode` of t (`lane` its lane, or
;; nil): the ranges, the curve, the fill and the track color. Read once per
;; slot grid, outside the slots' own subtrees.
(def slot-slider-look (t mode lane)
  (let ((r (track-color-part t 0 false))
        (g (track-color-part t 1 false))
        (b (track-color-part t 2 false)))
    (dict :slider-min (tp/seqv-track-param-slider-min t mode)
          :slider-max (tp/seqv-track-param-slider-max t mode)
          :origin (tp/seqv-track-param-origin t mode)
          :min (tp/seqv-track-param-min t mode)
          :max (tp/seqv-track-param-max t mode)
          :pivot-position (tp/seqv-param-haptic-pivot-position mode)
          :pivot-value (tp/seqv-track-param-haptic-pivot-value t mode)
          :exponent (tp/seqv-param-haptic-exponent mode)
          :items (if (= mode 5) project.sync-options '())
          :fill (if (tp/seqv-process-lane-mode? mode) :process-lane-accent (rgba r g b 1.0))
          :r r :g g :b b)))

;; Slot i's slider on step s (nil past the pattern's end), at `value` (the
;; slider position) showing `haptic` (the value), lit by `active`, drawn by
;; `look` (`slot-slider-look`). Process lanes draw in the process accent so a
;; lane never reads as a built-in step param.
(def slot-vslider (t s mode i look value haptic active)
  (vslider :height expanded-step-slider-height
    :key (str "expanded-step-slider-" t.tid "-" i)
    :width (if (= mode 5) 2 1)
    :min (get look :slider-min) :max (get look :slider-max)
    :origin (get look :origin)
    :value value
    :haptic-value haptic
    :haptic-min (get look :min)
    :haptic-max (get look :max)
    :haptic-pivot-position (get look :pivot-position)
    :haptic-pivot-value (get look :pivot-value)
    :haptic-exponent (get look :exponent)
    :items (get look :items)
    :font-size 11
    :color :white
    :fill (get look :fill)
    :dot-color :dark-gray
    :active active
    :track-r (get look :r)
    :track-g (get look :g)
    :track-b (get look :b)
    :material (eseq.sequencer/step-slider-track-material)
    :on-change (lambda (v) (when s (set-expanded-step-param t s mode v)))))

;; Slot i's slider: a linear step param binds its field, so an edit only
;; repaints. A curved one (its slider position is a curve of the value)
;; reads its value in a subtree of its own, so an edit re-runs that slot; a
;; lane slot reads an element of `lane.values` the same way, so a lane edit
;; re-runs every lane slot (all sixteen read the one list).
(def slot-slider (t s mode lane look i)
  (if (or (not s) (and (tp/seqv-process-lane-mode? mode) (not lane)))
    (slot-vslider t s mode i look 0 0 (if s #'s.active false))
    (if (or lane (tp/seqv-param-curved? mode))
      (subtree :key (str "expanded-step-slider-run-" t.tid "-" i)
        ;; An unkeyed root: the subtree's key would replace the slider's,
        ;; which tests find.
        (h-stack
          (let ((v (if lane (nth lane.values s.index) (tp/seqv-step-value s mode))))
            (slot-vslider t s mode i look (tp/seqv-param-slider-position mode v) v #'s.active))))
      (let ((ref (tp/seqv-step-ref s mode)))
        (slot-vslider t s mode i look ref ref #'s.active)))))

;; Slot i's gate toggle: the slot twin of the compact grid's step shell, so
;; it shows the p-lock tick (variant colors included) and the gate, selection
;; and muted looks.
(def slot-toggle (t s i)
  (let ((off-fill (if (= (step-odd i) 1) :sequencer-step-off-fill-alt :sequencer-step-off-fill)))
    (if s
      (box
        :key (str "expanded-step-toggle-" t.tid "-" i)
        :step s
        :track t
        :selected #'s.selected
        :color :sequencer-step-border
        :selected-color :sequencer-step-selected-border
        :off-fill off-fill
        :background "seqv-slot-shell"
        :align :center :width 3 :height expanded-step-toggle-height
        :on-mouse-down (lambda (evt) (expanded-step-pointer-down t s evt))
        :on-drag (lambda (evt) (expanded-step-drag t s evt))
        :on-mouse-up (lambda (evt) (expanded-step-pointer-up t s evt))
        :on-double-click (lambda (evt) (expanded-step-double-click t s evt)))
      (box
        :key (str "expanded-step-toggle-" t.tid "-" i)
        :track t
        :selected false
        :color :sequencer-step-border
        :selected-color :sequencer-step-selected-border
        :off-fill off-fill
        :background "seqv-slot-shell-off"
        :align :center :width 3 :height expanded-step-toggle-height))))

;; Slot i on page `page`: step s's slider, gate, number, length mark and
;; playhead dot, framed while it holds the editor's cursor.
(def expanded-slot (t mode lane look page i)
  (let ((s (slot-step t page i))
        (index (+ (* page slots-per-page) i)))
    (box :padding expanded-step-column-padding
      :key (str "expanded-step-column-" t.tid "-" i)
      :background "seqv-step-cursor"
      :index index
      :track t
      :cursor #'t.cursor
      :on-click (lambda (evt) (when s (expanded-step-click t s evt)))
      :on-drag (lambda (evt) (when s (expanded-step-drag t s evt)))
      (v-stack :align :center :gap expanded-step-column-gap
        (slot-slider t s mode lane look i)
        (slot-toggle t s i)
        (number-label
          :key (str "expanded-step-label-" t.tid "-" i)
          :value (+ index 1)
          :active (if s #'s.selected false)
          :active-color :yellow
          :decimals 0
          :width 2.8
          :height expanded-step-label-height
          :h-align :center
          :font-size 10 :bg :transparent
          :color :dim)
        (box :key (str "expanded-step-length-" t.tid "-" i)
          :width 2.8 :height 0.3
          :background "seqv-slot-length-mark"
          :track t
          :index index)
        (step-playhead-dot
          :key (str "expanded-step-playhead-" t.tid "-" i)
          :active (if s #'s.playing false))))))

;; The sixteen slots of the page t's editor shows in mode `mode` (`lane` its
;; lane, or nil). A subtree: a page turn (the cursor's, or the playhead's
;; while following) re-runs the slots alone.
(def expanded-step-grid (t mode lane)
  (subtree :key (str "expanded-steps-" t.tid)
    (let ((page (shown-page t))
          (look (slot-slider-look t mode lane)))
      (grid
        :cols 16
        :col-width 4
        :row-height expanded-step-row-height
        :align :stretch
        (each (range 0 slots-per-page) |i|
          (expanded-slot t mode lane look page i))))))

;; The cursor step's value in mode `mode` (`lane` its lane, or nil): a sync
;; label picker in sync mode, else a number picker (0 in a lane mode with no
;; lane). A subtree: a cursor move re-runs the picker alone.
(def expanded-cursor-picker (t mode lane)
  (subtree :key (str "expanded-cursor-param-" t.tid)
    ;; An unkeyed root: the picker's key is the one Rust looks up
    ;; (`current-number-picker-key`).
    (h-stack
      (let ((s (step-at t t.cursor)))
        (if (= mode 5)
          (dropdown
            :key (str "expanded-sync-picker-" t.tid)
            :value-index (if s #'s.sync 0)
            :options project.sync-options
            :on-change (lambda (label)
              (set-expanded-current-param t mode (max 0 (index-of project.sync-options label))))
            :width 8 :height 1.3 :font-size 11)
          (number-picker :key (str "expanded-param-number-picker-" t.tid)
            :border-color :white
            :value (if s
                     (if (tp/seqv-process-lane-mode? mode)
                       (if lane (nth lane.values s.index) 0)
                       (tp/seqv-step-ref s mode))
                     0)
            :min (tp/seqv-track-param-min t mode) :max (tp/seqv-track-param-max t mode)
            :decimals (expanded-param-decimals t mode lane)
            :step (expanded-param-step lane)
            :on-change (lambda (v) (set-expanded-current-param t mode v))
            :width 8 :height 1.3 :font-size 11))))))

;; The pattern's pages, the shown one lit, each over its bar transpose (Cirklon
;; P3 bar XPOSE; dim while 0). A subtree: a page turn or a transpose re-runs
;; the pages alone.
(def expanded-pages (t)
  (subtree :key (str "expanded-pages-run-" t.tid)
    (h-stack
      (let ((shown (shown-page t))
            (transposes t.bar-transposes))
        (box :background "transport-btn-bg" :padding 0.2 :height 2.75
          :key (str "expanded-pages-" t.tid)
          (h-stack :gap 0.1 :align :center
            (each (range 0 (track-pages t)) |page|
              (let ((transpose (or (nth transposes page) 0)))
                (v-stack :gap 0.15 :align :center
                  :key (str "expanded-page-slot-" t.tid "-" page)
                  (box :width sgi/page-button-width :height 1.1
                    :key (str "expanded-page-" t.tid "-" page)
                    :background "pattern-pill-bg"
                    :active (= page shown)
                    :style eseq.transport/pattern-control-style
                    :on-click (lambda (event) (goto-page t page))
                    (v-stack :align :center
                      (label (fmt " {} " (+ page 1))
                        :font-size 11
                        :active (= page shown)
                        :active-color :white
                        :color :dim
                        :bg :transparent)))
                  (number-picker
                    :key (str "expanded-bar-transpose-" t.tid "-" page)
                    :value transpose
                    :min -60 :max 60 :step 1 :decimals 0
                    :active (not (= transpose 0))
                    :active-color :white
                    :text-color :dim
                    :noui true
                    :text-align :center
                    :on-change (lambda (v) (set-bar-transpose! t page v))
                    :width sgi/page-button-width
                    :height 1.1 :font-size 7))))))))))

;; The row's quick controls over t's editor in mode `mode` (`lane` its lane,
;; or nil).
(def expanded-track-quick-controls (t mode lane)
  (v-stack
    (box :height 0.4 :width 1)
    (h-stack :gap 0.55 :align :center
      (box :width (param-header-width mode) :height 1.3
        :key (str "expanded-step-summary-" t.tid)
        (label (param-header-name t mode)
          :font-size 11 :width (param-header-width mode) :color :white :bg :transparent))
      (expanded-cursor-picker t mode lane)
      (h-stack :gap 0.4 :align :center
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :key (str "expanded-half-" t.tid)
          :on-click (lambda (event) (resize-pattern t seq-halve-track-pattern))
          (v-stack :align :center
            (label "-"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :key (str "expanded-double-" t.tid)
          :on-click (lambda (event) (resize-pattern t seq-double-track-pattern))
          (v-stack :align :center
            (label "+"
              :font-size 12
              :color :white
              :bg :transparent)))
        (expanded-pages t)))))

;; t's param tabs and lane selector (`lane` the lane its editor shows, or
;; nil). A subtree of its own: arming a process map (the tabs' armed tint)
;; re-runs the tabs, not the row.
(def expanded-param-tabs (t lane)
  (subtree :key (str "expanded-tabs-" t.tid)
    (h-stack :gap 0.5
      (box :width 1)
      (param-tab t 0 "vel")
      (param-tab t 1 "dur")
      (param-tab t 3 "tpose")
      (param-tab t 4 "pan")
      (param-tab t 5 "sync")
      (param-tab t 6 "delay")
      (param-tab t 7 "rtrg")
      (param-tab t 8 "rate")
      (process-lane-selector t lane))))

;; t's editor in mode `mode` (`lane` its lane, or nil). The tabs, the slots,
;; the strip, the patchbay and the OTHER LANES row are subtrees of their own,
;; so a map, cable, card or inlet edit re-runs one of them and never the
;; expanded rows; the OTHER LANES row stacks at gap 0 with the editor's body
;; and carries its own gap.
(def expanded-track-editor (t mode lane)
  (box :padding 0.85
    (box
      :background-color :buffer-bg :corner-radius (eseq.seq-core-state/radius 16)
      (v-stack :width :fill :padding 0.35 :gap 0.1
        (expanded-param-tabs t lane)
        (v-stack :width :fill :gap 0
          (h-stack :gap 0.5 :padding 0 :align :start
            (v-stack
              (expanded-step-grid t mode lane)
              (lane-patchbay-under t lane))
            (lane-strip t lane))
          (other-lanes-row t))))))

;; t's expanded row: its header and quick controls over its editor, in its
;; param mode (`t.param-mode`; a lane mode's lane read once here).
(def expanded-track-row (t bare)
  (let ((mode t.param-mode)
        (lane (tp/seqv-track-process-lane t mode)))
    (v-stack
      :width :fill :gap 0.2
      (h-stack :padding 0.1 :width :fill :gap 0.6 :align :start
        (track-header t bare)
        (expanded-track-quick-controls t mode lane)
        (box :flex 1 :width 0 :height 0.1 :bg :transparent)
        (track-actions t))
      (expanded-track-editor t mode lane))))

;; Which sound payloads track t will accept as a replacement of what it
;; already plays. Shared by the track row and the pad grid's occupied cells:
;; dropping on a pad replaces that pad's sound on its member track, so the
;; two must agree.
(def sound-drop-types (t)
  (if (eseq.track-collapse/replaceable-type? t.instrument-type)
    (list "sample" "instrument" "instrument-preset" "sound")
    (list "sample")))

;; One track's grid row. Rack members render through this exact path — a rack
;; member is an ordinary track, so its pattern length, timebase p-locks,
;; accumulator and expanded step editor all come along for free.
;; The selection and the silenced look are bindings (not value reads) so
;; selection, mute and solo changes update the row chrome without rerunning
;; the enclosing subtree. A binding cannot be negated: `:muted` is bound to
;; `t.audible`, so the plain props carry the silenced look.
(def track-row (t bare)
  (let ((i t.index))
    (box :width :fill
        :key (str "track-drop-" i)
        :selected #'t.in-selection
        :muted #'t.audible
        :background-color :mixer-strip-muted-bg
        :selected-background-color :mixer-strip-selected-bg
        :muted-background-color :buffer-bg
        :border-width 2
        :corner-radius (eseq.seq-core-state/radius 10)
        :border-color :mixer-strip-border
        :selected-border-color :mixer-strip-selected-border
        :muted-border-color :mixer-strip-border
        :drop-hover-border-color :mixer-strip-selected-border
        :drop-types (sound-drop-types t)
        :drop-meta (dict :kind "track" :track i)
        :on-drop (lambda (event) (drop-on-track event))
        :padding 0.0145
        :on-click (lambda (event) (track-click event t))
        :on-double-click (lambda (event) (open-piano-roll-for-track t))
        (if t.expanded
          (expanded-track-row t bare)
          (h-stack :padding 0.1 :width :fill :gap 0.6 :align :start
            (v-stack (box :height 0.1)
              (track-header t bare))
            (track-grid t)
            (box :flex 1 :width :fill :height 0.1 :bg :transparent)
            (track-actions t))))))

;; ── Track groups ────────────────────────────────────────────────────────
;; Regular groups and drum racks share one nested block: a header row owns the
;; backing bus's mute/solo/volume, name and collapse state, with member tracks
;; beneath as ordinary rows. A drum rack additionally owns pad-play arming and
;; rack-specific member chrome; regular groups deliberately have no Arm control.

(def group-ui-kind (g)
  (if g.rack "rack" "group"))

(def group-element-key (g element)
  (str (group-ui-kind g) "-" element "-" g.gid))

;; Keep group fills opaque because the rounded-box renderer draws its border
;; behind the inset fill. Reproduce the old alpha-0.22 track tint by compositing
;; it over the active theme's buffer surface in Lisp; selection then changes
;; only the border without changing the original fill appearance.
(def group-container-bg (c)
  (let ((bg THEME.buffer_bg))
    (rgba
      (+ (* (rgb-part c 0) 0.22) (* (nth bg 0) 0.78))
      (+ (* (rgb-part c 1) 0.22) (* (nth bg 1) 0.78))
      (+ (* (rgb-part c 2) 0.22) (* (nth bg 2) 0.78))
      1.0)))

;; Volume/meter for a group's backing bus b; a spacer while it has none.
(def group-volume-control (g b)
  (if b
    (let ((set-from (lambda (event) (set-volume! b event))))
      (volume-meter (group-element-key g "volume-control") b set-from set-from))
    (v-stack (box :height 0.13)
      (box :width 8.2 :height 1.25 :bg :transparent))))

;; Selecting a group selects its backing bus, exactly as the mixer header does,
;; so the fx panel follows the group chain.
(def select-group (g)
  (when g.bus (set! eseq.seq-core-state/selected-bus g.bus.index)))

(def show-fx-for-group (g)
  (seq-clear-delete-target)
  (seq-clear-selection)
  (select-group g)
  (eseq.seq-panels/seq-show-fx-lower-panel))

;; Selection visibility rides the *sel-sync* SEQV field, never a raw
;; `selected-bus` read: this block wraps every member row, so a render-time
;; read here re-rendered the whole group on each selection (eseq-4jv).
;; COMPAT(eseq-0l17): until the bus selection is a kind field.
(def group-selected-binding (g)
  (eseq.seq-core-state/group-selected-vis-binding g.gid))

;; Every group member gets the same indented prefix used by drum racks. It
;; visually connects the ordinary track row to the containing group header.
(def group-member-row (g t)
  (h-stack :width :fill :gap 0.15 :align :start
    (group-track-indicator
      :key (str "group-track-indicator-" g.gid "-" t.tid))
    (box :width 0 :flex 1 (track-row t false))))

(def group-type-icon (g)
  ;; The browser lists Drum Rack and Instrument Rack under the same :sampler
  ;; rack glyph, so both the drum-rack group and the slot-based rack use it.
  (when g.rack :sampler))

(def group-header-body (g)
  (let ((c g.color)
        (b g.bus)
        (on-select (lambda (event) (select-group g))))
    (box :background "seqv-track-container"
      :padding 0.1
      :on-click on-select
      ;; A fill row: the clip grid at the end flexes into whatever width the
      ;; panel has left and wraps there, so a wide window shows more cells
      ;; per row and a long bank grows the header instead of running off it.
      (h-stack :width :fill :gap 0.4 :align :center
        (box
          :key (group-element-key g "color-badge")
          :width 0.68 :height 2.0
          :background "seqv-track-color-badge"
          :track-r (rgb-part c 0)
          :track-g (rgb-part c 1)
          :track-b (rgb-part c 2)
          :on-click on-select)
        (disclosure-button
          :key (group-element-key g "collapse")
          :width 1.55 :height 1.4
          :collapsed g.collapsed
          :col 1
          :surface-alpha 1.0
          :focusable true
          :on-click (lambda (event) (toggle! g.collapsed)))
        ;; Arm = drum-rack pad-play mode. A regular group is not an input
        ;; target and therefore contributes no Arm control or placeholder.
        (if g.rack
          (box :width 2 :height 1.5
            :background "seqv-rec-arm-dot"
            :key (group-element-key g "arm")
            :active #'g.armed
            :on-click (lambda (event)
              (select-group g)
              (toggle! g.armed)))
          (box :width 2.0 :height 0.0 :bg :transparent))
        (header-toggle "M" (group-element-key g "mute") (if b #'b.muted false)
          :bg :control-on-bg :active-bg :sequencer-toggle-off-bg
          :color :black :active-color :gray
          :on-click (lambda (event)
            (when b
              (select-group g)
              (toggle! b.muted))))
        (header-toggle "S" (group-element-key g "solo") (if b #'b.soloed false)
          :active-color :white
          :on-click (lambda (event)
            (when b
              (select-group g)
              (toggle! b.soloed))))
        (box :width 8.6 :height 1
          :key (group-element-key g "select")
          :background-color :transparent
          :on-click on-select
          :on-double-click (lambda (event) (show-fx-for-group g))
          (badge (track-name-display g.name)
            :key (group-element-key g "name-label")
            :icon (group-type-icon g)
            :font-size 11 :width 8.6 :height 1 :padding 0
            :h-align :left
            :background-color :transparent
            :border-color :transparent
            :highlight-color :transparent
            :shadow-color :transparent
            :muted (if b #'b.muted false)
            :color :dim
            :muted-color (rgba 0.4 0.4 0.4 0.6)
            :bg :transparent))
        (box :width 0.3)
        ;; No PADS/KIT buttons here: selecting the rack puts both the pad grid
        ;; and SAVE KIT in the *fx* buffer's rack panel (ui/effects/buffers.lisp,
        ;; docs/drum-rack-v2-spec.md, "UI"), so the header keeps the same
        ;; name/meter shape an ordinary track header has.
        (group-volume-control g b)
        ;; A clip-bearing rack's clip grid sits in the header's empty right half
        ;; (§6.1), starting where the member rows' step grids start.
        (rack-clip-grid g)))))

;; ── Rack clip grid (docs/rack-clips-and-break-kits-spec.md §6.1) ─────────
;; A collapsed rack is no longer a dead row: it shows the rack's clip bank as a
;; Max-style preset box grid. The cell the CURRENT scene plays is lit;
;; clicking one launches it (quantized exactly like a scene launch),
;; right-clicking opens a menu (rename in place, launch, save what the rack
;; is playing now as a new clip, delete), and the trailing number picker
;; shows the lit clip's number and launches whatever number is typed in.
;; Drag reorder is not wired yet; the bank order is the create order.

;; The cells are the mixer's `track-pattern-cell-bg` boxes (ui/mixer.lisp)
;; without the sound glyph on top: unnumbered, tinted with the rack color,
;; lit when active. They sit in a `wrap` that fills the header's right half,
;; so the column count follows the panel width and a long bank wraps onto
;; more rows rather than stretching the row past the window.
(def rack-clip-cell-width 3.6)
(def rack-clip-cell-height 1.8)
(def rack-clip-rename-width 4.6)

;; Whether the held clip rc is still in its rack's bank (a delete, an undo or
;; a project load drops it).
(def clip-listed? (rc)
  (and rc rc.group (listed? rc rc.group.clips)))

(def begin-clip-rename (rc)
  (set! clip-menu.open false)
  (set! clip-rename.draft rc.name)
  (set! clip-rename.clip rc))

(def finish-clip-rename (commit)
  (let ((rc clip-rename.clip))
    (when (and commit (clip-listed? rc))
      (set! rc.name clip-rename.draft))
    (set! clip-rename.clip nil)
    (set! clip-rename.draft "")))

(def open-clip-menu (event rc)
  (set! clip-menu.clip rc)
  (open-menu! clip-menu event))

;; The menu's action on its clip, while that clip is still in the bank.
(def clip-menu-action (action)
  (lambda (event)
    (let ((rc clip-menu.clip))
      (set! clip-menu.open false)
      (when (clip-listed? rc) (action rc)))))

;; Mounted once at the buffer root (like the mixer's track menu) so it
;; overlays the grid instead of being clipped by the header row.
(def rack-clip-context-menu ()
  (menu-of clip-menu
    (menu-item "Rename…" :key "rack-clip-menu-rename"
      :on-select (clip-menu-action begin-clip-rename))
    (menu-item "Launch" :key "rack-clip-menu-launch"
      :on-select (clip-menu-action launch-rack-clip!))
    (menu-separator)
    (menu-item "New Clip from Playing" :key "rack-clip-menu-save-new"
      :on-select (clip-menu-action (lambda (rc) (save-rack-clip-as! rc.group ""))))
    (menu-item "Delete" :key "rack-clip-menu-delete"
      :on-select (clip-menu-action delete-rack-clip!))))

;; The rack color c, scaled by `k`.
(def tint (c k) (map (lambda (i) (* k (rgb-part c i))) (range 0 3)))

;; Clip rc's launch cell, tinted `tinted` (its rack's color, dimmed). A
;; subtree of its own: a rename's start, its keystrokes and its end re-run
;; this cell alone.
(def rack-clip-cell (g rc tinted)
  (subtree :key (str "rack-clip-cell-" g.gid "-" rc.cid)
    ;; An unkeyed root: the subtree's key would replace the cell's, which
    ;; tests and captures find.
    (h-stack (rack-clip-cell-body g rc tinted))))

(def rack-clip-cell-body (g rc tinted)
  (let ((renaming (= clip-rename.clip rc)))
    (box :key (str "rack-clip-" g.gid "-" rc.cid)
      :debug-name "rack-clip-cell"
      :width (if renaming rack-clip-rename-width rack-clip-cell-width)
      :height rack-clip-cell-height
      :padding (if renaming 0.1 0.3)
      :bg :transparent
      :background "track-pattern-cell-bg"
      :active #'rc.active
      :assigned 1
      :override 0
      :selected 0
      :track-r (nth tinted 0)
      :track-g (nth tinted 1)
      :track-b (nth tinted 2)
      :on-click (lambda (event)
        (if event.shift
          (begin-clip-rename rc)
          (launch-rack-clip! rc)))
      :on-right-click (lambda (event) (open-clip-menu event rc))
      (if renaming
        (text-input
          :key (str "rack-clip-rename-" g.gid "-" rc.cid)
          :width 4.3 :height 0.7 :font-size 10
          :value clip-rename.draft
          :auto-focus true
          :select-all-on-focus true
          :on-change (lambda (name) (set! clip-rename.draft name))
          :on-submit (lambda () (finish-clip-rename true))
          :on-cancel (lambda () (finish-clip-rename false))
          :on-blur (lambda () (finish-clip-rename true)))
        (label rc.cid
          :color :dimmer :active #'rc.active :active-color :white
          :font-size 10 :bg :transparent :v-align :center :h-align :center)))))

;; The last cell is a number picker showing the lit clip's number (0 while
;; the rack is silent): read it at a glance, or type/drag a number to launch
;; that clip (quantized like a click on its cell). Save and delete live in
;; the right-click menu. A subtree of its own, which alone reads the bank and
;; the playing clip: a launch re-renders it alone.
(def rack-clip-number-picker (g tinted)
  (subtree :key (str "rack-clip-number-run-" g.gid)
    (rack-clip-number-run g tinted)))

(def rack-clip-number-run (g tinted)
  (let ((clips g.clips)
        (playing g.rack-clip))
    (h-stack
      (number-picker :key (str "rack-clip-number-" g.gid)
        :width 5.2 :height rack-clip-cell-height :font-size 10
        ;; Same skin as the launch cells: rack-tinted rim, and the well is the
        ;; cell shader's 70% dark layer composited over that tint.
        :border-color (rgba (nth tinted 0) (nth tinted 1) (nth tinted 2) 1.0)
        :border-width 2
        :background-color (rgba
          (+ (* 0.3 (nth tinted 0)) 0.014)
          (+ (* 0.3 (nth tinted 1)) 0.0175)
          (+ (* 0.3 (nth tinted 2)) 0.021)
          1.0)
        :corner-radius 4
        :value (if playing (+ playing.index 1) 0)
        :min 1 :max (max 1 (len clips)) :step 1 :decimals 0
        :on-change (lambda (v)
          (let ((rc (nth clips (- (round v) 1))))
            (when (and rc (not (= rc g.rack-clip)))
              (launch-rack-clip! rc))))))))

;; One dot per member, lit by its pad's trigger, like the pad map.
(def rack-activity-strip (g)
  (h-stack :key (str "rack-activity-" g.gid) :gap 0.08 :align :center
    (each g.tracks |m|
      (box :key (str "rack-activity-dot-" g.gid "-" m.tid)
        :width 0.42 :height 0.42
        :corner-radius (eseq.seq-core-state/radius 3)
        :background-color '(rgba 0.19 0.20 0.21 1.0)
        :selected (if m.pad #'m.pad.triggered false)
        :selected-background-color '(rgba 0.95 0.98 1.0 1.0)))))

(def rack-clip-grid (g)
  (let ((clips g.clips)
        (tinted (tint g.color 0.65)))
    (when (and g.rack (> (len clips) 0))
      (h-stack :key (str "rack-clip-run-" g.gid) :gap 0.4 :align :center :width :fill :flex 1
        ;; Lines the first cell up with the member rows' step grids.
        (box :width 2.2 :height 0.0 :bg :transparent)
        (box :background-color '(rgba 0.1 0.1 0.1 0.2) :corner-radius 10 :padding 0.2
          (wrap :key (str "rack-clip-grid-" g.gid)
            :width 48 :gap 0.12 :row-gap 0.12 :align :center
            (each clips |rc|
              (rack-clip-cell g rc tinted))
            (rack-clip-number-picker g tinted)))
        (box :height 0.1 :width :fill :flex 1)
        (rack-activity-strip g)
        (box :width 1.0 :height 0.0 :bg :transparent)))))

(def group-header-row (g)
  (subtree :key (str "seqv-" (group-ui-kind g) "-header-" g.gid)
    (group-header-body g)))

(def group-block (g)
  (let ((c g.color))
    (box :width :fill
      :key (group-element-key g "block")
      :selected (group-selected-binding g)
      :background-color (group-container-bg c)
      :selected-background-color (group-container-bg c)
      :border-width 2
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :corner-radius (eseq.seq-core-state/radius 10)
      :padding 0.345
      ;; Hit testing chooses the deepest clickable widget, so member-track
      ;; clicks keep selecting the track; only exposed container chrome reaches
      ;; this handler and selects the group's backing bus for the FX panel.
      :on-click (lambda (event) (select-group g))
      (v-stack :width :fill :gap 0.1
        (group-header-row g)
        (if g.collapsed
          (nothing)
          (v-stack :width :fill :gap 0.0
            (each (shown-members g) |m|
              (subtree :key (str "sequencer-track-" m.tid)
                (group-member-row g m)))
            (each g.racks |child|
              (subtree :key (str "sequencer-rack-" child.gid)
                (group-block child)))))))))

;; ── Grid render order ───────────────────────────────────────────────────
;; Loose tracks stay in track order. Every top-level group collapses its member
;; run into one item anchored at its lowest member, so regular groups and drum
;; racks use the same nested block model. Unanchored groups (an empty, lazy
;; drum rack) follow the tracks. A group drawn inside another's block (a rack
;; in a plain group) is its parent's to draw.

;; The lowest position among tracks, or -1 for none.
(def lowest-index (ts)
  (reduce |acc t| (if (or (< acc 0) (< t.index acc)) t.index acc) -1 ts))

;; Where a group sits in track order: its lowest member, or that of a rack
;; drawn inside it; -1 before it claims a track.
(def group-anchor (g)
  (reduce |acc child|
    (let ((a (lowest-index child.tracks)))
      (if (< acc 0) a (if (< a 0) acc (min a acc))))
    (lowest-index g.tracks)
    g.racks))

;; The grid's rows in order: `(dict :kind "track" :track t)` for a loose
;; track the grid shows (not collapsed), `(dict :kind "group" :group g)` for
;; a top-level group.
(def grid-items ()
  (let ((top (filter (lambda (g) (not g.parent)) (groups)))
        (anchored (map (lambda (g) (list g (group-anchor g))) top)))
    (append
      (reduce |acc t|
        (let ((hit (first (filter (lambda (ga) (= (nth ga 1) t.index)) anchored))))
          (if hit
            (append acc (list (dict :kind "group" :group (nth hit 0))))
            (if (or t.group t.collapsed)
              acc
              (append acc (list (dict :kind "track" :track t))))))
        (list)
        (tracks))
      (map (lambda (ga) (dict :kind "group" :group (nth ga 0)))
        (filter (lambda (ga) (< (nth ga 1) 0)) anchored)))))

;; Group g's member tracks the grid shows: collapsed ones hide exactly as
;; loose ones do.
(def shown-members (g)
  (filter (lambda (m) (not m.collapsed)) g.tracks))

;; Group g's track rows in the order `group-block` draws them: its shown
;; members, then each rack drawn inside it; none while it is collapsed.
(def group-track-order (g)
  (if g.collapsed
    (list)
    (reduce |acc child| (append acc (group-track-order child))
      (shown-members g)
      g.racks)))

;; The grid's track rows in render order (the shift-click range).
(def visible-tracks ()
  (reduce |acc item|
    (append acc
      (if (= (get item :kind) "group")
        (group-track-order (get item :group))
        (list (get item :track))))
    (list)
    (grid-items)))

(def grid-render-item (item)
  (if (= (get item :kind) "group")
    (let ((g (get item :group)))
      (subtree :key (str "sequencer-" (group-ui-kind g) "-" g.gid)
        (group-block g)))
    (let ((t (get item :track)))
      (subtree :key (str "sequencer-track-" t.tid)
        (track-row t true)))))

;; ── Pad grid performance view ───────────────────────────────────────────
;; A 4x4 VIEW over a drum rack's pads (`g.pads`) — finger drumming and slot
;; browsing only. It owns no sequencing state: a cell draws the pad on its
;; note and a hit goes straight down the live pad path. The grid renders in
;; the *fx* buffer's rack panel (ui/effects/buffers.lisp); the sequencer
;; header carries no pad toggle of its own.
;;
;; Cells are NOTE-POSITIONAL: a cell is a fixed MIDI note and draws whichever
;; pad answers to it, so label and position can never contradict each other.
;; The note geometry — octave-aligned pages, lowest note bottom-left — lives
;; in eseq.drum-rack-v2; what lives here is which page is showing.

;; Which page of notes rack g's grid shows. One rack panel is on screen at a
;; time, so the page `pad-view` holds is the last paged rack's: any other
;; rack opens on its own default page instead of inheriting a stale one.
(def pad-page (g)
  (if (= pad-view.group g)
    (eseq.drum-rack-v2/clamp-pad-page pad-view.page)
    (default-pad-page g)))

(def set-pad-page (g page)
  (set! pad-view.group g)
  (set! pad-view.page (eseq.drum-rack-v2/clamp-pad-page page)))

;; Where a rack's grid opens: the page holding its lowest pad, so a kit that
;; lives at C7 does not open onto empty octaves; an empty rack's home is C1,
;; where a rack's first pad lands.
(def default-pad-page (g)
  (let ((pads g.pads))
    (eseq.drum-rack-v2/page-of-note
      (if (empty? pads)
        (eseq.drum-rack-v2/min-grid-pad-note)
        (reduce |acc p| (min acc p.note) (eseq.drum-rack-v2/max-grid-pad-note) pads)))))

;; The note a grid position names on the visible page — the cell's identity,
;; occupied or not.
(def pad-cell-note (g cell)
  (eseq.drum-rack-v2/cell-note (pad-page g) cell))

;; The pad of rack g answering to `note`, or nil.
(def pad-on-note (g note)
  (first (filter (lambda (p) (= p.note note)) g.pads)))

;; Pad drawn at a grid position: the one whose note IS this cell's note, or
;; nil. Pads on other pages simply do not render here; the grid is sparse by
;; design and never compacts.
(def pad-at (g cell)
  (pad-on-note g (pad-cell-note g cell)))

;; The pad the rack's *fx* panel is focused on, while it is rack g's. Per-pad
;; fx are the member track's own chain by design (docs/drum-rack-v2-spec.md,
;; "UI"), so all a pad focus has to do is name the member the panel offers
;; to open.
(def focused-pad (g)
  (let ((p pad-view.focus))
    (when (and p (listed? p g.pads)) p)))

(def focus-pad! (p) (set! pad-view.focus p))

(def pad-selected? (p)
  (and p (= pad-view.focus p)))

;; Lazy pads (docs/drum-rack-v2-spec.md, "Track budget"): dropping a sound on an
;; EMPTY cell is what makes that pad claim a track. The new member is mapped to
;; exactly this cell's pad note, so it is playable by pad click and armed key
;; the moment it lands.
(def drop-on-empty-pad (event g cell)
  (let ((payload (get event :payload))
        (note (pad-cell-note g cell))
        (path (get payload :path))
        (name (get payload :name)))
    (match (get event :drag-type)
      ;; A pad needs a member track in this rack's group on a specific pad
      ;; note; the builtin add-track host commands take neither, so builtins
      ;; are refused instead of landing as a loose track (eseq-mj8).
      "instrument"
      (if (= (get payload :kind) "builtin-instrument")
        (status "Drop a sample or saved instrument onto a pad")
        (if name
          (do
            (eseq.browser/show-loading! name)
            (host-command "add-track-instrument"
              (dict :name name :group-id g.gid :pad-note note)))
          (status "Drop an instrument, not a folder")))
      "instrument-preset"
      (let ((instrument (eseq.browser/preset-payload-instrument payload)))
        (if instrument
          (do
            (eseq.browser/show-loading! instrument)
            (host-command "add-track-instrument"
              (dict :name instrument :preset (get payload :preset)
                :group-id g.gid :pad-note note)))
          (status "Drop an instrument preset")))
      _
      (if path
        (host-command "add-track-sample"
          (dict :path path :group-id g.gid :pad-note note
            :preserve-browser-context true))
        (status "Drop a sample file, not a folder")))))

;; An OCCUPIED cell replaces the pad's sound on its existing member track — the
;; same replacement a drop on the member's grid row does — so pad identity,
;; pattern data, mixer settings and chokes all stay put.

;; Every cell also takes a dragged pad ("rack-pad"): dropping one on an empty
;; cell moves it to that note, dropping it on an occupied cell swaps the two.
(def pad-cell-drop-types (p)
  (cons "rack-pad"
    (if p
      (sound-drop-types p.track)
      (list "sample" "instrument" "instrument-preset"))))

;; A pad dragged from the grid carries its rack and note; the drop target
;; only needs to know which note it lands on. The drag stays within one rack:
;; a pad from another group is ignored rather than adopted.
(def pad-drag-payload (g p)
  (dict :group-id g.gid :pad-note p.note))

;; Move the dragged pad to `note` (an occupied note swaps the two pads).
(def drop-pad-on-note (event g note)
  (let ((payload (get event :payload)))
    (if (= (get payload :group-id) g.gid)
      (let ((p (pad-on-note g (get payload :pad-note))))
        (when (and p (not (= p.note note)))
          (set! p.note note)))
      (status "Drag pads within one rack"))))

(def pad-cell-drop-meta (g cell p)
  (pad-drop-meta g cell (pad-cell-note g cell) p))

;; Cell `cell`'s drop meta, on `note` with pad p (nil when empty).
(def pad-drop-meta (g cell note p)
  (if p
    (dict :kind "track" :track p.track.index :from-pad true)
    (dict :kind "rack-pad" :group-id g.gid :cell cell :pad-note note)))

(def drop-on-pad-cell (event g cell)
  (let ((p (pad-at g cell)))
    (if (= (get event :drag-type) "rack-pad")
      (drop-pad-on-note event g (pad-cell-note g cell))
      (if p
        (drop-on-track event)
        (drop-on-empty-pad event g cell)))))

;; A pad's own fx ARE its member track's chain (docs/drum-rack-v2-spec.md,
;; "UI"), so "open this pad" means: make its member the track under edit. That
;; drops the bus selection, which is exactly what swaps the *fx* buffer from
;; the rack panel to that member's instrument and effects — in place, without
;; touching the workspace layout.
(def open-pad! (p)
  (when p (select-track-for-edit p.track)))

;; Pad context menu (docs/rack-groove-spec.md, "Pad roles"): Role ▸ with
;; "Standard (<inferred>)" first — the role the standard layout gives the
;; pad's note — then every explicit role, the pad's current choice checked.
;; Mounted once at the *fx* rack panel root (like the clip menu in the grid)
;; so it overlays the pad grid instead of being clipped by its cell.
(def open-pad-menu (event p)
  (focus-pad! p)
  (set! pad-menu.pad p)
  (open-menu! pad-menu event))

;; Set the menu's pad's role to `key` ("" for the standard layout's), while
;; the pad is still its rack's.
(def choose-pad-role (key)
  (let ((p pad-menu.pad))
    (set! pad-menu.open false)
    (when (and p (listed? p p.group.pads))
      (set! p.role key))))

(def rack-pad-context-menu ()
  (let ((p pad-menu.pad))
    (menu-of pad-menu
      (menu-item "Role" :key "rack-pad-menu-role"
        (menu-item (str "Standard ("
                        (if (or (not p) (= p.standard-role-label "")) "none" p.standard-role-label)
                        ")")
          :key "rack-pad-menu-role-standard"
          :checked (or (not p) (= p.role ""))
          :on-select (lambda (event) (choose-pad-role "")))
        (menu-separator)
        (each (if pad-menu.open (eseq.drum-rack-v2/pad-role-options) (list)) |option|
          (menu-item (get option :label)
            :key (str "rack-pad-menu-role-" (get option :key))
            :checked (and p (= p.role (get option :key)))
            :on-select (lambda (event) (choose-pad-role (get option :key)))))))))

;; The role tag sits in the pad's top corner: bright when set on the pad,
;; dim when it is only the standard layout's guess, absent when neither
;; names a drum.
(def pad-role-tag-label (p)
  (label (if p p.role-tag "")
    :width 1.4 :font-size 6.2 :bg :transparent :text-align :right
    :color (if (and p (not (= p.role ""))) :white :dim)))

;; Pad trigger light (eseq-4b5.16): a pad is lit for as long as its member
;; track is sounding whatever fired it — a hit on this cell, an armed rack's
;; keys, or its own sequenced steps. It rides the box's bound `selected` state
;; (`#'p.triggered`) rather than a lisp-computed colour, so a hit repaints the
;; cell without re-rendering the grid, the way a mixer meter does; the
;; persistent pad focus keeps its own border. Cell `cell` of the page `page`
;; shows, `pads` the rack's pads by map slot (`pads-by-note`).
(def pad-cell (g pads page cell)
  (let ((note (eseq.drum-rack-v2/cell-note page cell))
        (p (nth pads (pad-map-slot note))))
    (box
      :key (str "rack-pad-cell-" g.gid "-" cell)
      :width 6.4 :height 2.3 :padding 0.15
      :background-color (if p :mixer-strip-bg :bg)
      :selected (if p #'p.triggered false)
      :selected-background-color :accent
      :border-width 1
      :border-color (if (pad-selected? p)
        :mixer-strip-selected-border
        '(rgba 0.30 0.31 0.32 1.0))
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-hover-background-color :mixer-control-bg
      :corner-radius (eseq.seq-core-state/radius 8)
      :drop-types (pad-cell-drop-types p)
      :drop-meta (pad-drop-meta g cell note p)
      :on-drop (lambda (event) (drop-on-pad-cell event g cell))
      ;; An occupied cell is a drag source: drag it onto another cell of this
      ;; page, or onto a note in the octave map, to move the pad there.
      :drag-type (when p "rack-pad")
      :drag-modifier :none
      :drag-payload (when p (pad-drag-payload g p))
      ;; A click focuses the pad; it no longer auditions it (the pad keys and
      ;; the sequencer play it), so a click that turns into a drag is silent.
      :on-click (lambda (event) (when p (focus-pad! p)))
      ;; A double-click opens the pad: its member track becomes the track under
      ;; edit, which swaps the *fx* panel to that member's own chain.
      :on-double-click (lambda (event) (open-pad! p))
      ;; Right-click: the pad menu (Role ▸ …).
      :on-right-click (lambda (event) (when p (open-pad-menu event p)))
      (v-stack :width :fill :height :fill :gap 0.05
        ;; The note is the cell's own, so an empty cell still says which note
        ;; a drop here would claim. The role tag balances a same-width spacer
        ;; on the left so the note stays centred.
        (h-stack :width :fill :gap 0 :align :center
          (box :width 1.4 :height 0.1 :bg :transparent)
          (label (if p p.label (eseq.drum-rack-v2/note-label note))
            :font-size 8 :color (if p :dim '(rgba 0.42 0.44 0.46 1.0))
            :active (if p #'p.triggered false)
            :active-color :black
            :bg :transparent
            :flex 1 :width :fill :text-align :center)
          (pad-role-tag-label p))
        (label (if p (substring p.track.name 0 12) "")
          :active (if p #'p.triggered false)
          :active-color :black
          :font-size 6.8 :color :white :bg :transparent
          :width :fill :text-align :center)
        (label (if (and p (> p.choke 0)) (str "choke " p.choke) "")
          :font-size 6.2 :color :dim :bg :transparent
          :width :fill :text-align :center)))))

;; Row 0 renders at the TOP and carries the page's HIGHEST four notes: the
;; grid reads bottom-up, so the bottom-left cell is the page's C.
(def pad-grid-row (g pads page row)
  (h-stack :gap 0.15 :align :center
    (each (range 0 4) |col|
      (pad-cell g pads page (+ (* row 4) col)))))

(def pad-grid-intrinsic-width 26.45)

;; Rack g's pad grid as a component: the *fx* rack panel draws it at its
;; intrinsic width beside the rack's own controls. It lives here, next to
;; the pad cell it is made of, so pad badges, drops and audition stay one
;; definition wherever a future surface draws them. The page and the pads by
;; note are read once here, like the octave map's.
(def pad-grid (g)
  (let ((page (pad-page g))
        (pads (pads-by-note g)))
    (box :key (str "rack-pad-grid-" g.gid)
      :width pad-grid-intrinsic-width :padding 0.2
      :background-color :bg
      :corner-radius (eseq.seq-core-state/radius 14)
      (v-stack :gap 0.1 :align :start
        (each (range 0 4) |row|
          (pad-grid-row g pads page row))))))

;; ── Octave overview mini-map (eseq-4b5.15) ──────────────────────────────
;; A slim full-range map to the LEFT of the pad grid: every note the grid can
;; address as a tiny cell, four to a row, lowest at the bottom, with the
;; sixteen notes currently enlarged drawn as a highlighted block. Occupied
;; notes are filled, so a kit that lives three octaves up is visible without
;; paging there — and a click on any row pages the grid to it, which is the
;; affordance the arrows can only offer one octave at a time.
;;
;; It is a second VIEW of the grid's page, not a second page state: the
;; highlight and the click both go through the same page functions the grid
;; uses. Each occupied cell lights on its pad's trigger, the same binding the
;; enlarged grid uses, which is what makes a hit on a pad the grid is NOT
;; showing still visible here.
;;
;; The pads are read ONCE at the root and laid out by note there: a per-cell
;; scan of the pads would walk them 88 times over to draw the map once.

(def pad-map-cell-width 0.75)
(def pad-map-cell-height 0.34)

;; Where a note sits in the map's note-indexed pad list. Pad notes are
;; transposes around C4 and so go negative, which no list index does: the
;; list is indexed from the lowest note the grid can name.
(def pad-map-slot (note)
  (- note (eseq.drum-rack-v2/min-grid-pad-note)))

;; Rack g's pads by map slot, nil where no pad answers.
(def pads-by-note (g)
  (let ((top (eseq.drum-rack-v2/max-grid-pad-note)))
    (reduce |acc p|
      (if (and (>= p.note (eseq.drum-rack-v2/min-grid-pad-note)) (<= p.note top))
        (set-nth acc (pad-map-slot p.note) p)
        acc)
      (map (lambda (n) nil) (range 0 (+ (pad-map-slot top) 1)))
      g.pads)))

;; Each map cell is also a drop target for a dragged pad: that is how a pad
;; reaches another octave in one gesture, without paging first.
(def pad-map-cell (g pads note)
  (let ((p (nth pads (pad-map-slot note))))
    (box :key (str "rack-pad-map-cell-" g.gid "-" note)
      :width pad-map-cell-width :height pad-map-cell-height
      :background-color (if p '(rgba 0.60 0.72 0.75 1.0) :buffer-bg)
      :selected (if p #'p.triggered false)
      :selected-background-color '(rgba 0.95 0.98 1.0 1.0)
      :drop-types (list "rack-pad")
      :drop-hover-background-color :mixer-strip-selected-border
      :on-drop (lambda (event) (drop-pad-on-note event g note))
      ;; Being a drop target makes the cell a pointer target too, so it must
      ;; page the grid itself: the click no longer reaches the row.
      :on-click (lambda (event) (set-pad-page g (eseq.drum-rack-v2/page-of-note note)))
      :corner-radius (eseq.seq-core-state/radius 2))))

;; A row is the click target, not its cells: four notes is already a finer jump
;; than the octave-aligned pages the click snaps to, and one handler per row
;; keeps the map cheap.
(def pad-map-row (g pads row page)
  (let ((base (eseq.drum-rack-v2/pad-map-row-base row))
        (on-page (eseq.drum-rack-v2/pad-map-row-on-page? row page)))
    (box :key (str "rack-pad-map-row-" g.gid "-" base)
      :background-color (if on-page '(rgba 0.24 0.32 0.34 1.0) :transparent)
      :border-width 1
      :border-color (if on-page :mixer-strip-selected-border :transparent)
      :corner-radius (eseq.seq-core-state/radius 2)
      :on-click (lambda (event) (set-pad-page g (eseq.drum-rack-v2/page-of-note base)))
      (h-stack :gap 0.08 :align :center
        (each (range 0 4) |col|
          (pad-map-cell g pads (+ base col)))))))

(def pad-map-intrinsic-width 3.6)

(def pad-map (g)
  (let ((page (pad-page g))
        (pads (pads-by-note g)))
    (box :debug-name "rack-pad-map"
      :key (str "rack-pad-map-" g.gid)
      :width pad-map-intrinsic-width :height :fill :padding 0.15
      :background-color :bg
      :corner-radius (eseq.seq-core-state/radius 8)
      :v-align :center :h-align :center
      (v-stack :gap 0.05 :align :center
        (each (range 0 (eseq.drum-rack-v2/pad-map-row-count)) |row|
          (pad-map-row g pads row page))))))

;; COMPAT(eseq-0l17.14): ui/effects/buffers.lisp addresses the rack panel by
;; group position (`gidx`) and reads the focused pad as a dict; these take
;; that address and hand its group to the pad grid.
(def rack-pad-grid (gidx) (pad-grid (nth (groups) gidx)))
(def rack-pad-map (gidx) (pad-map (nth (groups) gidx)))

(def selected-pad (gidx)
  (let ((p (focused-pad (nth (groups) gidx))))
    (when p (dict :label p.label :track p.track.index))))

(def open-pad-member-fx (gidx pad)
  (let ((t (track-at (get pad :track))))
    (when t (select-track-for-edit t))))


(effect-buffer "*sequencer*"
  (v-stack :key "sequencer-grid" :width :fill :fill-content-style true :padding 0.00 :gap 0.0
    ;; Sample import modal: opened by Rust after a file drop; renders as a
    ;; centered overlay (modal spec) with zero footprint here while closed.
    (subtree :key "seq-export-song"
      (eseq.export-song/panel))
    (subtree :key "seq-file-dialogs"
      (eseq.file-dialogs/panel))
    (subtree :key "seq-sample-import"
      (eseq.sample-import/panel))
    (subtree :key "seq-retrospective"
      (eseq.retrospective/panel))
    (subtree :key "seq-resample"
      (eseq.resample/panel))
    (subtree :key "seq-factory-promote"
      (eseq.factory-promote/panel))
    (subtree :key "seq-rack-clip-menu"
      (rack-clip-context-menu))
    (v-stack :key "sequencer-tracks" :width :fill :gap 0
      (each (grid-items) |item|
        (grid-render-item item)))

     (box :key "new-track-drop-zone"
      :width :fill :height 2.4 :flex 1
      :background-color :transparent
      :drop-hover-background-color :mixer-control-bg
      :border-width 1
      :border-color :transparent
      :drop-hover-border-color :mixer-strip-selected-border
      :corner-radius (eseq.seq-core-state/radius 10)
      :drop-types (list "sample" "instrument" "instrument-preset" "sound")
      :drop-meta (dict :kind "new-sample-track")
      :on-drop (lambda (event) (drop-new-track event))
      :on-double-click (lambda (event) (host-command "add-track-empty" (dict)))
      (label ""
        :font-size 1
        :color :transparent
        :bg :transparent))))


(set-buffer-mode-for "*sequencer*" "eseq.seq-grid-mode/seq-grid-mode")

;; The grid starts on the shared step cursor.
(set! grid-cursor.step (eseq.seq-core-state/cursor-step-value))
