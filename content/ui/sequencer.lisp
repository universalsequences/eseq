;; ui/sequencer.lisp — Project step sequencer view.
;; Renders to *sequencer* buffer. Shows every track's step grid laid out
;; vertically. Loaded by ui/main.lisp.

(module eseq.sequencer)
;; Compile-time edge (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile.
(import eseq.seq-core-state)

;; Migration aliases (module spec §10 step 2) for the names unconverted callers
;; still spell flat.  Nine are lisp-side — arrangement.lisp reuses the track
;; header and its selection binding, mixer.lisp opens the piano roll,
;; seq-panels.lisp toggles the expanded lane, seq-grid-mode.lisp routes the
;; *sequencer* keymap (both the `(seqv-handle-key …)` call and the "C-h"
;; `mode-bind-key` handler string, which dispatches through `invoke_global` and
;; therefore through the alias rung).  The rest are entry points src/ui/input.rs and
;; the Rust state_values / ui tests drive by name.  Deleted as each consumer
;; converts.

(import eseq.track-collapse)

;; Drum rack v2: rack lookups over SEQ.groups (docs/drum-rack-v2-spec.md).
(import eseq.drum-rack-v2)

(import eseq.seq-panels)

(export track-selected-binding
        expanded-track-ids
        select-track-for-edit
        open-piano-roll-for-track
        set-track-expanded
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
        track-menu-click
        drop-sample-on-track
        drop-on-track
        drop-new-track
        step-slider-track-material
        track-header
        grid-step-pointer-down
        grid-step-pointer-up
        track-current-step
        track-current-page
        select-process-lane-option
        pad-selected?
        selected-pad
        open-pad-member-fx
        rack-pad-grid
        rack-pad-map)

(def track-peak (i)
  (bind-seq (str "track-peak-" i)))

(def track-volume-field (track)
  (str "track-" track "-volume"))

(def track-volume-binding (track)
  (bind-seq (track-volume-field track)))

(def track-volume-value (track)
  (if (< track (len SEQ.track-volumes))
    (nth SEQ.track-volumes track)
    1.0))

(def track-volume-from-event (track event)
  (let ((sx (get event :sx)))
    (if (= sx nil)
      (track-volume-value track)
      (max 0.0 (min 1.0 (* 0.5 (+ sx 1.0)))))))

(def set-track-volume-from-event (track event)
  (do
    (activate-track-for-edit track)
    (seq-set-track-volume track (track-volume-from-event track event))))

(def track-color-r-binding (track)
  (bind-seq-nth "track-color-r-effective" track))

(def track-color-g-binding (track)
  (bind-seq-nth "track-color-g-effective" track))

(def track-color-b-binding (track)
  (bind-seq-nth "track-color-b-effective" track))

(def track-name-max-chars 9)

(def track-name-display (name)
  (if (> (len name) track-name-max-chars)
    (str (substring name 0 (- track-name-max-chars 2)) "..")
    name))

(def muted? (i)
  (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i)))

;; Bound, never a raw `selected-bus` read: reading the defstate here made
;; every track row (and every arrangement lane reusing this binding) re-render
;; on each bus/group selection. The *sel-sync* projection in
;; ui/seq-core-state.lisp owns that read and gates the Rust-owned per-track
;; selection into one bindable field per row (eseq-4jv).
(def track-selected-binding (i)
  (eseq.seq-core-state/track-selected-vis-binding i))

(def track-color (i)
  (if (< i (len SEQ.track-colors))
    (nth SEQ.track-colors i)
    (list 0.34 0.48 0.98)))

(def track-color-r (i muted)
  (let ((r (nth (track-color i) 0)))
    (if muted (+ (* r 0.34) (* 0.10 0.66)) r)))

(def track-color-g (i muted)
  (let ((g (nth (track-color i) 1)))
    (if muted (+ (* g 0.34) (* 0.10 0.66)) g)))

(def track-color-b (i muted)
  (let ((b (nth (track-color i) 2)))
    (if muted (+ (* b 0.34) (* 0.11 0.66)) b)))

(def row-bg (selected muted)
  (if selected
    :mixer-strip-selected-bg
    (if muted
      :mixer-strip-muted-bg
      :buffer-bg)))

(def row-border (selected)
  (if selected
    :mixer-strip-selected-border
    :mixer-strip-border))

(def timebase-options
  '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))

(def track-timebase (i)
  (if (< i (len SEQ.track-timebases))
    (nth SEQ.track-timebases i)
    "16"))

(def set-row-timebase (track label)
  (let ((plock-selected
      (and (< eseq.seq-core-state/selected-bus 0) (= SEQ.current-track track) (seq-has-selection?))))
    (do
      (activate-track-for-edit track)
      (eseq.seq-core-state/cool-off-follow)
      (if plock-selected
        (seq-plock-timebase label)
        (seq-set-timebase label)))))

(defstate expanded-track-ids '())

(defstate track-editor-state '())

(def list-contains? (xs item)
  (> (len (filter (lambda (x) (= x item)) xs)) 0))

(def list-remove (xs item)
  (filter (lambda (x) (not (= x item))) xs))

(def expanded-track-field (track-id)
  (str "track-expanded-" track-id))

(def track-id-at (track)
  (if (< track (len SEQ.track-ids))
    (nth SEQ.track-ids track)
    track))

(def track-index-for-id (track-id)
  (let ((matches
      (filter
        (lambda (i) (= (nth SEQ.track-ids i) track-id))
        (range 0 (len SEQ.track-ids)))))
    (if (> (len matches) 0)
      (nth matches 0)
      -1)))

(def project-cursor-step (track step)
  (mod step (max 1 (track-num-steps track))))

;; `cursor-step` is a mutable vanilla global; a module must read it through its
;; owner's accessor or the late-binding heal freezes the value (§10 hazard m).
(def global-cursor-step-for-track (track)
  (project-cursor-step track (eseq.seq-core-state/cursor-step-value)))

(def sync-track-cursor-to-global (track)
  (if (and (>= track 0) (< track (len SEQ.track-ids)))
    (set-track-cursor
      (track-id-at track)
      (eseq.seq-core-state/cursor-step-value))
    nil))

(def sync-all-track-cursors-to-global ()
  (for-each
    (lambda (track) (sync-track-cursor-to-global track))
    (range 0 (len SEQ.track-ids))))

;; Selecting an edit target must not change workspace layout. Opening FX is an
;; explicit gesture owned by show-fx-for-track (track-name double-click).
(def select-track-for-edit (track)
  (do
    (set! eseq.seq-core-state/selected-bus -1)
    (if (= SEQ.current-track track) nil (seq-clear-selection))
    (seq-set-track track)
    (sync-track-cursor-to-global track)))

(def activate-track-for-edit (track)
  (select-track-for-edit track))

(def open-piano-roll-for-track (track)
  (if (and (= eseq.seq-step-tabs/lower-panel-buffer "*piano-roll*") (= SEQ.current-track track))
    (eseq.seq-panels/seq-show-fx-lower-panel)
    (do
      (activate-track-for-edit track)
      (eseq.seq-panels/seq-open-piano-roll-bottom-for-track track))))

(def show-fx-for-track (track)
  (do
    (select-track-for-edit track)
    (eseq.seq-panels/seq-show-fx-lower-panel)))

(def track-expanded? (track-id)
  (reactive-get "SEQV" (expanded-track-field track-id)))

(def set-track-expanded (track-id expanded)
  (do
    (reactive-set "SEQV" (expanded-track-field track-id) expanded)
    (if expanded
      (let ((track (track-index-for-id track-id)))
        (if (>= track 0)
          (sync-expanded-step-slots-for track track-id)
          nil))
      (seqv-clear-expanded-step-slots track-id))
    (set! expanded-track-ids
      (if expanded
        (if (list-contains? expanded-track-ids track-id)
          expanded-track-ids
          (append expanded-track-ids (list track-id)))
        (list-remove expanded-track-ids track-id)))))

(def editor-state-for (track-id)
  (let ((matches (filter
      (lambda (state) (= (get state :id) track-id))
      track-editor-state)))
    (if (> (len matches) 0)
      (nth matches 0)
      (dict :id track-id :param-mode 0 :cursor-step nil))))

(def upsert-editor-state (track-id next-state)
  (if (list-contains? (map (lambda (state) (get state :id)) track-editor-state) track-id)
    (set! track-editor-state
      (map
        (lambda (state)
          (if (= (get state :id) track-id) next-state state))
        track-editor-state))
    (set! track-editor-state (append track-editor-state (list next-state)))))

(def track-param-mode (track-id)
  (get (editor-state-for track-id) :param-mode))

(def set-track-param-mode (track-id mode)
  (do
    (upsert-editor-state track-id
      (merge (editor-state-for track-id) :param-mode mode))
    (let ((track (track-index-for-id track-id)))
      (if (>= track 0)
        (sync-expanded-step-slots-for track track-id)
        nil))))

(def track-cursor (track-id)
  (let ((track (track-index-for-id track-id)))
    (if (>= track 0)
      (let ((stored-step (reactive-get "SEQV" (str "cursor-step-" track-id))))
        (if (= stored-step nil)
          (global-cursor-step-for-track track)
          (project-cursor-step track stored-step)))
      0)))

(def cursor-highlight-field (track step)
  (str "seqv-track-cursor-" track "-" step))

(def cursor-highlight-binding (track step)
  (bind "SEQV" (cursor-highlight-field track step)))

;; Clearing must target the exact field set last time. Recomputing it from the
;; stored step goes stale when num-steps or the id->index mapping changed in
;; between, leaving ghost cursor highlights behind.
(def set-track-cursor (track-id step)
  (let ((track (track-index-for-id track-id)))
    (if (>= track 0)
      (let ((previous-field (reactive-get "SEQV" (str "cursor-field-" track-id)))
          (next-field (cursor-highlight-field track (project-cursor-step track step)))
          (projected-step (project-cursor-step track step)))
        (do
          (reactive-set "SEQV" (str "cursor-step-" track-id) projected-step)
          (if (or (= previous-field nil) (= previous-field next-field))
            nil
            (reactive-set "SEQV" previous-field false))
          (reactive-set "SEQV" next-field true)
          (reactive-set "SEQV" (str "cursor-field-" track-id) next-field)
          (if (track-expanded? track-id)
            (sync-expanded-step-slots-for track track-id)
            nil)))
      nil)))

(def cursor-step-changed (track step)
  (if (and (>= track 0) (< track (len SEQ.track-ids)))
    (set-track-cursor (track-id-at track) step)
    nil))

;; Stub-then-override protocol (module spec §10 hazard i). The flat name stays
;; pinned through the §3 cross-module def escape hatch because
;; ui/step-grid-interactions.lisp compiles and calls the stub before this file
;; replaces it. This pair is the S4 `defhook` candidate.
(def eseq.vanilla/sequencer-cursor-step-changed (track step)
  (eseq.sequencer/cursor-step-changed track step))

(def current-track-id ()
  (track-id-at SEQ.current-track))

(def current-track-expanded? ()
  (track-expanded? (current-track-id)))

(def current-selected-step ()
  (track-cursor (current-track-id)))

(def current-param-mode ()
  (track-param-mode (current-track-id)))

;; Returns a widget stable key for Rust to look up verbatim
;; (`current_step_param_number_picker_key`, src/ui/input.rs:762, feeds
;; `layout_node_by_stable_key`, an exact match).  Widget `:key`s in a declared
;; module hash as `<module>/<key>`, so a lisp helper that hands a key *out* to
;; Rust has to emit the qualified spelling itself — the module name is part of
;; the value, not just of the def.
(def current-number-picker-key ()
  (str "eseq.sequencer/expanded-param-number-picker-" (current-track-id)))

(def select-current-param-mode (mode)
  (set-track-param-mode (current-track-id) mode))

(def param-mode-for-key (key)
  (if (not (= (len key) 1))
    -1
    (if (or (= key "v") (= key "V"))
      0
      (if (or (= key "d") (= key "D"))
        1
        (if (or (= key "t") (= key "T"))
          3
          (if (or (= key "p") (= key "P"))
            4
            (if (or (= key "s") (= key "S"))
              5
              (if (or (= key "x") (= key "X"))
                (if (> (len SEQ.process-lanes) 0) eseq.seqv-track-params/seqv-process-lane-mode-offset -1)
                -1))))))))

(def select-all-current-track-steps ()
  (do
    (set! eseq.seq-core-state/selected-bus -1)
    (eseq.step-grid-interactions/select-all-steps)))

(def collapse-all-tracks ()
  (do
    (for-each
      (lambda (track-id) (set-track-expanded track-id false))
      expanded-track-ids)
    (set! expanded-track-ids '())))

(def toggle-current-track-expanded ()
  (let ((track-id (current-track-id)))
    (do
      (set! eseq.seq-core-state/selected-bus -1)
      (set-track-expanded track-id (not (track-expanded? track-id))))))

(def handle-key (key text)
  (let ((mode (param-mode-for-key key)))
    (if (>= mode 0)
      (do (select-current-param-mode mode) true)
      (if (= key "LEFT")
        (do (eseq.step-grid-interactions/cursor-left) true)
        (if (= key "RIGHT")
          (do (eseq.step-grid-interactions/cursor-right) true)
          (if (= key "C-a")
            (do (select-all-current-track-steps) true)
            (if (or (= key "C-h") (= key "C-H"))
              (do (collapse-all-tracks) true)
              (if (or (= key "BS") (= key "Delete"))
                (do (eseq.step-grid-interactions/delete-selected-steps) true)
                (if (= key "RET")
                  (do (eseq.step-grid-interactions/cursor-toggle) true)
                  false)))))))))

(def track-menu-click (track)
  (let ((track-id (track-id-at track)))
    (do
      (activate-track-for-edit track)
      (set-track-expanded track-id (not (track-expanded? track-id))))))

(def drop-sample-on-track (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((path (get payload :path))
          (track (get target :track)))
      (if path
        (do
          (if (not (get target :from-pad))
            (activate-track-for-edit track))
          (eseq.browser/drop-sample-on-track event))
        (status "Drop a sample file, not a folder")))))

(def drop-on-track (event)
  (if (= (get event :drag-type) "sound")
    (eseq.browser/drop-sound-on-track event)
    (if (= (get event :drag-type) "instrument")
      (eseq.browser/drop-instrument-on-track event)
      (drop-sample-on-track event))))

(def drop-new-track (event)
  (let ((payload (get event :payload)))
    (let ((path (get payload :path))
          (name (get payload :name)))
      (if (= (get event :drag-type) "sound")
        (if path
          (host-command "add-track-from-sound" (dict :path path))
          (status "Drop a Sound item, not a folder"))
        (if (= (get event :drag-type) "instrument")
          (eseq.browser/drop-instrument-new-track payload)
          (if path
            (host-command "add-track-sample" (dict :path path :preserve-browser-context true))
            (status "Drop a sample file, not a folder")))))))

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

(defwidget seqv-track-color-badge
  :width 0.68 :height 1.5
  :paint-margin 0.08
  :state (track-r track-g track-b)
  :bindable (track-r track-g track-b)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.28)
      (rgba track-r track-g track-b 1.0)))
  )

(defwidget seqv-track-volume-meter
  :width 8.2 :height 1.05
  :paint-margin 0.16
  :state (level volume track-r track-g track-b)
  :bindable (level volume track-r track-g track-b)
  :shader
  (let ((lvl (min 1.0 (max 0.0 level)))
        (vol (min 1.0 (max 0.0 volume)))
        (track (sdf/rounded-rect width height 0.18))
        (green-end (min lvl 0.70))
        (yellow-end (min lvl 0.88))
        (red-end lvl)
        (marker-start (max 0.0 (- vol 0.012)))
        (marker-end (min 1.0 (+ vol 0.012))))
    (sdf/layer
      (sdf/fill track
        (material
          :lighting (lighting :edge-min -0.45 :edge-max 0.35
            :light (vec3 0.0 -1.4 2.2) :shininess 24.0)
          :color (rgba 0.035 0.040 0.050 0.2)))
      (if (> green-end 0.005)
        (sdf/fill
          (let ((__start 0.0)
                (__end green-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.30)
                (__radius (min 0.15 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.10 0.85 0.30 1.0)))
        (rgba 0 0 0 0))
      (if (> (- yellow-end 0.70) 0.005)
        (sdf/fill
          (let ((__start 0.70)
                (__end yellow-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.30)
                (__radius (min 0.15 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.96 0.82 0.18 1.0)))
        (rgba 0 0 0 0))
      (if (> (- red-end 0.88) 0.005)
        (sdf/fill
          (let ((__start 0.88)
                (__end red-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.30)
                (__radius (min 0.15 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.95 0.18 0.16 1.0)))
        (rgba 0 0 0 0))
      (sdf/fill
        (let ((__start marker-start)
              (__end marker-end)
              (__half_w (* 0.5 aspect (- __end __start)))
              (__half_h 0.44)
              (__radius 0.045))
          (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                (y (* 0.5 y)))
            (sdf/rounded-rect __half_w __half_h __radius)))
        (material
          :lighting (lighting :edge-min -0.30 :edge-max 0.45
            :light (vec3 0.0 -1.0 2.6) :shininess 64.0)
          :color
            (eseq.materials/color
              (rgba (+ (* track-r 0.38) 0.45) (+ (* track-g 0.38) 0.45) (+ (* track-b 0.38) 0.45) 1.0)
              (rgba 0.95 0.96 0.98 1.0)))))))

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

(defwidget seqv-playhead-row-bar
  :width 48.8 :height 0.24
  :paint-margin 0.18
  :state (col)
  :bindable (col)
  :shader
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
              :offset (vec2 0 0))))))))

(defwidget seqv-step-shell
  :width 1.5 :height 2.5
  :paint-margin 1
  :state (active plock-kind selected duration muted hide track-r track-g track-b variant-r variant-g variant-b off-fill-r off-fill-g off-fill-b)
  :bindable (active plock-kind selected duration muted hide track-r track-g track-b variant-r variant-g variant-b)
  :shader
  (if (= hide 1)
    (rgba 0 0 0 0)
    (let ((vcol (rgba variant-r variant-g variant-b 1.0))
        (seqcol (rgba 0.545 0.545 0.588 0.95))
        (radius (if (= active 1) 1 0.7))
        (border input-color)
        (offcol (rgba off-fill-r off-fill-g off-fill-b 1)))
      ;; duration visualization
      (sdf/layer
        (sdf/fill
          (sdf/translate 0.0 0.0
            (sdf/rounded-rect (* 3.0 width) (* 1.0 height) 0))
          (material
            :lighting (lighting :edge-min -0.32 :edge-max 1.293
              :light (vec3 0.8 -0.8 3.5) :shininess 92.0)
            :color (* 0.7 (if (= duration 1)
                (if (= muted 1)
                  (rgba 0 0 0 0)
                  (eseq.materials/color
                    (mix border (rgba (* track-r 0.85) (* track-g 0.85) (* track-b 0.85) 0.5) (if (= selected 1) 0.8 0.6))
                    (if (= selected 1) border (rgba track-r track-g track-b 0.6))))
                (rgba 0 0 0 0)))))
        ;; border
        (sdf/fill (sdf/circle (* radius 0.8))
          (material
            :lighting (lighting :edge-min -0.12 :edge-max 0.9
              :light (vec3 -0.3 0.7 3.8) :shininess 92.0)
            :color (* (if (= selected 1) 1 (if (= muted 1) 0.6 1.2)) (eseq.materials/color border border)))
          )
        
        (sdf/fill (sdf/circle (* radius (if (= selected 1) 0.64 0.69)))
          (material
            :lighting (lighting :edge-min -0.15 :edge-max 1.0
              :light (vec3 0.3 -2.0 0.8) :shininess 92.0)
            :color (* (if (= muted 1) 0.3 1) (eseq.materials/color offcol offcol))))
        ;; p-lock indicator
        (sdf/fill
          (sdf/translate 0 0.82
            (sdf/rounded-rect 0.52 0.10 0.05))
          (material
            ;; The tick tracks p-locks, not the gate: off steps are a
            ;; deliberate p-lock target (warp bpm, sampler ranges) and must
            ;; show the same indicator an on step shows. Only the muted
            ;; neutral fill still keys off `active`.
            :color (if (= plock-kind 0)
              (if (= active 1)
                (if (= muted 1)
                  border
                  (rgba 0 0 0 0))
                (rgba 0 0 0 0))
              (if (= muted 1)
                border
                (if (= plock-kind 2)
                  vcol
                  seqcol)))
            :shadow (shadow
              :color (if (= muted 1)
                (rgba 0 0 0 0)
                (if (= plock-kind 2)
                  (rgba variant-r variant-g variant-b 0.70)
                  (rgba 0 0 0 0)))
              :blur (if (= muted 1)
                0.0
                (if (= plock-kind 2) 0.12 0.0))
              :offset (vec2 0 0))))
        ;; toggled fill
        (sdf/fill (sdf/circle (if (= selected 1) 0.35 0.5))
          (material
            :lighting (lighting :edge-min -0.25 :edge-max 0.95
              :light (vec3 0.1 -1.4 0.3) :shininess 32.0)
            :color (if (= active 1)
              (if (= muted 1)
                (* 0.7 (eseq.materials/color offcol border))
                (eseq.materials/color
                  (rgba (* track-r 0.72) (* track-g 0.72) (* track-b 0.82) 1.0)
                  (rgba track-r track-g track-b 1.0)))
              (rgba 0 0 0 0))))))))

;; SEQ.song-track-governed carries one number per track (takes spec 10 UX):
;; 0 = the lane is not playing a take (pattern lanes stay fully editable —
;; jam with the step sequencer while the arrangement plays), 1 = the lane is
;; take-governed (dimmed steps + non-interactive grid + lit Back-to-Song
;; play button), 2 = a take lane the performer manually latched away
;; (editable again; the grey play button returns it to the song).
(def track-take-state (i)
  (let ((state (nth SEQ.song-track-governed i)))
    (if (= state nil) 0 state)))

(def track-song-governed? (i)
  (= (track-take-state i) 1))

;; Per-track take-lane indicator / Back-to-Song button: a play triangle that
;; sits lit green while a take governs the lane and grey while the lane is
;; manually latched (clicking then hands it back to the song). `take-state`
;; is the SEQ.song-track-governed value (0/1/2); at 0 the triangle renders
;; fully transparent — the box is ALWAYS in the layout so lanes flipping
;; between pattern and take never trigger a re-layout, only a repaint.
(defwidget seqv-back-to-song-icon
  :width 1.5 :height 1.5
  :state (take-state)
  :bindable (take-state)
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
            (rgba 0.62 0.63 0.67 1.0)
            (rgba 0 0 0 0)))))))


(defwidget seqv-back-to-song-bg
  :width 1.5 :height 1.5
  :state (take-state)
  :bindable (take-state)
  :shader
        (if (= take-state 1)
          (rgba 0.3 0.3 0.3 0.5)
          (if (= take-state 2)
            (rgba 0.32 0.33 0.37 0.5)
            (rgba 0 0 0 0))))

;; Legacy step tick, moved verbatim from ui/step-grid.lisp when the *metal*
;; buffer was unplugged from main.lisp. The expanded lane used it until its
;; toggle switched to the shared `seqv-step-shell`; kept only so a reloaded
;; step-grid.lisp still resolves (its identical copy is harmless — this one
;; loads later and wins).
(defwidget metal-track-tick
  :width 1.5 :height 1.5
  :state (active plocked selected track-r track-g track-b)
  :bindable (active plocked selected track-r track-g track-b)
  :shader
  (let ((sel-y (if (= selected 1) (* 0.1 (cos (* 3 itime))) 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (sdf/circle 1)
          (material
            :lighting (lighting :edge-min -0.35 :edge-max 0.5
              :light (vec3 0.0 -1.0 2.5) :shininess 32.0)
            :color
            (* (if (= active 1) 1 0.3)
               (eseq.materials/color
                 (rgba (* track-r 0.82) (* track-g 0.82) (* track-b 0.82) 1.0)
                 (rgba track-r track-g track-b 1.0)))))))))

(def mute-bg (active)
  (if active
    (rgba 0.08 0.09 0.10 1.0)
         (rgba 0.95 0.48 0.18 1.0)))

(def solo-bg (active)
  (if active
    (rgba 0.72 0.10 0.10 1.0)
    (rgba 0.08 0.09 0.10 1.0)))

;; Compact mixer track row — the common track actions plus an inline
;; meter/fader so the sequencer remains usable when the mixer is hidden.
;; The header lives in its own subtree so name/mute/solo/arm changes rerun
;; only this header instead of the whole track row (incl. its step grid).
(def track-header (i is-bare-track)
  (subtree :key (str "seqv-track-header-" (nth SEQ.track-ids i))
    (track-header-body i is-bare-track)))

(def track-volume-control (i)
  (v-stack (box :height 0.13 )
    (box
      :key (str "track-volume-control-" i)
      :width 8.2 :height 1.25
      :background "seqv-track-volume-meter"
      :level (track-peak i)
      :volume (track-volume-binding i)
      :track-r (track-color-r-binding i)
      :track-g (track-color-g-binding i)
      :track-b (track-color-b-binding i)
      :on-click (lambda (event) (set-track-volume-from-event i event))
      :on-drag (lambda (event) (set-track-volume-from-event i event))))
  )

(def track-header-body (i is-bare-track)
  (let ((name (nth SEQ.track-names i)))
    (box :background "seqv-track-container"
      :padding 0.1
      
      :on-click |x y r| (select-track-for-edit i)
      (h-stack :gap 0.4 :align :center
        (box
          :key (str "color-badge-" i)
          :width 0.68 :height 2.0
          :background "seqv-track-color-badge"
          :track-r (track-color-r-binding i)
          :track-g (track-color-g-binding i)
          :track-b (track-color-b-binding i)
          :on-click |x y r| (select-track-for-edit i))
        (box :width 2 :height 1.5
          :background "seqv-rec-arm-dot"
          :key (str "arm-" i)
          :active (if (nth SEQ.record-armed i) 1 0)
          :on-click |x y r| (do (activate-track-for-edit i) (seq-toggle-record-arm i)))
        (if is-bare-track (box :width 1.55))
        (button (str (+ i 1))
          :key (str "mute-" i)
          :width 1.55 :height 1.2 :padding 0 :font-size 10
          :border-color :transparent
          :background-color (mute-bg (nth SEQ.track-mutes i))
          :color (if (nth SEQ.track-mutes i) :gray :black)
          :on-click |x y r| (do (activate-track-for-edit i) (seq-toggle-track-mute i)))
        (button "S"
          :key (str "solo-" i)
          :width 1.55 :height 1.2 :padding 0 :font-size 10
          :background-color (solo-bg (nth SEQ.track-solos i))
          :border-color :transparent
          :color (if (nth SEQ.track-solos i) :white :gray)
          :on-click |x y r| (do (activate-track-for-edit i) (seq-toggle-track-solo i)))
        (box :width 8.6 :height 1
          :key (str "select-" i)
          :background-color :transparent
          :on-click |x y r| (select-track-for-edit i)
          ;; Arrangement/step track headers are device-view gestures:
          ;; double-click always enters FX mode rather than toggling.
          :on-double-click (lambda (evt) (show-fx-for-track i))
          (badge (track-name-display name)
            :key (str "track-name-label-" i)
            :icon (eseq.track-collapse/type-icon i)
            :font-size 11 :width 8.6 :height 1 :padding 0
            :h-align :left
            :background-color :transparent
            :border-color :transparent
            :highlight-color :transparent
            :shadow-color :transparent
            :color (if (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i))
              (rgba 0.4 0.4 0.4 0.6)
              :dim)
            :bg :transparent))
        (track-volume-control i)
        ;; Take-lane indicator (takes spec 10 UX): green = a take governs
        ;; the lane (steps dim, grid read-only); grey = the performer
        ;; latched the lane away — click returns it to the song; invisible
        ;; on pattern lanes. Always laid out — the reactive take-state only
        ;; repaints the widget, so pattern<->take flips never re-layout.
        (box :width 2 :height :fill 
          :background "seqv-back-to-song-bg"
          :take-state (bind-seq-nth "song-track-governed" i)
          (box :width 2 :height 1.5
            :background "seqv-back-to-song-icon"
            :key (str "back-to-song-" i)
            :take-state (bind-seq-nth "song-track-governed" i)
            :on-click |x y r|
            (if (> (track-take-state i) 0)
              (seq-song-back-to-song-track i)
              nil)))))))

(def track-actions (i)
  (h-stack :gap 0.35 :padding 0.85
    (box
      :key (str "expand-" i)
      :width 3.5 :height 1.0
      :background "seqv-ellipsis-button"
      :expanded (if (track-expanded? (nth SEQ.track-ids i)) 1 0)
      :on-click |x y r| (track-menu-click i))
    (box :width 0.1 :height 0.0)
    ))

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

(def drag-track nil)
(def duration-drag-source nil)

(def duration-edge? (evt)
  (let ((sx (get evt :sx)))
    (and (not (= sx nil)) (> sx 0.48))))

(def set-duration-from-drag (track source step)
  (do
    (seq-set-track track)
    (seq-set-step-param source :duration (max 1 (min 32 (+ (- step source) 1))))))

(def grid-step-select-drag-start (track step evt)
  (if (track-song-governed? track)
    nil
    (do
      (set! eseq.seq-core-state/selected-bus -1)
      (seq-set-track track)
      (set! drag-track track)
      (eseq.step-grid-interactions/step-select-drag-start step evt))))

(def grid-step-select-drag-over (track step evt)
  (if (track-song-governed? track)
    nil
    (if (and (= drag-track track) (not (= duration-drag-source nil)))
      (set-duration-from-drag track duration-drag-source step)
      (if (= drag-track track)
        (do
          (seq-set-track track)
          (eseq.step-grid-interactions/step-select-drag-over-for-track track step evt))
        nil))))

;; Song-governed lanes are non-interactive (takes spec 10 UX): while the
;; arrangement holds launch authority the Seq grid is a dimmed read-only view
;; of the session pattern — edits would silently target a pattern the lane is
;; not playing.
(def grid-step-pointer-down (track step evt)
  (if (track-song-governed? track)
    nil
    (let ((use-selection (= SEQ.current-track track)))
      (do
        (set! eseq.seq-core-state/selected-bus -1)
        (seq-set-track track)
        (set! drag-track track)
        (if (and (seq-track-step-active? track step) (not (eseq.step-grid-interactions/selection-click? evt)) (duration-edge? evt))
          (do
            (set! duration-drag-source step)
            (eseq.step-grid-interactions/step-clear-drag-state)
            (eseq.seq-core-state/cool-off-follow)
            (eseq.step-grid-interactions/set-track-cursor-step step)
            (set-duration-from-drag track step step))
          (eseq.step-grid-interactions/step-pointer-down-for-track track step evt use-selection))))))

(def grid-step-double-click (track step evt)
  (if (track-song-governed? track)
    nil
    (do
      (seq-set-track track)
      (eseq.step-grid-interactions/step-double-click-for-track track step evt))))

(def grid-step-pointer-up (track step evt)
  (do
    (if (and (= drag-track track)
          (= duration-drag-source nil)
          (not (track-song-governed? track)))
      (do
        (seq-set-track track)
        (eseq.step-grid-interactions/step-pointer-up step evt))
      nil)
    (set! drag-track nil)
    (set! duration-drag-source nil)))

;; Single tight step button (no slider, no number).
(def track-step-value (lists track step fallback)
  (let ((track-list (if (< track (len lists)) (nth lists track) '())))
    (if (< step (len track-list))
      (nth track-list step)
      fallback)))

(def step-odd (step)
  (let ((odd1 (mod (floor (/ step 4)) 2))
      (odd2 (mod (floor (/ step 32)) 2)))
    (if (= odd2 1) (if (= odd1 1) 0 1) odd1)))

(def step-cell (track step visible)
  ;; Step cells use the step-color channels: same as the track color but
  ;; additionally dimmed while the lane is take-governed. Muting is passed
  ;; separately so the shader can replace colored layers with opaque neutral
  ;; materials instead of receiving an already-dimmed track color.
  (let ((track-r (bind-seq-nth "step-color-r-effective" track))
      (track-g (bind-seq-nth "step-color-g-effective" track))
      (track-b (bind-seq-nth "step-color-b-effective" track))
      (muted (bind-seq-nth "track-muted-effective" track))
      (plock-kind (bind-seq (str "seq-track-step-plock-kind-" track "-" step)))
      (variant-r (bind-seq (str "seq-track-step-variant-r-" track "-" step)))
      (variant-g (bind-seq (str "seq-track-step-variant-g-" track "-" step)))
      (variant-b (bind-seq (str "seq-track-step-variant-b-" track "-" step))))
    (box
      :width 3.05 :height 1.55
      :key (str "step-cell-" track "-" step)
      :on-mouse-down (lambda (evt)
        (if visible
          (grid-step-pointer-down track step evt)
          nil))
      :on-drag (lambda (evt)
        (if visible
          (grid-step-select-drag-over track step evt)
          nil))
      :on-mouse-up (lambda (evt)
        (if visible
          (grid-step-pointer-up track step evt)
          nil))
      :on-double-click (lambda (evt)
        (if visible
          (grid-step-double-click track step evt)
          nil))
      :active (cursor-highlight-binding track step)
      :selected (track-selected-binding track)
      :hide (if visible 0 1)
      :background "cursor-highlight"
      (box
        :width 3.05 :height 1.55
        :align :center
        :active (bind-seq (str "seq-track-step-active-" track "-" step))
        :plock-kind plock-kind
        :selected (bind-seq (str "seq-track-step-selected-" track "-" step))
        :duration (bind-seq (str "seq-track-step-duration-" track "-" step))
        :muted muted
        :hide (if visible 0 1)
        :track-r track-r :track-g track-g :track-b track-b
        :variant-r variant-r :variant-g variant-g :variant-b variant-b
        :color :sequencer-step-border
        :selected-color :sequencer-step-selected-border
        :off-fill (if (= (step-odd step) 1)
          :sequencer-step-off-fill-alt
          :sequencer-step-off-fill)
        :background "seqv-step-shell"))))

(def playhead-row (track track-id row)
  (box
    :key (str "playhead-row-" track-id "-" row)
    :width 48.8 :height 0.24
    :background "seqv-playhead-row-bar"
    :col (bind-seq (str "track-playhead-row-" track "-" row))))

(def track-num-steps (track)
  (if (< track (len SEQ.track-num-steps))
    (nth SEQ.track-num-steps track)
    16))

(def expanded-track-color-r (track)
  (nth (track-color track) 0))

(def expanded-track-color-g (track)
  (nth (track-color track) 1))

(def expanded-track-color-b (track)
  (nth (track-color track) 2))

(def expanded-slider-fill (track)
  (rgba (expanded-track-color-r track) (expanded-track-color-g track) (expanded-track-color-b track) 1.0))

(def expanded-slider-muted-fill (track)
  (rgba
    (+ (* (expanded-track-color-r track) 0.30) (* 0.08 0.70))
    (+ (* (expanded-track-color-g track) 0.30) (* 0.08 0.70))
    (+ (* (expanded-track-color-b track) 0.30) (* 0.12 0.70))
    0.50))

(def expanded-slider-muted-dot (track)
  (rgba
    (+ (* (expanded-track-color-r track) 0.28) (* 0.25 0.72))
    (+ (* (expanded-track-color-g track) 0.28) (* 0.25 0.72))
    (+ (* (expanded-track-color-b track) 0.28) (* 0.30 0.72))
    0.55))

(def track-current-step (track track-id)
  (track-cursor track-id))

(def page-count (track)
  (max 1 (floor (/ (+ (track-num-steps track) (- eseq.seq-core-state/page-size 1)) eseq.seq-core-state/page-size))))

(def track-current-page (track track-id)
  (min (floor (/ (track-current-step track track-id) eseq.seq-core-state/page-size)) (- (page-count track) 1)))

(def playhead-page (track)
  (let ((page (reactive-get "SEQ" (str "track-playhead-page-" track))))
    (min
      (if page page 0)
      (- (page-count track) 1))))

(def visible-page (track track-id)
  (if (and SEQ.playing SEQ.auto-follow (not (seq-has-selection?)))
    (playhead-page track)
    (track-current-page track track-id)))

(def page-offset (track track-id)
  (* (visible-page track track-id) eseq.seq-core-state/page-size))

(def expanded-step-index (track track-id i)
  (+ (page-offset track track-id) i))

(def expanded-step-visible? (track track-id i)
  (< (expanded-step-index track track-id i) (track-num-steps track)))

(def sync-expanded-step-slots-for (track track-id)
  (seqv-sync-expanded-step-slots
    track
    track-id
    (visible-page track track-id)
    (track-param-mode track-id)
    (track-current-step track track-id)))

(def slot-field (name track-id slot)
  (str "seqv-slot-" name "-" track-id "-" slot))

(def slot-param-field (kind track-id mode slot)
  (str "seqv-slot-param-" kind "-" track-id "-" mode "-" slot))

(def slot-page-active-field (track-id page)
  (str "seqv-page-active-" track-id "-" page))

(def slot-step-index-binding (track-id slot)
  (bind-seq (slot-field "step-index" track-id slot)))

(def slot-step-index-value (track-id slot)
  (reactive-value (slot-step-index-binding track-id slot)))

(def slot-visible-binding (track-id slot)
  (bind-seq (slot-field "visible" track-id slot)))

(def slot-visible? (track-id slot)
  (> (reactive-value (slot-visible-binding track-id slot)) 0.5))

(def slot-label-binding (track-id slot)
  (bind-seq (slot-field "step-label" track-id slot)))

(def slot-active-binding (track-id slot)
  (bind-seq (slot-field "active" track-id slot)))

(def slot-plocked-binding (track-id slot)
  (bind-seq (slot-field "plocked" track-id slot)))

(def slot-plock-kind-binding (track-id slot)
  (bind-seq (slot-field "plock-kind" track-id slot)))

(def slot-variant-r-binding (track-id slot)
  (bind-seq (slot-field "variant-r" track-id slot)))

(def slot-variant-g-binding (track-id slot)
  (bind-seq (slot-field "variant-g" track-id slot)))

(def slot-variant-b-binding (track-id slot)
  (bind-seq (slot-field "variant-b" track-id slot)))

(def slot-selected-binding (track-id slot)
  (bind-seq (slot-field "selected" track-id slot)))

(def slot-playhead-binding (track-id slot)
  (bind-seq (slot-field "playhead-active" track-id slot)))

(def slot-cursor-binding (track-id slot)
  (bind-seq (slot-field "cursor-active" track-id slot)))

(def slot-param-slider-binding (track-id mode slot)
  (bind-seq (slot-param-field "slider" track-id mode slot)))

(def slot-param-haptic-binding (track-id mode slot)
  (bind-seq (slot-param-field "haptic" track-id mode slot)))

(def page-active-binding (track-id page)
  (bind-seq (slot-page-active-field track-id page)))

(def expanded-cursor-param-binding (track-id)
  (bind-seq (str "seqv-cursor-param-value-" track-id)))

(def expanded-cursor-sync-index-binding (track-id)
  (bind-seq (str "seqv-cursor-sync-index-" track-id)))

(def step-active-binding (track step)
  (bind-seq (str "seq-track-step-active-" track "-" step)))

(def step-plocked-binding (track step)
  (bind-seq (str "seq-track-step-plocked-" track "-" step)))

(def step-selected-binding (track step)
  (bind-seq (str "seq-track-step-selected-" track "-" step)))

(def step-param-slider-binding (track mode step)
  (bind-seq (str "seq-track-step-param-slider-" track "-" mode "-" step)))

(def step-param-haptic-binding (track mode step)
  (bind-seq (str "seq-track-step-param-haptic-" track "-" mode "-" step)))

(def expanded-sync-label-index (label)
  (reduce |acc index|
    (if (= label (nth SEQ.sync-labels index)) index acc)
    0
    (range 0 (len SEQ.sync-labels))))

(def set-expanded-cursor (track track-id step)
  (do
    (eseq.step-grid-interactions/set-track-cursor-step step)))

(def expanded-step-click (track track-id step evt)
  (if (expanded-step-visible? track track-id (- step (page-offset track track-id)))
    (do
      (activate-track-for-edit track)
      (eseq.seq-core-state/cool-off-follow)
      (set-expanded-cursor track track-id step)
      (if (eseq.step-grid-interactions/selection-click? evt)
        (eseq.step-grid-interactions/step-select-drag-start step evt)
        (seq-clear-selection)))
    nil))

(def expanded-step-drag (track track-id step evt)
  (do
    (eseq.step-grid-interactions/step-select-drag-over-for-track-no-cursor track step evt)))

(def expanded-step-pointer-down (track track-id step evt)
  (let ((use-selection (= SEQ.current-track track)))
    (do
      (activate-track-for-edit track)
      (set-expanded-cursor track track-id step)
      (eseq.step-grid-interactions/step-pointer-down-for-track track step evt use-selection))))

(def expanded-step-pointer-up (track track-id step evt)
  (do
    (activate-track-for-edit track)
    (set-expanded-cursor track track-id step)
    (eseq.step-grid-interactions/step-pointer-up step evt)))

(def expanded-slot-click (track track-id slot evt)
  (let ((step (slot-step-index-value track-id slot)))
    (if (>= step 0)
      (expanded-step-click track track-id step evt)
      nil)))

(def expanded-slot-drag (track track-id slot evt)
  (let ((step (slot-step-index-value track-id slot)))
    (if (>= step 0)
      (expanded-step-drag track track-id step evt)
      nil)))

(def expanded-slot-pointer-down (track track-id slot evt)
  (let ((step (slot-step-index-value track-id slot)))
    (if (>= step 0)
      (expanded-step-pointer-down track track-id step evt)
      nil)))

(def expanded-slot-pointer-up (track track-id slot evt)
  (let ((step (slot-step-index-value track-id slot)))
    (if (>= step 0)
      (expanded-step-pointer-up track track-id step evt)
      nil)))

(def expanded-step-double-click (track track-id step evt)
  (do
    (activate-track-for-edit track)
    (eseq.step-grid-interactions/step-double-click-for-track track step evt)))

(def expanded-slot-double-click (track track-id slot evt)
  (let ((step (slot-step-index-value track-id slot)))
    (if (>= step 0)
      (expanded-step-double-click track track-id step evt)
      nil)))

(def set-expanded-slot-param (track track-id slot mode slider-value)
  (let ((step (slot-step-index-value track-id slot)))
    (if (>= step 0)
      (set-expanded-step-param track track-id step mode slider-value)
      nil)))

(def set-expanded-step-param (track track-id step mode slider-value)
  (do
    (activate-track-for-edit track)
    (eseq.seq-core-state/cool-off-follow)
    (set-expanded-cursor track track-id step)
    (if (eseq.seqv-track-params/seqv-process-lane-mode? mode)
      (eseq.step-grid-interactions/seq-set-process-lane-from-step
        track
        mode
        step
        (eseq.seqv-track-params/seqv-track-step-slider-param-value track mode slider-value))
      (eseq.step-grid-interactions/seq-set-step-param-from-step
        step
        (eseq.seqv-track-params/seqv-param-keyword mode)
        (eseq.seqv-track-params/seqv-step-slider-param-value mode slider-value)))))

(def set-expanded-current-param (track track-id mode value)
  (do
    (activate-track-for-edit track)
    (eseq.seq-core-state/cool-off-follow)
    (eseq.step-grid-interactions/set-track-cursor-step (track-current-step track track-id))
    (if (eseq.seqv-track-params/seqv-process-lane-mode? mode)
      (eseq.step-grid-interactions/seq-set-process-lane-from-step
        track
        mode
        (track-current-step track track-id)
        (eseq.seqv-track-params/seqv-track-step-param-value track mode value))
      (eseq.step-grid-interactions/seq-set-step-param-from-step
        (track-current-step track track-id)
        (eseq.seqv-track-params/seqv-param-keyword mode)
        (eseq.seqv-track-params/seqv-step-param-value mode value)))))

(def set-expanded-timebase (track label)
  (let ((plock-selected
      (and (< eseq.seq-core-state/selected-bus 0) (= SEQ.current-track track) (seq-has-selection?))))
    (do
      (activate-track-for-edit track)
      (eseq.seq-core-state/cool-off-follow)
      (if plock-selected
        (seq-plock-timebase label)
        (seq-set-timebase label)))))

(def goto-page (track track-id page)
  (let ((step (min (* page eseq.seq-core-state/page-size) (- (max 1 (track-num-steps track)) 1))))
    (do
      (activate-track-for-edit track)
      (eseq.seq-core-state/cool-off-follow)
      (eseq.step-grid-interactions/set-track-cursor-step step))))

(def double-track-pattern (track track-id)
  (do
    (activate-track-for-edit track)
    (eseq.seq-core-state/cool-off-follow)
    (seq-double-track-pattern)
    (sync-all-track-cursors-to-global)))

(def halve-track-pattern (track track-id)
  (do
    (activate-track-for-edit track)
    (eseq.seq-core-state/cool-off-follow)
    (seq-halve-track-pattern)
    (sync-all-track-cursors-to-global)))

(def param-tab-width (mode)
  7.8)

(def clip-label (text max-chars)
  (let ((s (str text)))
    (if (> (len s) max-chars)
      (str (substring s 0 (- max-chars 2)) "..")
      s)))

(def param-header-name (track mode)
  (if (eseq.seqv-track-params/seqv-process-lane-mode? mode)
    (clip-label (eseq.seqv-track-params/seqv-track-param-name track mode) 28)
    (eseq.seqv-track-params/seqv-param-name mode)))

(def param-header-width (mode)
  (if (eseq.seqv-track-params/seqv-process-lane-mode? mode) 17.8 6.4))

(def param-tab (track track-id mode tab-label)
  (box :width (param-tab-width mode) :height 2
    :key (str "expanded-param-tab-" track-id "-" mode)
    :bg (if (= (track-param-mode track-id) mode) (eseq.seqv-track-params/seqv-param-color mode) :dark-gray)
    :on-click |x y r| (do (activate-track-for-edit track) (set-track-param-mode track-id mode))
    (label tab-label :font-size 12
      :color (if (= (track-param-mode track-id) mode) :primary :dim)
      :bg :transparent)))

(def process-lane-option-label (track lane-idx)
  (let ((lane (nth (eseq.seqv-track-params/seqv-track-process-lanes track) lane-idx)))
    (str (+ lane-idx 1) " " (get lane :short-label))))

(def process-lane-options (track)
  (append
    (list "none")
    (map
      (lambda (lane-idx) (process-lane-option-label track lane-idx))
      (range 0 (len (eseq.seqv-track-params/seqv-track-process-lanes track))))))

(def process-lane-selector-value (track mode)
  (if (eseq.seqv-track-params/seqv-process-lane-mode? mode)
    (let ((lane-idx (eseq.seqv-track-params/seqv-process-lane-index mode)))
      (if (and (>= lane-idx 0) (< lane-idx (len (eseq.seqv-track-params/seqv-track-process-lanes track))))
        (process-lane-option-label track lane-idx)
        "none"))
    "none"))

(def process-lane-selector-index (track label)
  (if (= label "none")
    -1
    (reduce |acc lane-idx|
      (if (= label (process-lane-option-label track lane-idx)) lane-idx acc)
      -1
      (range 0 (len (eseq.seqv-track-params/seqv-track-process-lanes track))))))

(def selected-process-lane (track mode)
  (if (eseq.seqv-track-params/seqv-process-lane-mode? mode)
    (let ((lane-idx (eseq.seqv-track-params/seqv-process-lane-index mode)))
      (if (and (>= lane-idx 0) (< lane-idx (len (eseq.seqv-track-params/seqv-track-process-lanes track))))
        (nth (eseq.seqv-track-params/seqv-track-process-lanes track) lane-idx)
        nil))
    nil))

(def select-process-lane-option (track track-id label)
  (let ((lane-idx (process-lane-selector-index track label)))
    (do
      (activate-track-for-edit track)
      (if (>= lane-idx 0)
        (set-track-param-mode track-id (+ eseq.seqv-track-params/seqv-process-lane-mode-offset lane-idx))
        (if (eseq.seqv-track-params/seqv-process-lane-mode? (track-param-mode track-id))
          (set-track-param-mode track-id 3)
          nil)))))

(def process-lane-selector (track track-id mode)
  (dropdown
    :value (process-lane-selector-value track mode)
    :key (str "expanded-process-lane-selector-" track-id)
    :options (process-lane-options track)
    :on-change (lambda (v) (select-process-lane-option track track-id v))
    :width 14.8 :height 1.45 :font-size 10))

(def expanded-track-quick-controls (track track-id)
  (let ((mode (track-param-mode track-id)))
    (v-stack 
      (box :height 0.4 :width 1)
      (h-stack :gap 0.55 :align :center
        (box :width (param-header-width mode) :height 1.3
          :key (str "expanded-step-summary-" track-id)
          (label (param-header-name track mode)
            :font-size 11 :width (param-header-width mode) :color :white :bg :transparent))
        (if (= mode 5)
          (dropdown
            :key (str "expanded-sync-picker-" track-id)
            :value-index (expanded-cursor-sync-index-binding track-id)
            :options SEQ.sync-labels
            :on-change (lambda (label)
              (set-expanded-current-param track track-id mode (expanded-sync-label-index label)))
            :width 8 :height 1.3 :font-size 11)
          (number-picker :key (str "expanded-param-number-picker-" track-id)
            :border-color :white
            :value (expanded-cursor-param-binding track-id)
            :min (eseq.seqv-track-params/seqv-track-param-min track mode) :max (eseq.seqv-track-params/seqv-track-param-max track mode) :decimals (eseq.seqv-track-params/seqv-track-param-decimals track mode)
            :on-change (lambda (v) (set-expanded-current-param track track-id mode v))
            :width 8 :height 1.3 :font-size 11))
        (h-stack :gap 0.4 :align :center
          (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
            :key (str "expanded-half-" track-id)
            :on-click |x y r| (halve-track-pattern track track-id)
            (v-stack :align :center
              (label "-"
                :font-size 12
                :color :white
                :bg :transparent)))
          (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
            :key (str "expanded-double-" track-id)
            :on-click |x y r| (double-track-pattern track track-id)
            (v-stack :align :center
              (label "+"
                :font-size 12
                :color :white
                :bg :transparent)))
          (box :background "transport-btn-bg" :padding 0.2 :height 1.4
            :key (str "expanded-pages-" track-id)
            (h-stack :gap 0.1 :align :center
              (each (range 0 (page-count track)) |page|
                (box :width eseq.step-grid-interactions/page-button-width :height 1.1
                  :key (str "expanded-page-" track-id "-" page)
                  :background "pattern-pill-bg"
                  :active (page-active-binding track-id page)
                  :style eseq.transport/pattern-control-style
                  :on-click |x y r| (goto-page track track-id page)
                  (v-stack :align :center
                    (label (fmt " {} " (+ page 1))
                      :font-size 11
                      :active (page-active-binding track-id page)
                      :active-color :white
                      :color :dim
                      :bg :transparent)))))))))))

(def expanded-track-editor (track track-id)
  (let ((mode (track-param-mode track-id)))
    (box :padding 0.25
      (box 
        :background-color (rgba 0.1 0.1 0.1 0.2) :corner-radius 8
        (v-stack :width :fill :padding 0.35 :gap 0.1
          (h-stack :gap 0.5
            (box :width 1)
            (param-tab track track-id 0 "vel")
            (param-tab track track-id 1 "dur")
            (param-tab track track-id 3 "tpose")
            (param-tab track track-id 4 "pan")
            (param-tab track track-id 5 "sync")
            (param-tab track track-id 6 "delay")
            (process-lane-selector track track-id mode)
            (h-stack :align :center :gap 0.35
              (dropdown :value (track-timebase track)
                :key (str "expanded-timebase-" track-id)
                :options timebase-options
                :on-change (lambda (v) (set-expanded-timebase track v))
                :width 6 :height 1.45 :font-size 10)))
          
          (grid
            :cols 16
            :col-width 4
            :row-height expanded-step-row-height
            :align :stretch
            (each (range 0 eseq.seq-core-state/page-size) |i|
              (box :padding expanded-step-column-padding
                :key (str "expanded-step-column-" track-id "-" i)
                :background "cursor-highlight"
                :active (slot-cursor-binding track-id i)
                :selected (track-selected-binding track)
                :on-click (lambda (evt)
                  (expanded-slot-click track track-id i evt))
                :on-drag (lambda (evt)
                  (expanded-slot-drag track track-id i evt))
                (v-stack :align :center :gap expanded-step-column-gap
                  (let ((active-ref (slot-active-binding track-id i))
                      (selected-ref (slot-selected-binding track-id i))
                      (track-r (expanded-track-color-r track))
                      (track-g (expanded-track-color-g track))
                      (track-b (expanded-track-color-b track)))
                    (list
                      (vslider :height expanded-step-slider-height
                        :key (str "expanded-step-slider-" track-id "-" i)
                        :width (if (= mode 5) 2 1)
                        :min (eseq.seqv-track-params/seqv-track-param-slider-min track mode) :max (eseq.seqv-track-params/seqv-track-param-slider-max track mode)
                        :origin (eseq.seqv-track-params/seqv-track-param-origin track mode)
                        :value (slot-param-slider-binding track-id mode i)
                        :haptic-value (slot-param-haptic-binding track-id mode i)
                        :haptic-min (eseq.seqv-track-params/seqv-track-param-min track mode)
                        :haptic-max (eseq.seqv-track-params/seqv-track-param-max track mode)
                        :haptic-pivot-position (eseq.seqv-track-params/seqv-param-haptic-pivot-position mode)
                        :haptic-pivot-value (eseq.seqv-track-params/seqv-track-param-haptic-pivot-value track mode)
                        :haptic-exponent (eseq.seqv-track-params/seqv-param-haptic-exponent mode)
                        :items (if (= mode 5) SEQ.sync-labels '())
                        :font-size 11
                        :color :white
                        :fill (expanded-slider-fill track)
                        :dot-color :dark-gray
                        :active active-ref
                        :track-r track-r
                        :track-g track-g
                        :track-b track-b
                        :material (eseq.sequencer/step-slider-track-material)
                        :on-change (lambda (v)
                          (set-expanded-slot-param track track-id i mode v)))
                      ;; Same shell widget as the compact grid's step-cell, so
                      ;; the expanded toggle inherits its p-lock tick (incl.
                      ;; variant colors) and active/selected/muted rendering.
                      (box
                        :key (str "expanded-step-toggle-" track-id "-" i)
                        :active active-ref
                        :plock-kind (slot-plock-kind-binding track-id i)
                        :selected selected-ref
                        :duration 0
                        :muted (bind-seq-nth "track-muted-effective" track)
                        :hide 0
                        :track-r (bind-seq-nth "step-color-r-effective" track)
                        :track-g (bind-seq-nth "step-color-g-effective" track)
                        :track-b (bind-seq-nth "step-color-b-effective" track)
                        :variant-r (slot-variant-r-binding track-id i)
                        :variant-g (slot-variant-g-binding track-id i)
                        :variant-b (slot-variant-b-binding track-id i)
                        :color :sequencer-step-border
                        :selected-color :sequencer-step-selected-border
                        :off-fill (if (= (step-odd i) 1)
                          :sequencer-step-off-fill-alt
                          :sequencer-step-off-fill)
                        :background "seqv-step-shell"
                        :align :center :width 3 :height expanded-step-toggle-height
                        :on-mouse-down (lambda (evt)
                          (expanded-slot-pointer-down track track-id i evt))
                        :on-drag (lambda (evt)
                          (expanded-slot-drag track track-id i evt))
                        :on-mouse-up (lambda (evt)
                          (expanded-slot-pointer-up track track-id i evt))
                        :on-double-click (lambda (evt)
                          (expanded-slot-double-click track track-id i evt)))))
                  (number-label
                    :key (str "expanded-step-label-" track-id "-" i)
                    :value (slot-label-binding track-id i)
                    :active (slot-selected-binding track-id i)
                    :active-color :yellow
                    :decimals 0
                    :width 2.8
                    :height expanded-step-label-height
                    :h-align :center
                    :font-size 10 :bg :transparent
                    :color :dim)
                  (subtree :key (str "seqv-expanded-step-playhead-probe-" track-id "-" i)
                    (step-playhead-dot
                      :active (slot-playhead-binding track-id i)))))))))
    )
  )
)

(def track-grid (track-idx)
  (let ((num-steps (nth SEQ.track-num-steps track-idx))
      (rows (max 1 (floor (/ (+ num-steps (- row-width 1)) row-width)))))
    (box :padding 0.15
      (box :background-color :buffer-bg
        (v-stack :gap -0.04
          (box :width 0.1 :height 0.342 :bg :transparent)
          (each (range 0 rows) |row|
            (v-stack :gap -0.16
              (h-stack
                
                (box :v-align :center :height 1.1 :padding 0.5
                  ;; `active` is a reactive float slot the label reads at paint
                  ;; time, so the playing row brightens without re-evaluating or
                  ;; re-laying out the grid.
                  (label (+ row 1)
                    :color (if (> rows 1) :dim :buffer-bg)
                    :active (if (> rows 1) (bind-seq (str "track-playhead-row-active-" track-idx "-" row)) 0)
                    :active-color :white
                    :width 0.1 :bg :transparent :font-size 8)
                  )
                (h-stack :gap 0.0
                  (each (range 0 row-width) |col|
                    (let ((step (+ (* row row-width) col)))
                      (step-cell
                        track-idx
                        step
                        (< step num-steps))))))
              (h-stack (box :width 1)
                (playhead-row track-idx (nth SEQ.track-ids track-idx) row)))))))
    )
  )

;; Which sound payloads a track will accept as a replacement of what it already
;; plays. Shared by the track row and the pad grid's occupied cells: dropping on
;; a pad replaces that pad's sound on its member track, so the two must agree.
(def sound-drop-types (i)
  (if (eseq.track-collapse/replaceable-instrument? i)
    (list "sample" "instrument" "sound")
    (if (eseq.track-collapse/sound-replaceable? i) (list "sample" "sound") (list "sample"))))

;; One track's grid row. Rack members render through this exact path — a rack
;; member is an ordinary track, so its pattern length, timebase p-locks,
;; accumulator and expanded step editor all come along for free.
;; :muted is a binding (not a value read) so mute/solo changes update the row
;; chrome without rerunning the enclosing subtree.
(def track-row (i is-bare-track)
  (box :width :fill
      :key (str "track-drop-" i)
      :selected (track-selected-binding i)
      :muted (bind-seq-nth "track-muted-effective" i)
      :background-color :buffer-bg
      :selected-background-color :mixer-strip-selected-bg
      :muted-background-color :mixer-strip-muted-bg
      :border-width 2
      :corner-radius 10
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :muted-border-color :mixer-strip-border
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types (sound-drop-types i)
      :drop-meta (dict :kind "track" :track i)
      :on-drop (lambda (event) (drop-on-track event))
      :padding 0.0145
      :on-click |x y r| (select-track-for-edit i)
      (if (track-expanded? (nth SEQ.track-ids i))
        (v-stack 
          :width :fill :gap 0.2
          (h-stack :padding 0.1 :width :fill :gap 0.6 :align :start
            (track-header i is-bare-track)
            (expanded-track-quick-controls i (nth SEQ.track-ids i))
            (box :flex 1 :width 0 :height 0.1 :bg :transparent)
            (track-actions i))
          (expanded-track-editor i (nth SEQ.track-ids i)))
        (h-stack :padding 0.1 :width :fill :gap 0.6 :align :start
          (v-stack (box :height 0.1)
            (track-header i is-bare-track))
          (track-grid i)
          (box :flex 1 :width 0 :height 0.1 :bg :transparent)
          (track-actions i)))))

;; ── Track groups ────────────────────────────────────────────────────────
;; Regular groups and drum racks share one nested block: a header row owns the
;; backing bus's mute/solo/volume, name and collapse state, with member tracks
;; beneath as ordinary rows. A drum rack additionally owns pad-play arming and
;; rack-specific member chrome; regular groups deliberately have no Arm control.

(def group-ui-kind (gidx)
  (if (eseq.drum-rack-v2/rack? gidx) "rack" "group"))

(def group-element-key (gidx element)
  (str (group-ui-kind gidx) "-" element "-" (eseq.drum-rack-v2/group-id gidx)))

;; Keep group fills opaque because the rounded-box renderer draws its border
;; behind the inset fill. Reproduce the old alpha-0.22 track tint by compositing
;; it over the active theme's buffer surface in Lisp; selection then changes
;; only the border without changing the original fill appearance.
(def group-container-bg (c)
  (let ((bg THEME.buffer_bg))
    (rgba
      (+ (* (nth c 0) 0.22) (* (nth bg 0) 0.78))
      (+ (* (nth c 1) 0.22) (* (nth bg 1) 0.78))
      (+ (* (nth c 2) 0.22) (* (nth bg 2) 0.78))
      1.0)))

(def group-bus-volume-from-event (bus-idx event)
  (let ((sx (get event :sx)))
    (if (= sx nil)
      nil
      (seq-set-bus-volume bus-idx (max 0.0 (min 1.0 (* 0.5 (+ sx 1.0))))))))

;; Volume/meter for a group's backing bus. Group bus lists are rebuilt per
;; frame, so an unresolved index degrades to a spacer instead of an error.
(def group-volume-control (gidx bus-idx)
  (let ((c (eseq.drum-rack-v2/color gidx)))
    (v-stack (box :height 0.13)
      (if (>= bus-idx 0)
        (box
          :key (group-element-key gidx "volume-control")
          :width 8.2 :height 1.25
          :background "seqv-track-volume-meter"
          :level (bind-seq (str "bus-peak-" bus-idx))
          :volume (bind-seq-nth "bus-volumes" bus-idx)
          :track-r (nth c 0)
          :track-g (nth c 1)
          :track-b (nth c 2)
          :on-click (lambda (event) (group-bus-volume-from-event bus-idx event))
          :on-drag (lambda (event) (group-bus-volume-from-event bus-idx event)))
        (box :width 8.2 :height 1.25 :bg :transparent)))))

;; Selecting a group selects its backing bus, exactly as the mixer header does,
;; so the fx panel follows the group chain.
(def select-group (gidx)
  (let ((bus-idx (eseq.drum-rack-v2/bus-index gidx)))
    (if (>= bus-idx 0)
      (set! eseq.seq-core-state/selected-bus bus-idx)
      false)))

;; Selection visibility rides the *sel-sync* SEQV field, never a raw
;; `selected-bus` read: this block wraps every member row, so a render-time
;; read here re-rendered the whole group on each selection (eseq-4jv).
(def group-selected-binding (gidx)
  (eseq.seq-core-state/group-selected-vis-binding (eseq.drum-rack-v2/group-id gidx)))

;; ── Group member chrome ─────────────────────────────────────────────────
;; Every group member gets the same indented prefix used by drum racks. It
;; visually connects the ordinary track row to the containing group header.

(def rack-pad-badge (gidx pad)
  (h-stack :gap 0.08 :align :center
    (button "-"
      :key (str "rack-pad-down-" (eseq.drum-rack-v2/group-id gidx) "-" (get pad :pad-note))
      :width 1.0 :height 0.9 :padding 0 :font-size 8
      :background-color '(rgba 0.1 0.1 0.1 1.0)
      :border-color :transparent
      :color :dim
      :on-click |x y r| (eseq.drum-rack-v2/nudge-pad-note gidx pad -1))
    (badge (get pad :label)
      :key (str "rack-pad-note-" (eseq.drum-rack-v2/group-id gidx) "-" (get pad :pad-note))
      :font-size 9 :width 2.6 :height 0.9 :padding 0
      :h-align :center
      :background-color '(rgba 0.18 0.22 0.23 1.0)
      :border-color :transparent
      :highlight-color :transparent
      :shadow-color :transparent
      :color :white)
    (button "+"
      :key (str "rack-pad-up-" (eseq.drum-rack-v2/group-id gidx) "-" (get pad :pad-note))
      :width 1.0 :height 0.9 :padding 0 :font-size 8
      :background-color '(rgba 0.1 0.1 0.1 1.0)
      :border-color :transparent
      :color :dim
      :on-click |x y r| (eseq.drum-rack-v2/nudge-pad-note gidx pad 1))))

(def group-member-chrome (gidx i)
  (group-track-indicator
    :key (str "group-track-indicator-" (eseq.drum-rack-v2/group-id gidx) "-" i))
  )

(def group-member-row (gidx i)
  (h-stack :width :fill :gap 0.15 :align :start
    (group-member-chrome gidx i)
    (box :width 0 :flex 1 (track-row i false))))

;; ── Pad grid performance view ───────────────────────────────────────────
;; A 4x4 VIEW over the pad map — finger drumming and slot browsing only. It
;; owns no sequencing state: a cell reads its pad from SEQ.groups and a hit
;; goes straight down the live pad path. The grid renders in the *fx* buffer's
;; rack panel (ui/effects/buffers.lisp); the sequencer header carries no pad
;; toggle of its own.
;;
;; Cells are NOTE-POSITIONAL: a cell is a fixed MIDI note and draws whichever
;; pad answers to it, so label and position can never contradict each other.
;; The note geometry — octave-aligned pages, lowest note bottom-left — lives
;; in eseq.drum-rack-v2; what lives here is which page is showing.

;; Which page of notes the grid shows, as (group id, page). One rack panel is
;; on screen at a time, so a single slot scoped by group id IS per-rack state:
;; a rack the page was never set for — or any rack other than the one last
;; paged — re-derives its own default instead of inheriting a stale page.
(defstate pad-grid-page-group -1)
(defstate pad-grid-page 0)

(def pad-page (gidx)
  (if (= pad-grid-page-group (eseq.drum-rack-v2/group-id gidx))
    (eseq.drum-rack-v2/clamp-pad-page pad-grid-page)
    (eseq.drum-rack-v2/default-pad-page gidx)))

(def set-pad-page (gidx page)
  (do
    (set! pad-grid-page-group (eseq.drum-rack-v2/group-id gidx))
    (set! pad-grid-page (eseq.drum-rack-v2/clamp-pad-page page))))

;; The note a grid position names on the visible page — the cell's identity,
;; occupied or not.
(def pad-cell-note (gidx cell)
  (eseq.drum-rack-v2/cell-note (pad-page gidx) cell))

;; Pad drawn at a grid position: the one whose pad note IS this cell's note,
;; or nil. Pads on other pages simply do not render here; the grid is sparse
;; by design and never compacts.
(def pad-at (gidx cell)
  (eseq.drum-rack-v2/pad-at-note gidx (pad-cell-note gidx cell)))

(def pad-cell-name (pad)
  (let ((track (get pad :track)))
    (if (and (>= track 0) (< track SEQ.num-tracks))
      (nth SEQ.track-names track)
      "")))

;; The pad the rack's *fx* panel is focused on, as (group id, pad note) — the
;; pad map's own key, so the focus survives track reindexing exactly as chokes
;; and note nudges do. Per-pad fx are the member track's own chain by design
;; (docs/drum-rack-v2-spec.md, "UI"), so all a pad selection has to do is name
;; the member the panel offers to open.
(defstate pad-selected-group -1)
(defstate pad-selected-note -1)

(def select-pad (gidx pad)
  (do
    (set! pad-selected-group (eseq.drum-rack-v2/group-id gidx))
    (set! pad-selected-note (get pad :pad-note))))

(def pad-selected? (gidx pad)
  (and (not (= pad nil))
    (= pad-selected-group (eseq.drum-rack-v2/group-id gidx))
    (= pad-selected-note (get pad :pad-note))))

;; The focused pad of a rack, or nil when the focus names another rack (or no
;; pad answers to the focused note any more).
(def selected-pad (gidx)
  (if (= pad-selected-group (eseq.drum-rack-v2/group-id gidx))
    (reduce |acc pad| (if (= (get pad :pad-note) pad-selected-note) pad acc)
      nil
      (eseq.drum-rack-v2/pads gidx))
    nil))

;; Lazy pads (docs/drum-rack-v2-spec.md, "Track budget"): dropping a sound on an
;; EMPTY cell is what makes that pad claim a track. The new member is mapped to
;; exactly this cell's pad note, so it is playable by pad click and armed key
;; the moment it lands.
(def drop-on-empty-pad (event gidx cell)
  (let ((payload (get event :payload))
      (group-id (eseq.drum-rack-v2/group-id gidx))
      (note (pad-cell-note gidx cell)))
    (let ((path (get payload :path))
        (name (get payload :name)))
      (if (= (get event :drag-type) "instrument")
        ;; A pad needs a member track in this rack's group on a specific pad
        ;; note; the builtin add-track host commands take neither, so builtins
        ;; are refused instead of landing as a loose track (eseq-mj8).
        (if (= (get payload :kind) "builtin-instrument")
          (status "Drop a sample or saved instrument onto a pad")
          (if name
            (do
              (set! sbrowser-loading-instrument-name name)
              (host-command "add-track-instrument"
                (dict :name name :group-id group-id :pad-note note)))
            (status "Drop an instrument, not a folder")))
        (if path
          (host-command "add-track-sample"
            (dict :path path :group-id group-id :pad-note note
              :preserve-browser-context true))
          (status "Drop a sample file, not a folder"))))))

;; An OCCUPIED cell replaces the pad's sound on its existing member track — the
;; same replacement a drop on the member's grid row does — so pad identity,
;; pattern data, mixer settings and chokes all stay put.
(def pad-cell-track (pad)
  (if (= pad nil) -1 (get pad :track)))

(def pad-cell-drop-types (pad)
  (let ((track (pad-cell-track pad)))
    (if (and (>= track 0) (< track SEQ.num-tracks))
      (sound-drop-types track)
      (list "sample" "instrument"))))

(def pad-cell-drop-meta (gidx cell pad)
  (let ((track (pad-cell-track pad)))
    (if (and (>= track 0) (< track SEQ.num-tracks))
      (dict :kind "track" :track track :from-pad true)
      (dict :kind "rack-pad"
        :group-id (eseq.drum-rack-v2/group-id gidx)
        :cell cell
        :pad-note (pad-cell-note gidx cell)))))

(def drop-on-pad-cell (event gidx cell)
  (let ((track (pad-cell-track (pad-at gidx cell))))
    (if (and (>= track 0) (< track SEQ.num-tracks))
      (drop-on-track event)
      (drop-on-empty-pad event gidx cell))))

;; A pad's own fx ARE its member track's chain (docs/drum-rack-v2-spec.md,
;; "UI"), so "open this pad" means: make its member the track under edit. That
;; drops the bus selection, which is exactly what swaps the *fx* buffer from
;; the rack panel to that member's instrument and effects — in place, without
;; touching the workspace layout.
(def open-pad-member-fx (gidx pad)
  (let ((track (pad-cell-track pad)))
    (if (and (>= track 0) (< track SEQ.num-tracks))
      (select-track-for-edit track)
      nil)))

;; Pad trigger light (eseq-4b5.16): the host publishes one flag per rack member
;; track, lit for as long as that track is sounding whatever fired it — a hit on
;; this cell, an armed rack's keys, or its own sequenced steps. It rides the
;; box's BOUND `selected` state rather than a lisp-computed colour so a hit
;; repaints the cell without re-rendering the grid, the way a mixer meter does;
;; the persistent pad focus keeps its own border, computed below.
(def pad-trigger-binding (pad)
  (let ((track (pad-cell-track pad)))
    (if (>= track 0) (bind-seq (str "rack-pad-trigger-" track)) nil)))

(def pad-cell (gidx cell)
  (let ((pad (pad-at gidx cell)))
    (box
      :key (str "rack-pad-cell-" (eseq.drum-rack-v2/group-id gidx) "-" cell)
      :width 6.4 :height 2.3 :padding 0.15
      :background-color (if (= pad nil)
        :bg
        :mixer-strip-bg
        )
      :selected (pad-trigger-binding pad)
      :selected-background-color :accent 
      :border-width 1
      :border-color (if (pad-selected? gidx pad)
        :mixer-strip-selected-border
        '(rgba 0.30 0.31 0.32 1.0))
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-hover-background-color :mixer-control-bg
      :corner-radius 8
      :drop-types (pad-cell-drop-types pad)
      :drop-meta (pad-cell-drop-meta gidx cell pad)
      :on-drop (lambda (event) (drop-on-pad-cell event gidx cell))
      ;; A hit both plays the pad and focuses it: auditioning IS how you pick
      ;; the pad you then want to open, so the two must not need two gestures.
      :on-click |x y r| (if (= pad nil)
        nil
        (do
          (select-pad gidx pad)
          (eseq.drum-rack-v2/trigger-pad gidx pad)))
      (v-stack :width :fill :height :fill :gap 0.05
        ;; The note is the cell's own, so an empty cell still says which note
        ;; a drop here would claim.
        (label (if (= pad nil)
            (eseq.drum-rack-v2/note-label (pad-cell-note gidx cell))
            (get pad :label))
          :font-size 8 :color (if (= pad nil) '(rgba 0.42 0.44 0.46 1.0) :dim)
          :active (pad-trigger-binding pad)
          :active-color :black
          :bg :transparent
          :width :fill :text-align :center)
        (label (if (= pad nil) "" (substring (pad-cell-name pad) 0 12))
          :active (pad-trigger-binding pad)
          :active-color :black
          :font-size 6.8 :color :white :bg :transparent
          :width :fill :text-align :center)
        (label (if (= pad nil)
            ""
            (if (< (get pad :choke) 0) "" (str "choke " (get pad :choke))))
          :font-size 6.2 :color :dim :bg :transparent
          :width :fill :text-align :center)))))

;; Row 0 renders at the TOP and carries the page's HIGHEST four notes: the
;; grid reads bottom-up, so the bottom-left cell is the page's C.
(def pad-grid-row (gidx row)
  (h-stack :gap 0.15 :align :center
    (each (range 0 4) |col|
      (pad-cell gidx (+ (* row 4) col)))))

;; Paging walks the note range, not the pad list: every octave is reachable so
;; a new pad can be placed anywhere, and the readout names the notes on screen
;; rather than a meaningless "1/2".
(def pad-grid-page-selector (gidx)
  (h-stack :gap 0.15 :align :center
    (button "◂"
      :key (str "rack-pad-page-prev-" (eseq.drum-rack-v2/group-id gidx))
      :width 1.4 :height 0.9 :padding 0 :font-size 8
      :background-color '(rgba 0.1 0.1 0.1 1.0)
      :border-color :transparent :color :dim
      :on-click |x y r| (set-pad-page gidx (- (pad-page gidx) 1)))
    (label (eseq.drum-rack-v2/pad-page-label (pad-page gidx))
      :key (str "rack-pad-page-label-" (eseq.drum-rack-v2/group-id gidx))
      :font-size 8 :color :dim :bg :transparent)
    (button "▸"
      :key (str "rack-pad-page-next-" (eseq.drum-rack-v2/group-id gidx))
      :width 1.4 :height 0.9 :padding 0 :font-size 8
      :background-color '(rgba 0.1 0.1 0.1 1.0)
      :border-color :transparent :color :dim
      :on-click |x y r| (set-pad-page gidx (+ (pad-page gidx) 1)))))

(def pad-grid-intrinsic-width 26.45)

;; The pad grid as a component: the *fx* rack panel draws it at its intrinsic
;; width beside the rack's own controls. It lives here, next to the pad cell it
;; is made of, so pad badges, drops and audition stay one definition wherever a
;; future surface draws them.
(def rack-pad-grid (gidx)
  (box :key (str "rack-pad-grid-" (eseq.drum-rack-v2/group-id gidx))
    :width pad-grid-intrinsic-width :padding 0.2
    :background-color :bg
    :corner-radius 14
    (v-stack :gap 0.1 :align :start
      (each (range 0 4) |row|
        (pad-grid-row gidx row)))))

;; ── Octave overview mini-map (eseq-4b5.15) ──────────────────────────────
;; A slim full-range map to the LEFT of the pad grid: every note the grid can
;; address as a tiny cell, four to a row, lowest at the bottom, with the
;; sixteen notes currently enlarged drawn as a highlighted block. Occupied
;; notes are filled, so a kit that lives three octaves up is visible without
;; paging there — and a click on any row pages the grid to it, which is the
;; affordance the arrows can only offer one octave at a time.
;;
;; It is a second VIEW of pad-grid-page, not a second page state: the
;; highlight and the click both go through the same page functions the grid
;; uses.
;;
;; The pad map is read ONCE at the root and flattened into a note-indexed
;; track list there. A per-cell read of SEQ.groups would re-render all 88
;; cells on any pad edit — whole-list reads are the expensive kind of
;; reactivity here — and a per-cell scan of the pads would walk the map 88
;; times over to draw it once.

(def pad-map-cell-width 0.75)
(def pad-map-cell-height 0.34)

;; Where a note sits in that flattened list. Pad notes are transposes around C4
;; and so go negative, which no list index does: the map is indexed from the
;; lowest note the grid can name.
(def pad-map-slot (note)
  (- note (eseq.drum-rack-v2/min-grid-pad-note)))

;; Slot -> the member track behind that note, -1 where no pad answers. The map
;; needs the track, not just "occupied", because a cell's trigger light is that
;; track's (eseq-4b5.16) — and "occupied" is just `track >= 0`, so one pass over
;; the pads still answers both questions.
(def pad-note-tracks (pads)
  (reduce |acc pad|
    (let ((note (get pad :pad-note)))
      (if (and (>= note (eseq.drum-rack-v2/min-grid-pad-note))
          (<= note (eseq.drum-rack-v2/max-grid-pad-note)))
        (set-nth acc (pad-map-slot note) (get pad :track))
        acc))
    (map (lambda (n) -1)
      (range 0 (+ (pad-map-slot (eseq.drum-rack-v2/max-grid-pad-note)) 1)))
    pads))

(def note-track (tracks note)
  (nth tracks (pad-map-slot note)))

(def note-occupied? (tracks note)
  (>= (note-track tracks note) 0))

;; The map lights on the same per-track binding the enlarged grid does, which
;; is what makes a hit on a pad the grid is NOT showing still visible here.
(def pad-map-cell (gid tracks note)
  (let ((track (note-track tracks note)))
    (box :key (str "rack-pad-map-cell-" gid "-" note)
      :width pad-map-cell-width :height pad-map-cell-height
      :background-color (if (>= track 0)
        '(rgba 0.60 0.72 0.75 1.0)
        '(rgba 0.19 0.20 0.21 1.0))
      :selected (if (>= track 0) (bind-seq (str "rack-pad-trigger-" track)) nil)
      :selected-background-color '(rgba 0.95 0.98 1.0 1.0)
      :corner-radius 2)))

;; A row is the click target, not its cells: four notes is already a finer jump
;; than the octave-aligned pages the click snaps to, and one handler per row
;; keeps the map cheap.
(def pad-map-row (gidx gid tracks row page)
  (let ((base (eseq.drum-rack-v2/pad-map-row-base row))
      (on-page (eseq.drum-rack-v2/pad-map-row-on-page? row page)))
    (box :key (str "rack-pad-map-row-" gid "-" base)
      :background-color (if on-page
        '(rgba 0.24 0.32 0.34 1.0)
        :transparent)
      :border-width 1
      :border-color (if on-page :mixer-strip-selected-border :transparent)
      :corner-radius 2
      :on-click |x y r| (set-pad-page gidx (eseq.drum-rack-v2/page-of-note base))
      (h-stack :gap 0.08 :align :center
        (each (range 0 4) |col|
          (pad-map-cell gid tracks (+ base col)))))))

(def pad-map-intrinsic-width 3.6)

(def rack-pad-map (gidx)
  (let ((gid (eseq.drum-rack-v2/group-id gidx))
      (tracks (pad-note-tracks (eseq.drum-rack-v2/pads gidx)))
      (page (pad-page gidx)))
    (box :debug-name "rack-pad-map"
      :key (str "rack-pad-map-" gid)
      :width pad-map-intrinsic-width :height :fill :padding 0.15
      :background-color :bg
      :corner-radius 8
      :v-align :center :h-align :center
      (v-stack :gap 0.05 :align :center
        (each (range 0 (eseq.drum-rack-v2/pad-map-row-count)) |row|
          (pad-map-row gidx gid tracks row page))))))

(def group-header-body (gidx)
  (let ((c (eseq.drum-rack-v2/color gidx))
      (bus-idx (eseq.drum-rack-v2/bus-index gidx))
      (rack (eseq.drum-rack-v2/rack? gidx))
      (armed (eseq.drum-rack-v2/armed? gidx))
      (muted (and (>= bus-idx 0) (nth SEQ.bus-mutes bus-idx)))
      (soloed (and (>= bus-idx 0) (nth SEQ.bus-solos bus-idx))))
    (box :background "seqv-track-container"
      :padding 0.1
      :on-click |x y r| (select-group gidx)
      (h-stack :gap 0.4 :align :center
        (box
          :key (group-element-key gidx "color-badge")
          :width 0.68 :height 2.0
          :background "seqv-track-color-badge"
          :track-r (nth c 0)
          :track-g (nth c 1)
          :track-b (nth c 2)
          :on-click |x y r| (select-group gidx))
        (disclosure-button
          :key (group-element-key gidx "collapse")
          :width 1.55 :height 1.4
          :collapsed (eseq.drum-rack-v2/collapsed? gidx)
          :surface-alpha 1.0
          :focusable true
          :on-click |x y r| (eseq.drum-rack-v2/toggle-collapsed gidx))
        ;; Arm = drum-rack pad-play mode. A regular group is not an input
        ;; target and therefore contributes no Arm control or placeholder.
        (if rack
          (box :width 2 :height 1.5
            :background "seqv-rec-arm-dot"
            :key (group-element-key gidx "arm")
            :active (if armed 1 0)
            :on-click |x y r| (do
              (select-group gidx)
              (eseq.drum-rack-v2/toggle-armed gidx)))
          (box :width 2.0 :height 0.0 :bg :transparent))
        (button "M"
          :key (group-element-key gidx "mute")
          :width 1.55 :height 1.2 :padding 0 :font-size 10
          :border-color :transparent
          :background-color (mute-bg muted)
          :color (if muted :gray :black)
          :on-click |x y r| (if (>= bus-idx 0)
            (do (select-group gidx) (seq-toggle-bus-mute bus-idx))
            nil))
        (button "S"
          :key (group-element-key gidx "solo")
          :width 1.55 :height 1.2 :padding 0 :font-size 10
          :background-color (solo-bg soloed)
          :border-color :transparent
          :color (if soloed :white :gray)
          :on-click |x y r| (if (>= bus-idx 0)
            (do (select-group gidx) (seq-toggle-bus-solo bus-idx))
            nil))
        (box :width 8.6 :height 1
          :key (group-element-key gidx "select")
          :background-color :transparent
          :on-click |x y r| (select-group gidx)
          (badge (track-name-display (eseq.drum-rack-v2/group-name gidx))
            :key (group-element-key gidx "name-label")
            :icon (eseq.track-collapse/group-type-icon (nth SEQ.groups gidx))
            :font-size 11 :width 8.6 :height 1 :padding 0
            :h-align :left
            :background-color :transparent
            :border-color :transparent
            :highlight-color :transparent
            :shadow-color :transparent
            :color (if muted (rgba 0.4 0.4 0.4 0.6) :dim)
            :bg :transparent))
        ;; No PADS/KIT buttons here: selecting the rack puts both the pad grid
        ;; and SAVE KIT in the *fx* buffer's rack panel (ui/effects/buffers.lisp,
        ;; docs/drum-rack-v2-spec.md, "UI"), so the header keeps the same
        ;; name/meter shape an ordinary track header has.
        (group-volume-control gidx bus-idx)))))

(def group-header-row (gidx)
  (subtree :key (str "seqv-" (group-ui-kind gidx) "-header-" (eseq.drum-rack-v2/group-id gidx))
    (group-header-body gidx)))

(def group-block (gidx)
  (let ((c (eseq.drum-rack-v2/color gidx)))
    (box :width :fill
      :key (group-element-key gidx "block")
      :selected (group-selected-binding gidx)
      :background-color (group-container-bg c)
      :selected-background-color (group-container-bg c)
      :border-width 2
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :corner-radius 10
      :padding 0.345
      ;; Hit testing chooses the deepest clickable widget, so member-track
      ;; clicks keep selecting the track; only exposed container chrome reaches
      ;; this handler and selects the group's backing bus for the FX panel.
      :on-click |x y r| (select-group gidx)
      (v-stack :width :fill :gap 0.1
        (group-header-row gidx)
        (if (eseq.drum-rack-v2/collapsed? gidx)
          (box :width 0.0 :height 0.0 :bg :transparent)
          (v-stack :width :fill :gap 0.0
            (each (eseq.drum-rack-v2/visible-members gidx) |m|
              (subtree :key (str "sequencer-track-" (nth SEQ.track-ids m))
                (group-member-row gidx m)))
            (each (eseq.drum-rack-v2/child-racks gidx) |child|
              (subtree :key (str "sequencer-rack-" (eseq.drum-rack-v2/group-id child))
                (group-block child)))))))))

(def grid-render-item (item)
  (if (= (get item :kind) "group")
    (let ((gidx (get item :gidx)))
      (subtree :key (str "sequencer-" (group-ui-kind gidx) "-" (eseq.drum-rack-v2/group-id gidx))
        (group-block gidx)))
    (let ((i (get item :track)))
      (subtree :key (str "sequencer-track-" (nth SEQ.track-ids i))
        (track-row i true)))))

(effect-buffer "*sequencer*"
  (v-stack :width :fill :fill-content-style true :padding 0.00 :gap 0.0
    (each (eseq.drum-rack-v2/grid-render-items) |item|
      (grid-render-item item))

     (box :key "new-track-drop-zone"
      :width :fill :height 2.4 :flex 1
      :background-color :transparent
      :drop-hover-background-color :mixer-control-bg
      :border-width 1
      :border-color :transparent
      :drop-hover-border-color :mixer-strip-selected-border
      :corner-radius 10
      :drop-types (list "sample" "instrument" "sound")
      :drop-meta (dict :kind "new-sample-track")
      :on-drop (lambda (event) (drop-new-track event))
      (label ""
        :font-size 1
        :color :transparent
        :bg :transparent))))


(set-buffer-mode-for "*sequencer*" "eseq.seq-grid-mode/seq-grid-mode")

(cursor-step-changed SEQ.current-track (eseq.seq-core-state/cursor-step-value))
