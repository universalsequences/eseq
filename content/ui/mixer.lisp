;; ui/mixer.lisp — Horizontal DAW-style mixer.
;; Renders to *mixer* buffer. Loaded by ui/main.lisp.

(module eseq.mixer)
;; Compile-time edge (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile.
(import eseq.seq-core-state)

;; Migration aliases (module spec §10 step 2) for the names unconverted
;; callers still spell flat. Four are lisp-side (effects/track-panels.lisp
;; paints the track-panel header with the mixer's colour/name helpers);
;; thirteen are `mixer-v2-*` entry points driven by name from Rust
;; state_values tests; `seq-ctrl-g` is the global Ctrl+G / Cmd+G dispatcher
;; that src/ui/input.rs evals by name.  Deleted as each consumer converts.

(import eseq.track-collapse)
(import eseq.drum-rack-v2)
;; Read-only: the step-tab registry names the `(load …)` a script came from.
(import eseq.seq-step-tabs)
(import eseq.effects.drag-drop :as effect-dd)
(import eseq.effects.param-controls :as pc)
;; Shared scene-bank view state: the clip grid below shows only the bank the
;; transport strip is viewing (scene-banks spec 10.1).
(import eseq.scene-banks)

(export render-order
        bus-index-by-id
        display-bus-index
        group-bus-id?
        track-color-r
        track-color-g
        track-color-b
        select-track
        select-track-delete-target
        drop-sample-on-track
        drop-sample-new-track
        drop-on-track
        drop-effect-on-bus
        track-pattern-cells
        launch-track-pattern
        track-collapsed-label
        select-prev-channel
        select-next-channel
        delete-selected-track
        handle-key
        drop-on-group-header
        patch-mixer-strip
        seq-ctrl-g
        clip-area-height
        strip-height
        grouped-strip-height
        collapsed-strip-height
        bus-strip-height
        group-bus-strip-height
        drop-zone-height
        clip-growth-spacer
        viewed-bank-track-pattern-cells
        track-pattern-grid
        mixer-body
        track-meter-control
        bus-meter-control
        bus-output-dropdown
        bus-mute-label
        track-volume-field
        select-bus
        activate-track-control
        clear-delete-target
        display-bus-index
        has-mix-bus?
        track-context-menu
        bus-meter
        track-meter
        pointer-volume
        mod-port-row
        bus-mod-port-row)

;; Strip heights derive from the shared clip-area knob
;; (eseq.seq-core-state/mixer-clip-area-height) so a package can grow the
;; whole mixer by one `setopt`. Each is a function, so a package can also
;; `override` one strip kind independently. The constants are the stock
;; 13.8-cell strip minus its 4.0-cell clip area, and the historical offsets
;; of the other strip kinds from it.
(def clip-area-height ()
  (eseq.seq-core-state/effective-clip-area-height))

;; Compact mode (eseq.seq-core-state/mixer-show-clip-grid off): no clip grid,
;; and the bus/group strips drop the spacers that kept them level with the
;; taller track strips.
(def compact? ()
  (not eseq.seq-core-state/mixer-show-clip-grid))

;; Strip widths (cells), customize-tier knobs.
(defcustom track-strip-width 12.9
  :type :number :min 11 :max 24 :step 0.1
  :doc "Width (cells) of a track strip in the mixer.")

(defcustom bus-strip-width 10.3
  :type :number :min 9 :max 20 :step 0.1
  :doc "Width (cells) of a bus strip in the mixer.")

(def strip-height ()
  (+ 9.8 (clip-area-height)))

;; Grouped strips drop the output dropdown, so they are shorter.
(def grouped-strip-height ()
  (- (strip-height) 1.3))

(def collapsed-strip-height ()
  (- (strip-height) 1.65))

(def bus-strip-height ()
  (strip-height))

(def group-bus-strip-height ()
  (- (strip-height) 0.05))

(def drop-zone-height ()
  (strip-height))

;; Bus and group strips have no clip area, so they absorb the clip-area
;; growth with a spacer above their meter row; that keeps their meters,
;; buttons and labels level with the track strips'. Nothing is inserted at
;; the stock height so the factory layout is unchanged.
(def clip-growth-spacer ()
  (let ((extra (- (clip-area-height) 4.0)))
    (if (> extra 0)
      (box :width :fill :height extra :bg :transparent)
      nil)))

(defstate track-menu-open false)
(defstate track-menu-col 0)
(defstate track-menu-row 0)
(defstate track-menu-track -1)
(defstate track-menu-group-id -1)
(defstate track-renaming -1)
(defstate track-rename-draft "")
(defstate group-renaming -1)
(defstate group-rename-draft "")

;; Data, rather than menu-specific control flow, is the extension seam for
;; Duplicate/Delete/Group/color actions added later.
(def track-menu-actions
  (list (dict :id :rename :label "Rename")))

(def group-menu-actions
  (list
    (dict :id :rename :label "Rename")
    (dict :id :convert-drum-rack :label "Convert to Drum Rack")
    (dict :id :ungroup :label "Ungroup")))

;; A rack's menu also offers the graph sequencers it could own. A sequencer
;; that merely routes to tracks inside the rack is still project-owned;
;; "Attach … to rack" hands it to the rack (routes become rack members, and
;; the pair travels together into kits), and "Detach …" gives it back
;; (docs/rack-clips-and-break-kits-spec.md §5.1).
(def rack-group-menu-actions (gid)
  (append
    (append
      (list
        (dict :id :rename :label "Rename"))
      ;; Rack clips (docs/rack-clips-and-break-kits-spec.md §6.3). A LEGACY
      ;; rack (no bank) is offered the one-shot conversion; a rack that already
      ;; has clips saves a new one here and deletes from the row's [-] button.
      (if (eseq.drum-rack-v2/has-clips? gid)
        (list (dict :id :save-rack-clip :label "Save clip as..."))
        (list (dict :id :convert-to-clips :label "Convert to clips"))))
    (map
      (lambda (graph)
        (if (= (get graph :owner-rack) gid)
          (dict :id :detach-sequencer
                :sequencer-id (get graph :id)
                :sequencer-name (get graph :name)
                :label (str "Detach " (get graph :name)))
          (dict :id :move-sequencer-into-rack
                :sequencer-id (get graph :id)
                :sequencer-name (get graph :name)
                :label (str "Attach " (get graph :name) " to rack"))))
      (filter (lambda (graph)
                (or (= (get graph :owner-rack) nil)
                    (= (get graph :owner-rack) gid)))
        (or SEQ.graph-sequencers (list))))
    (list
      ;; Break kits (docs/rack-clips-and-break-kits-spec.md 7.2): the kit save
      ;; panel, which carries a scene checklist for the clip bank.
      (dict :id :export-kit :label "Export as kit...")
      (dict :id :ungroup :label "Ungroup"))))

;; The path a project-owned script was loaded from, per the step-tab registry;
;; "" when unknown (the moved instance then does not come back by itself on
;; project open). The host turns it into `(import …)` for package modules or
;; `(load …)` for plain files.
(def sequencer-source-path (name)
  (let ((hits (filter
                (lambda (tab)
                  (= (eseq.seq-step-tabs/seq-step-tab-sequencer-name tab) name))
                eseq.seq-step-tabs/seq-registered-step-tabs)))
    (if (> (len hits) 0)
      (eseq.seq-step-tabs/seq-step-tab-source-path (nth hits 0))
      "")))

(def track-peak (i)
  (bind-seq (str "track-peak-" i)))

(def bus-peak-l (i)
  (if (= (nth SEQ.bus-names i) "Mix")
    (bind-seq "master-peak-l")
    (bind-seq (str "bus-peak-" i))))

(def bus-peak-r (i)
  (if (= (nth SEQ.bus-names i) "Mix")
    (bind-seq "master-peak-r")
    (bind-seq (str "bus-peak-" i))))

(def track-selected-binding (i)
  (bind-seq (str "track-selected-" i)))

(def track-delete-target-binding (i)
  (bind-seq (str "mixer-track-delete-target-" i)))

;; Group targets are ID-stable and sparse, so derive their selected state from
;; the shared target plus its reactive version instead of registering one field
;; per current group.
(def group-delete-target? (group-id)
  (do
    SEQ.delete-target-version
    (seq-delete-target? :mixer-group (dict :group-id group-id))))

(def track-pattern-cell-active-binding (track pattern-id)
  (bind-seq (str "track-pattern-cell-active-" track "-" pattern-id)))

(def track-pattern-cell-assigned-binding (track pattern-id)
  (bind-seq (str "track-pattern-cell-assigned-" track "-" pattern-id)))

(def track-pattern-cell-override-binding (track pattern-id)
  (bind-seq (str "track-pattern-cell-override-" track "-" pattern-id)))

(def track-pattern-cell-selected-binding (track pattern-id)
  (bind-seq (str "track-pattern-cell-selected-" track "-" pattern-id)))

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

(def strip-bg (selected muted)
  (if selected
    :mixer-strip-selected-bg
    (if muted
      :mixer-strip-muted-bg
      :mixer-strip-bg)))

(def strip-border (selected)
  (if selected
    :mixer-strip-selected-border
    :mixer-strip-border))

(def arm-bg (active)
  (if active
    (rgba 0.95 0.20 0.18 1.0)
    :mixer-control-bg))

(def pointer-volume (sy)
  (max 0.0 (min 1.0 (* 0.5 (- 1.0 sy)))))

(def event-volume (event)
  (pointer-volume (get event :sy)))

;; Stable endpoint for Finder-style range extension. Cmd-click deliberately
;; leaves it untouched; a plain selection starts a new range.
(def track-selection-anchor nil)

(def select-track (i)
  (do
    (set! track-selection-anchor i)
    (set! eseq.seq-core-state/selected-bus -1)
    (clear-delete-target)
    (seq-set-track i)
    (host-command "reveal-sequencer-track" (dict :track i))))

(def select-track-delete-target (i)
  (do
    (set! track-selection-anchor i)
    (set! eseq.seq-core-state/selected-bus -1)
    (seq-set-track i)
    (host-command "reveal-sequencer-track" (dict :track i))
    (seq-set-delete-target :mixer-track (dict :track i))))

(def activate-track-control (i)
  (do
    (set! eseq.seq-core-state/selected-bus -1)
    (clear-delete-target)))

;; The platform selection gesture toggles membership: Command-click on macOS,
;; Alt-click on Linux. Rust supplies the semantic field so this behavior does
;; not depend on window-manager ownership of Super.
(def multi-select-click? (event)
  (get event :additive-selection))

(def toggle-track-select (i)
  (do
    (set! eseq.seq-core-state/selected-bus -1)
    (seq-toggle-track-selected i)
    (host-command "reveal-sequencer-track" (dict :track i))))

(def visual-track-range (anchor target)
  (eseq.seq-core-state/track-range-in-order
    (eseq.drum-rack-v2/mixer-visible-track-order) anchor target))

(def range-track-select (i)
  (let ((anchor (if (= track-selection-anchor nil)
                  SEQ.current-track
                  track-selection-anchor)))
    (do
      (set! track-selection-anchor anchor)
      (set! eseq.seq-core-state/selected-bus -1)
      (seq-select-tracks (visual-track-range anchor i) i)
      (host-command "reveal-sequencer-track" (dict :track i)))))

;; Plain click = single-select; shift-click = replace with the anchored range;
;; cmd-click = toggle membership without moving the range anchor.
(def track-body-click (event i)
  (if (get event :shift)
    (range-track-select i)
    (if (multi-select-click? event)
      (toggle-track-select i)
      (select-track i))))

;; The label preserves its delete-target gesture on a plain click. Shift-click
;; clears that target just like cmd-click and selects the anchored range.
(def track-label-click (event i)
  (if (get event :shift)
    (range-track-select i)
    (if (multi-select-click? event)
      (toggle-track-select i)
      (select-track-delete-target i))))

(def select-bus (i)
  (do
    (seq-clear-selection)
    (seq-clear-delete-target)
    (set! eseq.seq-core-state/selected-bus i)))

(def drop-sample-on-track (event)
  (let ((payload (get event :payload))
      (target (get event :target)))
    (let ((path (get payload :path))
        (track (get target :track)))
      (do
        (clear-delete-target)
        (if path
          (eseq.browser/drop-sample-on-track event)
          (status "Drop a sample file, not a folder"))))))

(def drop-sample-new-track (event)
  (if (= (get event :drag-type) "track-badge")
    (drop-track-out-of-group event)
    (let ((payload (get event :payload)))
      (let ((path (get payload :path))
            (name (get payload :name)))
        (do
          (clear-delete-target)
          (if (= (get event :drag-type) "sound")
            (if path
              (host-command "add-track-from-sound" (dict :path path))
              (status "Drop a Sound item, not a folder"))
            (if (= (get event :drag-type) "instrument")
              (eseq.browser/drop-instrument-new-track payload)
              (if path
                (host-command "add-track-sample" (dict :path path :preserve-browser-context true))
                (status "Drop a sample file, not a folder")))))))))

;; Drag a track badge onto a group container -> add it to that group.
(def drop-track-into-group (event gidx)
  (let ((trk (get (get event :payload) :track)))
    (do
      (clear-delete-target)
      (if (>= trk 0)
        (host-command "move-track-to-group" (dict :track trk :gidx gidx))
        false))))

;; Drag a track badge onto the "Drop samples here" zone -> remove it from its group.
(def drop-track-out-of-group (event)
  (let ((trk (get (get event :payload) :track)))
    (do
      (clear-delete-target)
      (if (>= trk 0)
        (host-command "remove-track-from-group" (dict :track trk))
        false))))

(def drop-effect-on-track (event)
  (let ((payload (get event :payload))
      (target (get event :target)))
    (let ((kind (get payload :kind))
        (name (get payload :name))
        (track (get target :track)))
      (do
        (select-track track)
        (if (= kind "builtin-audio-effect")
          (host-command "add-builtin-effect-to-track" (dict :track track :name name))
          (if (= kind "custom-audio-effect")
            (host-command "add-effect-to-track" (dict :track track :name name))
            (if (= kind "midi-effect")
              (host-command "add-midi-fx-to-track" (dict :track track :name name))
              (status "Drop an audio or MIDI effect"))))))))

(def drop-on-track (event)
  (let ((drag-type (get event :drag-type)))
    (if (= drag-type "sound")
      (eseq.browser/drop-sound-on-track event)
      (if (= drag-type "instrument")
        (eseq.browser/drop-instrument-on-track event)
        (if (= drag-type "sample")
          (drop-sample-on-track event)
          (if (= drag-type "effect-instance")
            (let ((target (get event :target)))
              (effect-dd/drop-existing-effect
                (get event :payload)
                (dict :chain "append" :track (get target :track) :bus -1 :slot -1)))
            (if (or (= drag-type "audio-effect") (= drag-type "midi-effect"))
              (drop-effect-on-track event)
              (status "Unsupported drop"))))))))

(def track-drop-types (i)
  (if (eseq.track-collapse/replaceable-instrument? i)
    (list "sample" "instrument" "sound" "audio-effect" "midi-effect" "effect-instance")
    (if (eseq.track-collapse/sound-replaceable? i)
      (list "sample" "sound" "audio-effect" "midi-effect" "effect-instance")
      (list "sample" "audio-effect" "midi-effect" "effect-instance"))))

(def drop-effect-on-bus (event)
  (let ((payload (get event :payload))
      (target (get event :target)))
    (let ((kind (get payload :kind))
        (name (get payload :name))
        (bus (get target :bus)))
      (do
        (select-bus bus)
        (if (= kind "builtin-audio-effect")
          (host-command "add-builtin-bus-effect" (dict :bus bus :name name))
          (if (= kind "custom-audio-effect")
            (host-command "add-bus-effect" (dict :bus bus :name name))
            (status "Drop an audio effect on a bus")))))))

(defstate pending-mod-source -1)
(def track-modulator? (i)
  (and (< i (len SEQ.track-instrument-types))
    (= (nth SEQ.track-instrument-types i) "modulator")))

(def track-mod-output? (i)
  (or (track-modulator? i)
    (and (< i (len SEQ.track-mod-output-available))
      (nth SEQ.track-mod-output-available i))))

(def clear-delete-target ()
  (seq-clear-delete-target))

(def mod-route-dest-kind (route)
  (let ((kind (get route :dest-kind)))
    (if kind kind "track")))

(def mod-route-exists-at (source dest-kind dest input idx)
  (if (>= idx (len SEQ.mod-routes))
    false
    (let ((route (nth SEQ.mod-routes idx)))
      (or (and (= (get route :source) source)
            (= (mod-route-dest-kind route) dest-kind)
            (= (get route :dest) dest)
            (= (get route :input) input))
        (mod-route-exists-at source dest-kind dest input (+ idx 1))))))

(def mod-route-exists? (source dest input)
  (mod-route-exists-at source "track" dest input 0))

(def bus-mod-route-exists? (source bus-id input)
  (mod-route-exists-at source "bus" bus-id input 0))

(def mod-route-sources-at (dest-kind dest input idx acc)
  (if (>= idx (len SEQ.mod-routes))
    acc
    (let ((route (nth SEQ.mod-routes idx)))
      (mod-route-sources-at dest-kind dest input (+ idx 1)
        (if (and (= (mod-route-dest-kind route) dest-kind)
              (= (get route :dest) dest)
              (= (get route :input) input))
          (append acc (list (get route :source)))
          acc)))))

(def mod-route-sources (dest input)
  (mod-route-sources-at "track" dest input 0 (list)))

;; True when any mod route leaves `source`'s OUT port; only a connected OUT
;; port lights with its signal.
(def mod-route-from-at (source idx)
  (if (>= idx (len SEQ.mod-routes))
    false
    (or (= (get (nth SEQ.mod-routes idx) :source) source)
      (mod-route-from-at source (+ idx 1)))))

(def track-mod-out-connected? (track)
  (mod-route-from-at track 0))

;; Live port levels published by the host at meter rate (see
;; read_mod_port_levels): bound by reference so a level change repaints the
;; port without re-running the strip.
(def mod-out-level (track)
  (if (track-mod-out-connected? track)
    (bind-seq (str "mod-out-level-" track))
    0))

(def mod-in-level (track input)
  (bind-seq (str "mod-in-level-" track "-" input)))

(def bus-mod-in-level (bus-id input)
  (bind-seq (str "bus-mod-in-level-" bus-id "-" input)))

(def bus-mod-route-sources (bus-id input)
  (mod-route-sources-at "bus" bus-id input 0 (list)))

(def clear-selected-mod-route ()
  (clear-delete-target))

(def select-mod-route-kind (source dest-kind dest input)
  (do
    (seq-set-delete-target :mod-route (dict :source source :dest-kind dest-kind :dest dest :input input))
    (status (if (= dest-kind "bus")
      (str "Selected mod route: track " (+ source 1) " out -> group Ext" (+ input 1))
      (str "Selected mod route: track " (+ source 1) " out -> track " (+ dest 1) " Ext" (+ input 1))))))

(def select-mod-route (source dest input)
  (select-mod-route-kind source "track" dest input))

(def selected-mod-sources-at (dest-kind dest input idx acc)
  (if (>= idx (len SEQ.selected-mod-routes))
    acc
    (let ((route (nth SEQ.selected-mod-routes idx)))
      (selected-mod-sources-at dest-kind dest input (+ idx 1)
        (if (and (= (mod-route-dest-kind route) dest-kind)
              (= (get route :dest) dest)
              (= (get route :input) input))
          (append acc (list (get route :source)))
          acc)))))

(def selected-mod-sources (dest input)
  (selected-mod-sources-at "track" dest input 0 (list)))

(def selected-bus-mod-sources (bus-id input)
  (selected-mod-sources-at "bus" bus-id input 0 (list)))

(def mod-out-click (track)
  (if (track-mod-output? track)
    (do
      (set! pending-mod-source track)
      (clear-delete-target)
      (status (str "Mod out: track " (+ track 1))))
    (do
      (clear-delete-target)
      (status "This track has no mod output"))))

(def cancel-mod-draw ()
  (do
    (set! pending-mod-source -1)
    true))

(def connect-mod-route (source track input)
  (do
    (clear-delete-target)
    (if (= source track)
      (status "Mod self-routes are not allowed")
      (if (mod-route-exists? source track input)
        (status "Mod route already connected")
        (host-command "set-mod-route"
          (dict :source source :dest-kind "track" :dest track :input input))))))

(def connect-bus-mod-route (source bus-id input)
  (do
    (clear-delete-target)
    (if (bus-mod-route-exists? source bus-id input)
      (status "Mod route already connected")
      (host-command "set-mod-route"
        (dict :source source :dest-kind "bus" :dest bus-id :input input)))))

(def mod-in-click (track input)
  (if (< pending-mod-source 0)
    false
    (do
      (connect-mod-route pending-mod-source track input)
      (set! pending-mod-source -1))))

(def bus-mod-in-click (bus-id input)
  (if (< pending-mod-source 0)
    false
    (do
      (connect-bus-mod-route pending-mod-source bus-id input)
      (set! pending-mod-source -1))))

(defwidget mixer-v2-mod-port
  :width 1.55 :height 1.55
  :paint-margin 0.12
  :state (active pending output selected level)
  :bindable (level)
  :shader
  ;; `level` is the live mod signal at this port (0..1, bound to the SEQ
  ;; mod-*-level fields at meter rate). It lights the port like a VCV Rack
  ;; jack LED: the dark centre fills with the ring colour and the ring itself
  ;; brightens a little.
  (let ((outer (if active
          (if selected
            :mod-port-selected
            (if output
              (if pending :mod-port-pending :mod-port-output)
              :mod-port-input))
          :mod-port-inactive))
      (inner (if active
          (if output :mod-port-output-inner :mod-port-input-inner)
          :mod-port-inactive-inner))
      (glow (* 0.9 (clamp level 0.0 1.0)))
      (ring-lift (* 0.3 glow)))
    (sdf/layer
      (sdf/fill (sdf/circle 0.82)
        (material :color (+ (* outer (rgba (- 1.0 ring-lift) (- 1.0 ring-lift) (- 1.0 ring-lift) 1.0))
                            (rgba ring-lift ring-lift ring-lift 0.0))))
      (sdf/fill (sdf/circle 0.43)
        (material :color (+ (* inner (rgba (- 1.0 glow) (- 1.0 glow) (- 1.0 glow) 1.0))
                            (* outer (rgba glow glow glow 0.0))))))))

(defwidget track-pattern-cell-bg
  :width 0.88 :height 0.38
  :paint-margin 0.04
  :state (active assigned override selected track-r track-g track-b)
  :bindable (active assigned override selected track-r track-g track-b)
  :shader
  (let ((track-col (rgba track-r track-g track-b 1.0))
      (track-r (if (= active 1) (* 1.10 track-r) track-r))
      (track-g (if (= active 1) (* 1.1 track-g) track-g))
      (track-b (if (= active 1) (* 1.1 track-b) track-b))
      (outer (if (= selected 1)
          (rgba 0.94 0.96 1.0 1.0)
          (rgba track-r track-g track-b 1.0)))
      (middle (rgba track-r track-g track-b 1.0))
      (inner (if (= active 0)
          (rgba 0.02 0.025 0.03 0.7)
          (rgba 0.1 0.1 0.1 0.3)
          )))
    ;; The play triangle moved into the sound-glyph shader (it must sit ON TOP
    ;; of the glyph; this background always draws underneath it). Liveness
    ;; comes from the host play-key store (sync_pattern_cell_glyph_frames),
    ;; and patch-less patterns still get an empty glyph frame published, so
    ;; every active cell renders the glyph-drawn triangle — no bg fallback.
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.3)
        (material :color outer))
      (sdf/fill (sdf/rounded-rect (* width 0.94) (* height 0.94) 0.2)
        (material :color middle))
      (sdf/fill (sdf/rounded-rect (* width (if (= active 1) 0.8 0.92))
          (* (if (= active 1) 0.8 0.92) height) (if (= active 1) 0.1 0.22))
        (material :color inner)))))

;; `track-pattern-cell-bg` for a cell whose quantized launch is pending:
;; identical geometry (the sound glyph on top is untouched), but the ring
;; blinks the track color toward white at the queued-scene-pill cadence
;; until the boundary launch fires and the host swaps the background back.
(defwidget track-pattern-cell-queued-bg
  :width 0.88 :height 0.38
  :paint-margin 0.04
  :state (active assigned override selected track-r track-g track-b)
  :bindable (active assigned override selected track-r track-g track-b)
  :animates true
  :shader
  (let ((pulse (+ 0.5 (* 0.5 (cos (* itime 5.4)))))
      (blink-r (+ track-r (* pulse (- 1.0 track-r))))
      (blink-g (+ track-g (* pulse (- 1.0 track-g))))
      (blink-b (+ track-b (* pulse (- 1.0 track-b))))
      (outer (rgba blink-r blink-g blink-b 1.0))
      (middle (rgba blink-r blink-g blink-b 1.0))
      (inner (if (= active 0)
          (rgba 0.02 0.025 0.03 0.7)
          (rgba 0.1 0.1 0.1 0.3))))
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.3)
        (material :color outer))
      (sdf/fill (sdf/rounded-rect (* width 0.94) (* height 0.94) 0.2)
        (material :color middle))
      (sdf/fill (sdf/rounded-rect (* width (if (= active 1) 0.8 0.92))
          (* (if (= active 1) 0.8 0.92) height) (if (= active 1) 0.1 0.22))
        (material :color inner)))))



(def track-pattern-cells (track)
  (if (< track (len SEQ.track-pattern-cells))
    (nth SEQ.track-pattern-cells track)
    (list)))

;; The pattern id this track has queued behind a quantized clip launch
;; (-1 = none), from the host's pending-launch poll.
(def queued-clip (track)
  (let ((queued (or SEQ.queued-track-clips (list))))
    (if (< track (len queued))
      (nth queued track)
      -1)))

(def launch-track-pattern (track cell)
  (do
    (activate-track-control track)
    (seq-set-track track)
    ;; Clip launches follow the transport's scene launch quantize: the host
    ;; assigns the cell now and defers the audible launch to the boundary.
    (host-command "set-scene-cell"
      (dict
        :scene (or SEQ.current-pattern 0)
        :track track
        :pattern-id (get cell :id)
        :quantize (or SEQ.scene-launch-quantize "off")))
    (seq-set-delete-target :track-pattern (dict :track track :pattern-id (get cell :id)))))

;; Clips of `track` that belong to the viewed scene bank (spec 10.1). A bank
;; holds at most 24 scenes, so at most 24 referenced clips per track — the
;; grid's 6x4 capacity — plus any orphan clips no scene references yet.
(def viewed-bank-track-pattern-cells (track)
  (filter (lambda (cell) (eseq.scene-banks/clip-in-viewed-bank? cell))
    (track-pattern-cells track)))

;; Clip launch cells scale with the strip width: the 6x4 grid was sized for
;; the stock 12.9-cell strip (six 2.0-cell columns), so a narrower strip
;; shrinks the cells instead of spilling them past the strip's edge.
(def clip-cell-scale ()
  (/ track-strip-width 12.9))

;; Character budget for a name that fit `base` characters at the stock strip
;; width; narrower strips truncate harder, wider ones show more.
(def name-chars (base)
  (max 3 (floor (* base (clip-cell-scale)))))

;; Bus strips have their own width knob; ~1.1 characters per cell at the
;; label's 10pt font.
(def bus-name-chars ()
  (max 3 (floor (* bus-strip-width 1.1))))

(def track-pattern-grid (track)
  (let ((cells (viewed-bank-track-pattern-cells track))
      (k (clip-cell-scale)))
    (box :width :fill :height (clip-area-height) :align :top :bg :black :background-color :buffer-bg
      (scroll
      (grid :cols 6 :col-width (* 2.0 k) :row-height (* 1.0 k) :align :center
        (each cells |cell cell-idx|
          (let ((pattern-id (get cell :id)))
            (box
              :key (str "track-pattern-cell-" track "-" pattern-id)
              :drag-type (str "track-pattern-" (nth SEQ.track-ids track))
              :drag-modifier :none
              :drag-payload (dict :track track :track-id (nth SEQ.track-ids track) :pattern-id pattern-id)
              :width (* 1.90 k) :height (* 0.95 k)
              :padding (* 0.35 k)
              :bg :transparent
              :background (if (= pattern-id (queued-clip track))
                "track-pattern-cell-queued-bg"
                "track-pattern-cell-bg")
              :active (track-pattern-cell-active-binding track pattern-id)
              :assigned (track-pattern-cell-assigned-binding track pattern-id)
              :override (track-pattern-cell-override-binding track pattern-id)
              :selected (track-pattern-cell-selected-binding track pattern-id)
              ;; Dimmed so the sound glyph on top carries the cell's identity;
              ;; the launch/selection states still read through the shader.
              :track-r (* 0.65 (track-color-r track false))
              :track-g (* 0.65 (track-color-g track false))
              :track-b (* 0.65 (track-color-b track false))
              :on-click (lambda (event) (launch-track-pattern track cell))
              ;; The pattern's bound sound, as its palette glyph (host feed:
              ;; sync_pattern_cell_glyph_frames). The tuned shader styling is
              ;; the widget default (TUNING_PROPS); the substrate body tints
              ;; with the track color so cells keep their track identity.
              (sound-glyph
                :key (str "cell-glyph-" track "-" pattern-id)
                :source (str "pattern-glyph:track:" track ":pattern:" pattern-id)
                ;; Quantize to a coarse virtual-pixel grid: the mixer shows ~50
                ;; glyphs at once, so the palette's hi-def gooey rendering reads
                ;; as noise at this size. Odd count keeps a cell centered.
                :pixelate 2
                :edge-soft 0.1
                :white-damp 0
                :height-in 0.1
                :height-out -0.08
                :height-amp 3
                :diffuse 0.05
                :rim-width 0.1
                ;; Leave the active launch mark visually dominant: its host-
                ;; driven play state shrinks and dims only the identity glyph.
                :play-glyph-padding 0.14
                :play-glyph-opacity 0.4
                :play-color :white
                :tint-r (* 0.1 (track-color-r track false))
                :tint-g (* 0.1 (track-color-g track false))
                :tint-b (* 0.1 (track-color-b track false))
                )))))))))

(def mod-output-style
  (ui/style
    :hover (dict
      :brightness 1.45
      :transition (dict :brightness 0.08 :ease :smoothstep))))

(def mod-port-row (track)
     (box :height 0.8 :width :fill 
  (h-stack :key (str "mod-ports-" track)
    :width 9.8 :height 0.1 :gap 0.42 :align :center
    (mixer-v2-mod-port
      :key (str "mod-out-" track)
      :patch-port true
      :direction :out
      :track track
      :active (track-mod-output? track)
      :pending (= pending-mod-source track)
      :output true
      :selected false
      :level (mod-out-level track)
      :style (if (track-mod-output? track) mod-output-style nil)
      :on-click |x y r| (mod-out-click track)
      :on-mouse-down |x y r| (mod-out-click track)
      :on-patch-cancel (lambda (source)
        (cancel-mod-draw))
      :on-patch-miss (lambda ()
        (clear-selected-mod-route)))
    (if (track-modulator? track)
      (each (range 0 4) |input|
        (box :key (str "mod-in-spacer-" track "-" input)
          :width 1.05 :height 1.05))
      (each (range 0 4) |input|
        (mixer-v2-mod-port
          :key (str "mod-in-" track "-" input)
          :patch-port true
          :direction :in
          :track track
          :input input
          :connected-sources (mod-route-sources track input)
          :selected-sources (selected-mod-sources track input)
          :active true
          :pending false
          :output false
          :selected (> (len (selected-mod-sources track input)) 0)
          :level (mod-in-level track input)
          :on-patch-drop (lambda (source dest input)
            (do
              (connect-mod-route source dest input)
              (set! pending-mod-source -1)))
          :on-cable-click (lambda (source dest input)
            (select-mod-route source dest input))
          :on-click |x y r| (mod-in-click track input)
          :on-mouse-up |x y r| (mod-in-click track input)))))))

(def bus-mod-port-row (bus-id)
  (box :height 0.8 :width :fill
    (h-stack :key (str "bus-mod-ports-" bus-id)
      :width 7.1 :height 0.1 :gap 0.42 :align :center
      (each (range 0 4) |input|
        (mixer-v2-mod-port
          :key (str "bus-mod-in-" bus-id "-" input)
          :patch-port true
          :direction :in
          :dest-kind "bus"
          :dest bus-id
          :input input
          :connected-sources (bus-mod-route-sources bus-id input)
          :selected-sources (selected-bus-mod-sources bus-id input)
          :active true
          :pending false
          :output false
          :selected (> (len (selected-bus-mod-sources bus-id input)) 0)
          :level (bus-mod-in-level bus-id input)
          :on-patch-drop (lambda (source dest input)
            (do
              (connect-bus-mod-route source bus-id input)
              (set! pending-mod-source -1)))
          :on-cable-click (lambda (source dest input)
            (select-mod-route-kind source "bus" bus-id input))
          :on-click |x y r| (bus-mod-in-click bus-id input)
          :on-mouse-up |x y r| (bus-mod-in-click bus-id input))))))

(defwidget mixer-v2-volume-triangle
  :width 1.35 :height 4.24
  :paint-margin 0.15
  :state (value)
  :bindable (value)
  :shader
  (let ((marker-y (* (- 1.0 (* 2.0 value)) (* height 1.04))))
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.04)
        (material :color (rgba 0 0 0 0)))
      (sdf/fill
        (sdf/translate 0 marker-y
          (let ((tx x)
                (ty (* y aspect))
                (p1x -0.82) (p1y -0.15) (p2x -0.82) (p2y 0.15) (p3x 0.70) (p3y 0.0))
            (let ((d1 (- (* (- p2x p1x) (- ty p1y)) (* (- p2y p1y) (- tx p1x))))
                  (d2 (- (* (- p3x p2x) (- ty p2y)) (* (- p3y p2y) (- tx p2x))))
                  (d3 (- (* (- p1x p3x) (- ty p3y)) (* (- p1y p3y) (- tx p3x)))))
              (max (max d1 d2) d3))))
        (material :color :mixer-volume-handle)))))

(def level-color (level)
  (if (> level 0.88)
    (rgba 0.95 0.18 0.16 1.0)
    (if (> level 0.70)
      (rgba 0.96 0.82 0.18 1.0)
      (rgba 0.10 0.85 0.30 1.0))))

(def meter-display (level-l level-r)
  (mixer-meter
    :level-l level-l :level-r level-r
    :width 2.22 :height 4.24
    :font-size 7 :label-height 0.42 :label-top-inset 0.0
    :label-color :dim))

(def track-meter (i)
  (subtree :key (str "mixer-v2-track-meter-" i)
    (meter-display (track-peak i) (track-peak i))))

(def bus-meter (i)
  (subtree :key (str "mixer-v2-bus-meter-" i)
    (meter-display (bus-peak-l i) (bus-peak-r i))))

(def track-meter-control (i)
  (box :width 3.65 :height 4.24
    :on-click (lambda (event)
      (do
        (clear-delete-target)
        (seq-set-track-volume i (event-volume event))))
    :on-drag (lambda (event)
      (do
        (clear-delete-target)
        (seq-set-track-volume i (event-volume event))))
    (h-stack :gap 0.06 :align :center
      (mixer-v2-volume-triangle
        :value (bind-seq (track-volume-field i))
        :on-click (lambda (sx sy region)
          (do
            (clear-delete-target)
            (seq-set-track-volume i (pointer-volume sy))))
        :on-drag (lambda (sx sy region)
          (do
            (clear-delete-target)
            (seq-set-track-volume i (pointer-volume sy)))))
      (track-meter i))))

;; `spacer` is the room left above the meter: the clip-area equivalent that
;; keeps a bus/group meter level with the track strips' meters, or 0 when the
;; caller already filled that room (a rack's clip column, mixer §6.2).
(def bus-meter-control-with-spacer (i is-group spacer)
  (box :width 3.65 :height 4.24
    :on-click (lambda (event)
      (do
        (select-bus i)
        ;(seq-set-bus-volume i (event-volume event))
        )
      )
    :on-drag (lambda (event)
      (do
        (select-bus i)
        ;;(seq-set-bus-volume i (event-volume event))
        ))
    (v-stack
      ;; Level with the track strips' meters, which sit below the clip area.
      (box :width :fill :height spacer)
      (h-stack :gap 0.06 
        (box :width (if is-group 6 2))
        (mixer-v2-volume-triangle
          :value (bind-seq-nth "bus-volumes" i)
          :on-click (lambda (sx sy region)
            (do
              (select-bus i)
              (seq-set-bus-volume i (pointer-volume sy))))
          :on-drag (lambda (sx sy region)
            (do
              (select-bus i)
              (seq-set-bus-volume i (pointer-volume sy)))))
        (bus-meter i)))))

(def bus-meter-control (i is-group)
  (bus-meter-control-with-spacer i is-group (if (compact?) 0.2 4.2)))

(def send-label (name)
  (if (= name "Bus A")
    "A"
    (if (= name "Bus B")
      "B"
      (substring name 0 3))))

(def send-field (track bus)
  (str "track-" track "-bus-" bus "-send"))

(def track-volume-field (track)
  (str "track-" track "-volume"))

(def track-pan-field (track)
  (str "track-" track "-pan"))

(def send-knob (track send)
  (let ((field (send-field track (get send :bus-idx)))
        (has-locks (= (reactive-get "SEQ" (str field "-plock-any")) 1))
        ;; A process OUT port writing this send: show its last value as the
        ;; amber knob dot (same read-only overlay as instrument params).
        (proc-mapped (= (reactive-get "SEQ" (str field "-proc-mapped")) 1))
        (target (dict :track track :target "bus-send" :param-idx (get send :bus-idx))))
    (box :debug-name (str "track-" track "-send-" (get send :bus-idx) "-plock")
      :plock-any (if has-locks 1 0)
      :on-right-click (lambda (event) (pc/open-target-plock-menu event target has-locks))
      (pc/process-send-map-wrapper track send (str "track-" track "-send-" (get send :bus-idx))
      (knob-number :label (send-label (get send :name))
        :key (str "track-" track "-send-" (get send :bus-idx))
        :value (bind-seq field)
        :plock-active (bind-seq (str field "-plock-active"))
        :plock-default (bind-seq (str field "-plock-default"))
        :plock-color-r (pc/param-plock-color-r)
        :plock-color-g (pc/param-plock-color-g)
        :plock-color-b (pc/param-plock-color-b)
        :process-value (if proc-mapped (bind-seq (str field "-proc-value")) false)
        :min 0 :max 1 :decimals 2
        :show-value false
        :font-size 9 :label-font-size 5
        :text-color :dim :label-color :dim
        :width 3.4 :height 2.0 :knob-size 1.44
        :on-change (lambda (v)
          (do
            (clear-delete-target)
            (host-command "set-track-bus-send"
              (dict :track track :bus (get send :bus-idx) :amount v)))))))))

(def track-strip (i)
  (let ((sends (nth SEQ.track-bus-sends i)))
    ;; Grouped strips drop the output dropdown, so they are shorter to avoid
    ;; dead space at the bottom inside the group container.
    ;; Mute/name/output reads live in bindings or nested subtrees so those
    ;; changes don't rerun the whole strip.
    (box :width track-strip-width :height (if (track-grouped? i) (grouped-strip-height) (strip-height))
      :selected (track-selected-binding i)
      :muted (bind-seq-nth "track-muted-effective" i)
      :background-color :mixer-strip-bg
      :selected-background-color :mixer-strip-selected-bg
      :muted-background-color :mixer-strip-muted-bg
      :border-width 4
      :corner-radius (eseq.seq-core-state/radius 16)
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :muted-border-color :mixer-strip-border
      :drop-hover-border-color :mixer-strip-selected-border
      :padding 0.45
      :drop-types (track-drop-types i)
      :drop-meta (dict :kind "track" :track i)
      :on-drop (lambda (event) (drop-on-track event))
      :on-click (lambda (event) (track-body-click event i))
      :on-right-click (lambda (event) (open-track-menu event i))
      (v-stack :gap 0.18 :width :fill
        ;; Grouped tracks drop the output dropdown (their output is the group
        ;; bus); the container provides the color above. A small spacer keeps
        ;; the pattern grid aligned with loose strips.
        (if (track-grouped? i)
          nil
          (subtree :key (str "mixer-v2-track-output-sub-" i)
            (dropdown :value (nth SEQ.track-outputs i)
              :key (str "track-output-" i)
              :options SEQ.track-output-options
              :on-change (lambda (v)
                (do
                  (clear-delete-target)
                  (host-command "set-track-output" (dict :track i :label v))))
              :width :fill :height 1.2 :font-size 10
              :corner-radius (eseq.seq-core-state/radius 20))))
        (if (compact?) nil (track-pattern-grid i))
        
        (box :width :fill
        (h-stack :gap 0 :align :center :width :fill
          (v-stack
            
            (h-stack :gap 0.05
              ;; Stable identities keep only Bus A / Bus B here even when
              ;; other buses are created or the bus list is reordered.
              (each (filter (lambda (send)
                    (or (= (get send :bus-id) 1) (= (get send :bus-id) 2))) sends) |send|
                (send-knob i send)))
            
            (knob-number :label "pan"
              :key (str "track-pan-" i)
              :value (bind-seq (track-pan-field i))
              :min -1 :max 1 :origin 0 :decimals 2
              :font-size 9 :label-font-size 8
              :text-color :dim :label-color :dim
              :width 6.5 :height 2.35 :knob-size 2.58
              :on-change (lambda (v)
                (do
                  (clear-delete-target)
                  (seq-set-track-pan i v)))))
          (box :flex 1 :height 1)
          (track-meter-control i)))
        
        (box :width :fill :height 0.05)
        (subtree :key (str "mixer-v2-strip-buttons-" i)
          (strip-buttons i))
        (mod-port-row i)
        (subtree :key (str "mixer-v2-strip-label-" i)
          (strip-label i))))))

;; Mute and solo only repaint bound button states; they never rebuild the strip.
(def strip-buttons (i)
  (let ((muted (bind-seq-nth "track-muted-effective" i)))
    (h-stack :gap 0.35
      (button (str (+ i 1))
        :width 2.1 :height 1.0 :padding 0 :font-size 10
        :border-color :transparent
        :active muted
        :background-color :control-on-bg
        :active-background-color :mixer-control-bg
        :color :control-on-fg
        :active-color :dim
        :on-click (lambda (event) (do (activate-track-control i) (seq-toggle-track-mute i))))
      (button "S"
        :width 2.1 :height 1.0 :padding 0 :font-size 10
        :border-color :transparent
        :active (bind-seq-nth "track-solos" i)
        :background-color :mixer-control-bg
        :active-background-color :control-on-bg
        :color :dim
        :active-color :control-on-fg
        :on-click (lambda (event) (do (activate-track-control i) (seq-toggle-track-solo i))))
      (button "R"
        :width 2.1 :height 1.0 :padding 0 :font-size 10
        :border-color :transparent
        :background-color (arm-bg (nth SEQ.record-armed i))
        :color (if (nth SEQ.record-armed i) :black :dim)
        :on-click (lambda (event) (do (activate-track-control i) (seq-toggle-record-arm i)))))))

(def open-track-menu (event i)
  (do
    (set! track-menu-track i)
    (set! track-menu-group-id -1)
    (set! track-menu-col (get event :col))
    (set! track-menu-row (get event :row))
    (set! track-menu-open true)))

(def open-group-menu (event gidx)
  (do
    (set! track-menu-track -1)
    (set! track-menu-group-id (get (nth SEQ.groups gidx) :id))
    (set! track-menu-col (get event :col))
    (set! track-menu-row (get event :row))
    (set! track-menu-open true)))

(def begin-track-rename (i)
  (do
    (set! track-menu-open false)
    (set! track-renaming i)
    (set! track-rename-draft (nth SEQ.track-names i))))

(def finish-track-rename (i commit)
  (if (= track-renaming i)
    (do
      (if commit
        (host-command "rename-track" (dict :track i :name track-rename-draft))
        nil)
      (set! track-renaming -1)
      (set! track-rename-draft ""))
    nil))

(def begin-group-rename (group-id)
  (let ((gidx (group-index-by-id group-id)))
    (if (>= gidx 0)
      (do
        (set! track-menu-open false)
        (set! group-renaming group-id)
        (set! group-rename-draft (get (nth SEQ.groups gidx) :name)))
      nil)))

(def finish-group-rename (group-id commit)
  (if (= group-renaming group-id)
    (do
      (if commit
        (host-command "rename-group"
          (dict :group-id group-id :name group-rename-draft))
        nil)
      (set! group-renaming -1)
      (set! group-rename-draft ""))
    nil))

(def track-menu-target-selected? ()
  (> (len (filter (lambda (track) (= track track-menu-track)) SEQ.selected-tracks)) 0))

(def track-context-menu-actions ()
  (if (>= track-menu-group-id 0)
    (let ((gidx (group-index-by-id track-menu-group-id)))
      (if (< gidx 0)
        (list)
        (if (get (nth SEQ.groups gidx) :rack)
          (rack-group-menu-actions track-menu-group-id)
          group-menu-actions)))
    (append
      track-menu-actions
      (if (track-grouped? track-menu-track)
        (list (dict :id :ungroup-track :label "Ungroup track"))
        (list))
      (if (and (>= (len SEQ.selected-tracks) 2) (track-menu-target-selected?))
        (list (dict :id :group :label "Group Tracks"))
        (list)))))

(def select-track-menu-action (action)
  (if (= (get action :id) :rename)
    (if (>= track-menu-group-id 0)
      (begin-group-rename track-menu-group-id)
      (begin-track-rename track-menu-track))
    (if (= (get action :id) :convert-drum-rack)
      (do
        (set! track-menu-open false)
        (host-command "convert-group-to-drum-rack"
          (dict :group-id track-menu-group-id)))
      (if (= (get action :id) :group)
        (do
          (set! track-menu-open false)
          (group-selected))
        (if (or (= (get action :id) :ungroup)
                (= (get action :id) :ungroup-track))
          (do
            (set! track-menu-open false)
            (if (= (get action :id) :ungroup-track)
              (host-command "remove-track-from-group"
                (dict :track track-menu-track))
              (host-command "ungroup-tracks"
                (dict :group-id track-menu-group-id))))
          (if (= (get action :id) :export-kit)
            (do
              (set! track-menu-open false)
              (eseq.browser/enter-kit-save track-menu-group-id
                (get (nth SEQ.groups (group-index-by-id track-menu-group-id)) :name)))
          (if (= (get action :id) :move-sequencer-into-rack)
            (do
              (set! track-menu-open false)
              (host-command "move-sequencer-into-rack"
                (dict :group-id track-menu-group-id
                      :sequencer-id (get action :sequencer-id)
                      :source-path (sequencer-source-path (get action :sequencer-name)))))
            (if (= (get action :id) :detach-sequencer)
              (do
                (set! track-menu-open false)
                (host-command "detach-rack-sequencer"
                  (dict :group-id track-menu-group-id
                        :sequencer-id (get action :sequencer-id))))
              (if (= (get action :id) :convert-to-clips)
                (do
                  (set! track-menu-open false)
                  (eseq.drum-rack-v2/convert-to-clips track-menu-group-id))
                (if (= (get action :id) :save-rack-clip)
                  (do
                    (set! track-menu-open false)
                    (eseq.drum-rack-v2/save-clip-as track-menu-group-id ""))
                  nil))))))))))

(def track-context-menu ()
  (context-menu :is-open track-menu-open
    :anchor-col track-menu-col
    :anchor-row track-menu-row
    :on-close (lambda () (set! track-menu-open false))
    (each (track-context-menu-actions) |action|
      (menu-item (get action :label)
        :key (str "track-menu-" (get action :id))
        :on-select (lambda (event) (select-track-menu-action action))))))

(def rename-input (key width font-size value on-change on-submit on-cancel)
  (text-input
    :key key
    :width width :height 1.0 :font-size font-size
    :value value
    :auto-focus true
    :select-all-on-focus true
    :on-change on-change
    :on-submit on-submit
    :on-cancel on-cancel
    :on-blur on-submit))

(def track-rename-input (i key-prefix width font-size)
  (rename-input
    (str key-prefix i) width font-size track-rename-draft
    (lambda (name) (set! track-rename-draft name))
    (lambda () (finish-track-rename i true))
    (lambda () (finish-track-rename i false))))

(def group-rename-input (group-id)
  (rename-input
    (str "group-rename-input-" group-id) 6.8 9 group-rename-draft
    (lambda (name) (set! group-rename-draft name))
    (lambda () (finish-group-rename group-id true))
    (lambda () (finish-group-rename group-id false))))

;; Renames rebuild the label; mute/solo and selection only repaint bindings.
(def strip-label (i)
  (let ((muted (bind-seq-nth "track-muted-effective" i)))
    (box
      :key (str "track-label-" i)
      :width :fill :height 1.0
      :corner-radius (eseq.seq-core-state/radius 30)
      :padding 0
      :drag-type "track-badge"
      :drag-payload (dict :track i)
      :selected (track-delete-target-binding i)
      :muted muted
      :background-color (rgba
        (track-color-r i false)
        (track-color-g i false)
        (track-color-b i false)
        1.0)
      :muted-background-color (rgba
        (track-color-r i true)
        (track-color-g i true)
        (track-color-b i true)
        1.0)
      :selected-background-color :fx-panel-header-selected-bg
      :on-click (lambda (event) (track-label-click event i))
      :on-double-click (lambda (event) (eseq.sequencer/open-piano-roll-for-track i))
      (if (= track-renaming i)
        (track-rename-input i "track-rename-input-" (* 9.8 (clip-cell-scale)) 10)
        (badge (substring (nth SEQ.track-names i) 0 (name-chars 12))
          :key (str "track-label-content-" i)
          :icon (eseq.track-collapse/type-icon i)
          :width (* 9.8 (clip-cell-scale))
          :height 1.0
          :padding 0
          :font-size 10
          :v-align :center
          :h-align :left
          :background-color :transparent
          :border-color :transparent
          :highlight-color :transparent
          :shadow-color :transparent
          :muted muted
          :color :black
          :muted-color :dim
          :active (track-delete-target-binding i)
          :active-color :white
          :bg :transparent)))))

(def track-collapsed-label (i)
  (str (+ i 1) " " (substring (nth SEQ.track-names i) 0 3)))

(def track-collapsed-strip (i)
  (let ((muted (bind-seq-nth "track-muted-effective" i)))
    (box :width 4.7 :height (collapsed-strip-height)
      :selected (track-selected-binding i)
      :muted muted
      :background-color :mixer-strip-bg
      :selected-background-color :mixer-strip-selected-bg
      :muted-background-color :mixer-strip-muted-bg
      :border-width 2
      :corner-radius (eseq.seq-core-state/radius 10)
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :muted-border-color :mixer-strip-border
      :drop-hover-border-color :mixer-strip-selected-border
      :padding 0.45
      :drop-types (track-drop-types i)
      :drop-meta (dict :kind "track" :track i)
      :on-drop (lambda (event) (drop-on-track event))
      :on-click (lambda (event) (track-body-click event i))
      :on-right-click (lambda (event) (open-track-menu event i))
      (v-stack :gap 0.42 :align :center
        ;; Spacer absorbs the clip-area growth so the meter stays level with
        ;; the full strips' meters.
        (box :width :fill :height (max 0 (+ 3.45 (- (clip-area-height) 4.0))) :bg :transparent)
        (track-meter-control i)
        (button "M"
          :key (str "track-collapsed-mute-" i)
          :width 3.65 :height 1.0 :padding 0 :font-size 10
          :active (bind-seq-nth "track-mutes" i)
          :background-color :mixer-control-bg
          :active-background-color :control-on-bg
          :color :dim
          :active-color :control-on-fg
          :on-click (lambda (event) (do (activate-track-control i) (seq-toggle-track-mute i))))
        (box
          :key (str "track-collapsed-label-" i)
          :width 3.65 :height 1.0
          :padding 0
          :selected (track-delete-target-binding i)
          :muted muted
          :background-color (rgba
            (track-color-r i false)
            (track-color-g i false)
            (track-color-b i false)
            1.0)
          :muted-background-color (rgba
            (track-color-r i true)
            (track-color-g i true)
            (track-color-b i true)
            1.0)
          :selected-background-color :fx-panel-header-selected-bg
          :on-click (lambda (event) (track-label-click event i))
          :on-double-click (lambda (event) (eseq.sequencer/open-piano-roll-for-track i))
          (if (= track-renaming i)
            (track-rename-input i "track-collapsed-rename-input-" 3.65 9)
            (badge (track-collapsed-label i)
              :key (str "track-collapsed-label-content-" i)
              :icon (eseq.track-collapse/type-icon i)
              :width 3.65
              :height 1.0
              :padding 0
              :font-size 9
              :h-align :center
              :background-color :transparent
              :border-color :transparent
              :highlight-color :transparent
              :shadow-color :transparent
              :muted muted
              :color :black
              :muted-color :dim
              :active (track-delete-target-binding i)
              :active-color :white
              :bg :transparent)))))))

(def bus-label (i)
  (if (= i 0) "Main" (nth SEQ.bus-names i)))

(def bus-mute-label (i)
  (if (= i 0) "M" (if (= i 1) "A" (if (= i 2) "B" (str i)))))

(def has-mix-bus? ()
  (and (> (len SEQ.bus-names) 0) (= (nth SEQ.bus-names 0) "Mix")))

(def display-bus-index (display-i)
  (if (or (not (has-mix-bus?)) (<= (len SEQ.bus-names) 1))
    display-i
    (if (= display-i (- (len SEQ.bus-names) 1))
      0
      (+ display-i 1))))

(def bus-display-index (bus-i)
  (if (or (not (has-mix-bus?)) (<= (len SEQ.bus-names) 1))
    bus-i
    (if (= bus-i 0)
      (- (len SEQ.bus-names) 1)
      (- bus-i 1))))

(def channel-count ()
  (+ SEQ.num-tracks (len SEQ.bus-names)))

(def current-channel-index ()
  (if (and (>= eseq.seq-core-state/selected-bus 0) (< eseq.seq-core-state/selected-bus (len SEQ.bus-names)))
    (+ SEQ.num-tracks (bus-display-index eseq.seq-core-state/selected-bus))
    (let ((selected (filter
            (lambda (i) (> (reactive-value (bind-seq (str "track-selected-" i))) 0.5))
            (range 0 SEQ.num-tracks))))
      (if (> (len selected) 0)
        (nth selected 0)
        0))))

(def select-channel-index (idx)
  (let ((clamped (min (max idx 0) (- (max (channel-count) 1) 1))))
    (if (< clamped SEQ.num-tracks)
      (do
        (set! eseq.seq-core-state/selected-bus -1)
        (clear-delete-target)
        (seq-set-track clamped))
      (do
        (clear-delete-target)
        (seq-clear-selection)
        (set! eseq.seq-core-state/selected-bus (display-bus-index (- clamped SEQ.num-tracks)))))))

(def select-prev-channel ()
  (do
    (select-channel-index (- (current-channel-index) 1))
    true))

(def select-next-channel ()
  (do
    (select-channel-index (+ (current-channel-index) 1))
    true))

(def delete-selected-track ()
  (seq-delete-active-target))

;; BS/Delete: an armed delete target (an explicit click on a strip's delete
;; badge) wins; with nothing armed the handler declines the key so the
;; inherited sequencer keymap deletes the selected steps instead.
(def delete-key ()
  (if (seq-delete-active-target) true false))

(def handle-key (key text)
  (if (= key "LEFT")
    (select-prev-channel)
    (if (= key "RIGHT")
      (select-next-channel)
      (if (= key "BS")
        (delete-key)
        (if (= key "Delete")
          (delete-key)
          false)))))

(def bus-output-dropdown (i)
  (subtree :key (str "mixer-bus-output-" i)
    (let ((route (nth SEQ.bus-output-routes i))
        (options (get route :options))
        (ids (get route :ids)))
      (if (> (len options) 0)
        (dropdown :key (str "bus-output-" i)
          :value (get route :value) :options options
          :width :fill :height 1.2 :font-size 10
          :corner-radius (eseq.seq-core-state/radius 20)
          :on-change (lambda (value)
            (each (range 0 (len options)) |index|
              (if (= value (nth options index))
                (host-command "set-bus-output"
                  (dict :bus-id (nth SEQ.bus-ids i) :destination-id (nth ids index)))
                nil))))
        (box :height 1.2 :width :fill)))))

(def bus-strip (i)
  (do
    ;; `do` keeps the original body indentation; the strip is one box.
    (box :key (str "bus-strip-" i)
      :width bus-strip-width :height (bus-strip-height)
      ;; Bound selection state (eseq-4jv): a raw `selected-bus` read here
      ;; re-rendered every bus strip on each selection.
      :selected (eseq.seq-core-state/bus-selected-vis-binding i)
      :muted (bind-seq-nth "bus-mutes" i)
      :background-color :mixer-strip-bg
      :selected-background-color :mixer-strip-selected-bg
      :muted-background-color :mixer-strip-muted-bg
      :border-width 2
      :corner-radius (eseq.seq-core-state/radius 16)
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :drop-hover-border-color :mixer-strip-selected-border
      :padding 0.45
      :drop-types (list "audio-effect")
      :drop-meta (dict :kind "bus" :bus i)
      :on-drop (lambda (event) (drop-effect-on-bus event))
      :on-click (lambda (event) (select-bus i))
      (v-stack :gap 0.25
        (bus-output-dropdown i)
        (clip-growth-spacer)
        ;; Mix/Main is the graph output and has no external modulation inputs.
        ;; Every other bus has the same four backend inputs used by group buses.
        ;(if (= (nth SEQ.bus-names i) "Mix")
        ;  (box :height 0.8 :width :fill :bg :transparent)
        ;  )
        (h-stack :gap 0.45 :align :center
          (box :width 3.0 :height (if (compact?) 1.0 5.0))
          (bus-meter-control i false))
        (box :height (if (compact?) 0 2.8))
        (h-stack :gap 0.35
          (button (bus-mute-label i)
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :active (bind-seq-nth "bus-mutes" i)
            :background-color :control-on-bg
            :active-background-color :mixer-control-bg
            :border-color :transparent
            :color :control-on-fg
            :active-color :dim
            :on-click (lambda (event) (do (select-bus i) (seq-toggle-bus-mute i))))
          (button "S"
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :active (bind-seq-nth "bus-solos" i)
            :background-color :mixer-control-bg
            :active-background-color :control-on-bg
            :border-color :transparent
            :color :dim
            :active-color :control-on-fg
            :on-click (lambda (event) (do (select-bus i) (seq-toggle-bus-solo i))))
          (box :width 2.1 :height 1.0))
        (if (not (= (nth SEQ.bus-names i) "Mix"))
          (bus-mod-port-row (nth SEQ.bus-ids i))
          (box :height (if (compact?) 0 0.8))
          )
        
        (button (substring (bus-label i) 0 (bus-name-chars))
          :width :fill :height 1.0 :padding 0 :font-size 10
          :background-color :mixer-label-bg
          :border-color :transparent
          :color :white
          :on-click (lambda (event) (select-bus i)))))))

;; --- Track groups -------------------------------------------------------

(def list-contains? (xs v)
  (> (len (filter (lambda (x) (= x v)) xs)) 0))

;; A group drawn inside another group's block (a rack in a plain group,
;; docs/drum-rack-v2-spec.md) is not a top-level render item: its parent draws
;; it. `:parent` is the containing group's id, or -1.
(def group-nested? (gidx)
  (let ((p (get (nth SEQ.groups gidx) :parent)))
    (if p (>= p 0) false)))

;; Index (in SEQ.groups) of the group with the given id, else -1.
(def group-index-by-id (gid)
  (reduce |acc gidx|
    (if (>= acc 0) acc (if (= (get (nth SEQ.groups gidx) :id) gid) gidx acc))
    -1
    (range 0 (len SEQ.groups))))

;; Index (in SEQ.groups) of the TOP-LEVEL group anchored at track i (lowest
;; member), else -1. A nested rack is skipped here and drawn by its parent; the
;; parent itself anchors at the lowest track of any of its units, so a nested
;; rack's tracks still land inside the parent's block.
(def group-anchored-at (i)
  (reduce |acc gidx|
    (if (>= acc 0)
      acc
      (if (and (not (group-nested? gidx))
            (= (group-anchor gidx) i))
        gidx
        acc))
    -1
    (range 0 (len SEQ.groups))))

;; Index (in SEQ.groups) of the group containing track i, else -1.
(def group-of-track (i)
  (reduce |acc gidx|
    (if (>= acc 0)
      acc
      (if (list-contains? (get (nth SEQ.groups gidx) :members) i) gidx acc))
    -1
    (range 0 (len SEQ.groups))))

;; Child rack indices (in SEQ.groups) of a group, in id order.
(def child-racks (gidx)
  (filter (lambda (c) (>= c 0))
    (map (lambda (gid) (group-index-by-id gid))
      (or (get (nth SEQ.groups gidx) :rack-members) (list)))))

;; Where a group sits in the flat track order: the lowest member track of the
;; group itself or of any rack nested inside it. -1 when nothing is claimed yet.
(def group-anchor (gidx)
  (let ((own (if (= (len (get (nth SEQ.groups gidx) :members)) 0)
            -1
            (get (nth SEQ.groups gidx) :anchor))))
    (reduce |acc child|
      (let ((a (if (= (len (get (nth SEQ.groups child) :members)) 0)
              -1
              (get (nth SEQ.groups child) :anchor))))
        (if (< acc 0) a (if (< a 0) acc (if (< a acc) a acc))))
      own
      (child-racks gidx))))

(def track-grouped? (i)
  (>= (group-of-track i) 0))

(def group-color (gidx)
  (let ((c (get (nth SEQ.groups gidx) :color)))
    (if (>= (len c) 3) c (list 0.5 0.5 0.5))))

;; Storage index of the bus with the given id, or -1.
(def bus-index-by-id (bid)
  (reduce |acc i|
    (if (>= acc 0)
      acc
      (if (= (nth SEQ.bus-ids i) bid) i acc))
    -1
    (range 0 (len SEQ.bus-ids))))

;; Set of bus ids that back a group (hidden from the ordinary bus list).
(def group-bus-id? (bid)
  (reduce |acc gidx|
    (or acc (= (get (nth SEQ.groups gidx) :bus-id) bid))
    false
    (range 0 (len SEQ.groups))))

;; Build the flat mixer render-item list: loose tracks and group containers,
;; each group anchored at its lowest member index (visual contiguity without
;; reindexing the track Vec).
;; Top-level groups that have claimed no track yet (an empty rack: its pads are
;; lazy) have no anchor, so they follow the tracks — the grid does the same.
(def unanchored-groups ()
  (reduce |acc gidx|
    (if (and (not (group-nested? gidx)) (< (group-anchor gidx) 0))
      (append acc (list (dict :kind "group" :gidx gidx)))
      acc)
    (list)
    (range 0 (len SEQ.groups))))

(def render-order ()
  (append
    (reduce |acc i|
      (let ((ganch (group-anchored-at i)))
        (if (>= ganch 0)
          (append acc (list (dict :kind "group" :gidx ganch)))
          (if (track-grouped? i)
            acc
            (append acc (list (dict :kind "loose" :track i))))))
      (list)
      (range 0 SEQ.num-tracks))
    (unanchored-groups)))

(def toggle-group-collapsed (gid)
  (seq-toggle-group-collapsed gid))

(def select-group (gidx)
  (let ((idx (bus-index-by-id (get (nth SEQ.groups gidx) :bus-id))))
    (if (>= idx 0)
      (select-bus idx)
      false)))

(def select-group-delete-target (gidx)
  (let ((group (nth SEQ.groups gidx)))
    (do
      (select-group gidx)
      (seq-set-delete-target :mixer-group (dict :group-id (get group :id))))))

;; Selection visibility rides the *sel-sync* SEQV projection
;; (ui/seq-core-state.lisp): a render-time `selected-bus` read here
;; re-rendered the whole group container — every member strip — on each
;; selection (eseq-4jv).
(def group-selected-binding (gidx)
  (eseq.seq-core-state/group-selected-vis-binding (get (nth SEQ.groups gidx) :id)))

;; Mute and solo always operate on the group's backing bus. A rack additionally
;; owns a pad-play arm state; a plain mixer group deliberately has no arm
;; control because it is not an input target.
(def group-control-buttons (gidx bus-idx)
  (let ((gid (get (nth SEQ.groups gidx) :id))
      (rack (get (nth SEQ.groups gidx) :rack))
      (muted (bind-seq-nth "bus-mutes" bus-idx))
      (soloed (bind-seq-nth "bus-solos" bus-idx))
      (armed (and rack (= SEQ.armed-rack-id gid))))
    (h-stack :gap 0.35 :align :left :padding 0.1 :width :fill
      ;; Mute is lit when the strip is *passing* audio and goes dark when
      ;; muted, matching the track and bus strips right next to it.
      (button "M"
        :key (str "group-mute-" gid)
        :width 2.1 :height 1.0 :padding 0 :font-size 10
        :border-color :transparent
        :active muted
        :background-color :control-on-bg
        :active-background-color :mixer-control-bg
        :color :control-on-fg
        :active-color :dim
        :on-click (lambda (event)
          (do (select-group gidx) (seq-toggle-bus-mute bus-idx))))
      (button "S"
        :key (str "group-solo-" gid)
        :width 2.1 :height 1.0 :padding 0 :font-size 10
        :border-color :transparent
        :active soloed
        :background-color :mixer-control-bg
        :active-background-color :control-on-bg
        :color :dim
        :active-color :control-on-fg
        :on-click (lambda (event)
          (do (select-group gidx) (seq-toggle-bus-solo bus-idx))))
      (if rack
        (button "R"
          :key (str "group-arm-" gid)
          :width 2.1 :height 1.0 :padding 0 :font-size 10
          :border-color :transparent
          :background-color (arm-bg armed)
          :color (if armed :black :dim)
          :on-click (lambda (event)
            (do (select-group gidx) (seq-toggle-rack-arm gid))))
        (box :width 0.0 :height 0.0 :bg :transparent)))))

;; The group's own channel slot (collapse toggle + name) shown at the left of
;; the container, over the container color. It matches a bus strip's width so
;; the full rack mute/solo/arm row has room without crowding the container.
;; Sized so column + the v-stack gap + the meter box equal the spacer-plus-
;; 8.1 meter box a clipless group strip has: the meters stay level.
;; Play glyph for a clip row: a rounded tile with a disclosure triangle that
;; lights when the row is the clip the current scene plays.
(defwidget rack-clip-play
  :width 1.4 :height 1.4
  :state (playing)
  :bindable (playing)
  :paint-margin 0.4
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width width 0.06)
      (material :color (if playing (rgba 0 0 0 1) (rgba 0 0 0 0.35))))
    (sdf/fill (sdf/scale 1.02 (sdf/disclosure-right))
      (material :color (if (= playing 1)
          (rgba 0.30 0.95 0.55 1.0)
          (rgba 0.05 0.05 0.06 0.5))))))

(def rack-clip-row-height 0.9)

;; Index of the active clip in the bank, or -1 (no follow) for silence.
(def rack-clip-active-row (clips active)
  (reduce |acc i|
    (if (and (< acc 0) (= (get (nth clips i) :id) active)) i acc)
    -1
    (range 0 (len clips))))

;; Follow is a numeric binding, like the playing glyphs. Keep its layout
;; arithmetic in Lisp and update it without reconstructing the clip column.
(observe
  (each (or SEQ.rack-clips (list)) |bank|
    (let ((row (rack-clip-active-row (get bank :clips) (get bank :active))))
      (reactive-set "SEQV" (str "rack-clip-center-" (get bank :group-id))
        (if (>= row 0) (* row (+ rack-clip-row-height 0.01)) -1)))))

;; Sized so column + the v-stack gap + the meter box equal the spacer-plus-
;; 8.1 meter box a clipless group strip has: the meters stay level. Past the
;; visible rows the list scrolls, following the active clip the way the
;; session-view package follows a track's effective clip: only a changed row
;; scrolls, so a manual scroll in between holds.
(def rack-clip-column (gid gidx)
  (let ((clips (eseq.drum-rack-v2/clips gid))
      (c (group-color gidx)))
    (let ((row-bg (rgba (nth c 0) (nth c 1) (nth c 2) 1.0)))
      (box :key (str "rack-clip-column-" gid)
        :width :fill :height (- (clip-area-height) 0.1) :align :top
        :bg :black :background-color :buffer-bg
        (scroll :key (str "rack-clip-scroll-" gid) :width :fill :height :fill
          :center-row (bind "SEQV" (str "rack-clip-center-" gid))
          :center-span rack-clip-row-height
          (v-stack :gap 0.01
            (each clips |clip|
              (box :key (str "mixer-rack-clip-" gid "-" (get clip :id))
                :debug-name "mixer-rack-clip-cell"
                :width :fill :height rack-clip-row-height :padding 0.01
                :corner-radius 0
                :background-color row-bg
                :on-click (lambda (event)
                  (eseq.drum-rack-v2/launch-clip gid (get clip :id)))
                (h-stack :gap 0.3 :align :center
                  (rack-clip-play :playing (bind-seq (str "rack-clip-active-" gid "-" (get clip :id))))
                  (label (substring (get clip :name) 0 (name-chars 9))
                    :key (str "mixer-rack-clip-label-" gid "-" (get clip :id))
                    :font-size 9 :h-align :left :v-align :center
                    :background-color :transparent :border-color :transparent
                    :highlight-color :transparent :shadow-color :transparent
                    :color :black
                    :bg :transparent))))))))))

(def group-header-slot (gidx)
  (let ((group (nth SEQ.groups gidx))
      (c (group-color gidx))
      (bus-idx (bus-index-by-id (get (nth SEQ.groups gidx) :bus-id)))
      ;; Compact mode has no clip area for the column to stand in.
      (show-clips (and (not (compact?))
        (eseq.drum-rack-v2/has-clips? (get (nth SEQ.groups gidx) :id)))))
    (box :key (str "group-bus-strip-" bus-idx)
      :width 10.2 :height (group-bus-strip-height)
      :corner-radius (eseq.seq-core-state/radius 12)
      :padding 0.1
      :background-color :mixer-strip-bg
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types (if (>= bus-idx 0)
        (list "sample" "instrument" "audio-effect")
        (list))
      :drop-meta (dict :kind "bus" :bus bus-idx)
      :on-drop (lambda (event) (drop-on-group-header event gidx))
      (v-stack :gap 0.3 :align :center
        (bus-output-dropdown bus-idx)
        ;; Where a plain track strip shows its pattern grid, a clip-bearing
        ;; rack shows its clip run vertically (§6.2). Everything else keeps the
        ;; spacer that levels the meters.
        (if show-clips
          (rack-clip-column (get group :id) gidx)
          (clip-growth-spacer))
        ;; Meter + fader reflect the group's backing bus. Selecting/dragging
        ;; them selects the group's bus (bus-meter-control selects by
        ;; index). Fall back to nothing if the bus can't be resolved. With the
        ;; clip column above, the meter gives up the spacer that kept it level.
        (if (>= bus-idx 0)
          (v-stack :gap 0.4 :align :center
            (box :width :fill :height (if (compact?) 4.1 (if show-clips 3.9 8.1))
              (if show-clips
                (bus-meter-control-with-spacer bus-idx true 0.0)
                (bus-meter-control bus-idx true))
              )
            )
          (box :width 0.0 :height 0.0 :bg :transparent))
        (group-control-buttons gidx bus-idx)
        (bus-mod-port-row (get group :bus-id))
        (box :corner-radius (eseq.seq-core-state/radius 34) :background-color c :width 9.5 :padding 0.1
          
          :key (str "group-badge-" (get group :id))
          :selected (group-delete-target? (get group :id))
          :selected-background-color :fx-panel-header-selected-bg
          :on-click (lambda (event) (select-group-delete-target gidx))
          :on-double-click (lambda (event) (eseq.sequencer/show-fx-for-group gidx))
          :on-right-click (lambda (event) (open-group-menu event gidx))
          (h-stack :gap 0.2 :align :center
            (box :width 0.05)
            (box :background "disclosure-button"
              :width 1.7 :height 0.8
              :collapsed (get group :collapsed)
              :surface-alpha 0.35
              :on-click (lambda (event)
                (do
                  (select-group gidx)
                  (toggle-group-collapsed (get group :id)))))
            (box :width 0.05)
            (if (= group-renaming (get group :id))
              (group-rename-input (get group :id))
              (label (substring (get group :name) 0 (name-chars 10))
                :key (str "group-name-label-" (get group :id))
                :font-size 11
                :height 0.9
                :v-align :center
                :h-align :center
                :background-color :transparent
                :border-color :transparent
                :highlight-color :transparent
                :shadow-color :transparent
                :color :black
                :bg :transparent)))        )))))

(def drop-new-track-into-group (event gidx)
  (let ((payload (get event :payload))
      (group (nth SEQ.groups gidx)))
    (let ((path (get payload :path))
        (name (get payload :name))
        (group-id (get group :id)))
      (do
        (select-group gidx)
        (if (= (get event :drag-type) "instrument")
          ;; The builtin add-track host commands take no :group-id (a rack even
          ;; creates its own group), so a builtin dropped on a group header is
          ;; refused rather than silently landing outside the group (eseq-mj8).
          (if (= (get payload :kind) "builtin-instrument")
            (status "Builtin instruments cannot be added inside a group")
            (if name
              (do
                (set! sbrowser-loading-instrument-name name)
                (host-command "add-track-instrument" (dict :name name :group-id group-id)))
              (status "Drop an instrument, not a folder")))
          (if path
            (host-command "add-track-sample"
              (dict :path path :group-id group-id :preserve-browser-context true))
            (status "Drop a sample file, not a folder")))))))

(def drop-on-group-header (event gidx)
  (if (= (get event :drag-type) "audio-effect")
    (drop-effect-on-bus event)
    (drop-new-track-into-group event gidx)))

(def group-member-strip (i)
  (if (eseq.track-collapse/collapsed? i)
    (track-collapsed-strip i)
    (track-strip i)))

;; A group rendered as a real container: a colored box wrapping the group's
;; channel slot and its member strips, with a top spacer so the color shows
;; above the contained tracks.
(def group-container (gidx)
  (let ((group (nth SEQ.groups gidx))
      (c (group-color gidx)))
    (box
      :corner-radius (eseq.seq-core-state/radius 16)
      :padding 0.3
      ;; Selection is a bound state, not a computed color (eseq-4jv): the
      ;; selected look keeps a constant border width so it never relayouts.
      :selected (group-selected-binding gidx)
      :background-color (rgba (nth c 0) (nth c 1) (nth c 2) 0.78)
      :selected-background-color (rgba (nth c 0) (nth c 1) (nth c 2) 1.0)
      :border-width 2
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types (list "track-badge")
      :drop-meta (dict :kind "group" :gidx gidx)
      :on-drop (lambda (event) (drop-track-into-group event gidx))
      :on-click (lambda (event) (select-group gidx))
      (h-stack :gap 0.0 :align :start
        (group-header-slot gidx)
        (if (get group :collapsed)
          (box :width 0.0 :height 0.0 :bg :transparent)
          (v-stack :gap 0.0
            (box :width :fill :height 0.85 :bg :transparent)
            (h-stack :gap 0.1
              (each (get group :members) |m|
                (subtree :key (str "mixer-v2-track-" m)
                  (group-member-strip m)))
              ;; Child racks draw as their own container inside this block —
              ;; collapsed, that is a single header strip; expanded, the rack
              ;; header plus its members (docs/drum-rack-v2-spec.md).
              (each (child-racks gidx) |child|
                (subtree :key (str "mixer-v2-group-" (get (nth SEQ.groups child) :id))
                  (group-container child))))
            ))))))

(def render-item (item)
  (let ((kind (get item :kind)))
    (if (= kind "group")
      (let ((gidx (get item :gidx)))
        (subtree :key (str "mixer-v2-group-" (get (nth SEQ.groups gidx) :id))
          (group-container gidx)))
      (let ((i (get item :track)))
        (subtree :key (str "mixer-v2-track-" i)
          (if (eseq.track-collapse/collapsed? i)
            (track-collapsed-strip i)
            (track-strip i)))))))

(def sample-drop-zone ()
  (box :key "sample-drop-zone"
    :width 11.8 :height (drop-zone-height)
    :background-color :buffer-bg
    :drop-hover-background-color :mixer-control-bg
    :border-width 2
    :border-color :mixer-strip-border
    :drop-hover-border-color :mixer-strip-selected-border
    :corner-radius (eseq.seq-core-state/radius 16)
    :padding 0.5
    :align :center
    :drop-types (list "sample" "instrument" "sound" "track-badge")
    :drop-meta (dict :kind "new-sample-track")
    :on-drop (lambda (event) (drop-sample-new-track event))
    (label "Drop sounds here"
      :font-size 9.5
      :color :gray
      :bg :transparent)))

;; ── Patch-editor mixer slot ──
;; One compact channel strip for the current track: volume/meter,
;; mute/solo/arm, and the name badge. No clip grid, output routing,
;; sends, or mod ports — those stay in the full *mixer* buffer.
(def patch-mixer-strip (i)
  (box :width 10.0 :height 10.7
    :selected (track-selected-binding i)
    :muted (bind-seq-nth "track-muted-effective" i)
    :background-color :mixer-strip-bg
    :selected-background-color :mixer-strip-selected-bg
    :muted-background-color :mixer-strip-muted-bg
    :border-width 2
    :corner-radius (eseq.seq-core-state/radius 16)
    :border-color :mixer-strip-border
    :selected-border-color :mixer-strip-selected-border
    :muted-border-color :mixer-strip-border
    :padding 0.45
    (v-stack :gap 0.35 :align :right
      ;; poly/voices — the same track params the *track* buffer's parameter
      ;; panel edits, surfaced here so a patch can be auditioned in mono
      ;; without leaving the patch editor.
      (subtree :key (str "patch-mixer-strip-poly-" i)
        (box :width :fill :height 3
          (h-stack :gap 0.7 :align :center
            (v-stack :align :center :gap 0.34
              (label "poly" :font-size 8 :color :dim :bg :transparent)
              (button (if SEQ.tp-poly "ON" "OFF") :width 3.2 :height 1.3
                :background-color (if SEQ.tp-poly :control-on-bg :poly-off-bg)
                :border-color :none
                :font-size 11
                :color (if SEQ.tp-poly :control-on-fg :poly-off-fg)
                ;; Rack tracks: playback polyphony is per-slot
                ;; (RackSlotSnapshot::max_polyphony), never the track-level
                ;; param — route there or this silently edits a dead value.
                :on-click |x y r| (do (eseq.seq-core-state/cool-off-follow)
                  (if SEQ.tp-is-rack
                    (host-command "set-rack-slot-max-polyphony"
                      (dict :track SEQ.current-track :slot SEQ.tp-rack-slot-idx :value (if SEQ.tp-poly 1 4)))
                    (seq-set-track-param :poly (if SEQ.tp-poly 0 1))))))
            (v-stack :align :center :gap 0.5
              (label "voices" :font-size 8 :color :dim :bg :transparent)
              (number-picker :value SEQ.tp-max-polyphony :min 1 :max 12 :decimals 0
                :noui false :font-size 8 :text-color :white
                :background-color :mixer-strip-bg
                :border-color :none
                :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow)
                    (if SEQ.tp-is-rack
                      (host-command "set-rack-slot-max-polyphony"
                        (dict :track SEQ.current-track :slot SEQ.tp-rack-slot-idx :value v))
                      (seq-set-track-param :voices v))))
                :width 3.4 :height 1.15)))))
      (h-stack
        (box :width 3.5)
        (track-meter-control i)
        )
      (subtree :key (str "patch-mixer-strip-buttons-" i)
        (h-stack
          (strip-buttons i)
          )
        )
      (subtree :key (str "patch-mixer-strip-label-" i)
        (strip-label i)))))

;; The patch mixer has no keys of its own; it takes the shared sequencer keymap.
(set-buffer-mode-for "*patch-mixer*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*patch-mixer*"
  (box :padding 0.2
    (subtree :key (str "patch-mixer-track-" SEQ.current-track)
      (patch-mixer-strip SEQ.current-track))
    (track-context-menu)))

;; The *mixer* buffer keeps its name (the host keys meter/peak liveness and
;; delete-target routing on it) but its whole body is one overridable
;; function, so a package can replace the strip-per-track view with its own
;; (e.g. a grid of compact channels) via (override eseq.mixer/mixer-body …).
(def mixer-body ()
  (h-stack :padding 0.2 :gap 1.5
    (h-stack :gap 0.5
      (each (render-order) |item|
        (render-item item)))
    (sample-drop-zone)
    (h-stack :gap 0.5
      (each (range 0 (len SEQ.bus-names)) |display-i|
        (let ((i (display-bus-index display-i)))
          (subtree :key (str "mixer-v2-bus-" i)
            (if (group-bus-id? (nth SEQ.bus-ids i))
              (box :width 0.0 :height 0.0)
              (bus-strip i)))))
      (track-context-menu)
      (subtree :key "mixer-param-plock-menu" (pc/param-plock-context-menu)))))

(effect-buffer "*mixer*"
  (mixer-body))

;; Ctrl+G / Cmd+G — fold the multi-selected tracks into a new group.
(def group-selected ()
  (do
    (host-command "group-selected-tracks" (dict))
    true))

;; Global grouping dispatcher. Multi-selection is shared across the sequencer
;; UI, so both shortcuts work from any tile that accepts global UI shortcuts.
(def seq-ctrl-g ()
  (if (>= (len SEQ.selected-tracks) 2)
    (group-selected)
    (status "Select 2+ tracks to group")))

(define-mode "seq-mixer-mode" :read-only true :live-keys true :on-key "handle-key"
  :inherit "eseq.sequencer-keys/sequencer-keys")
(mode-bind-key "seq-mixer-mode" "LEFT" "select-prev-channel")
(mode-bind-key "seq-mixer-mode" "RIGHT" "select-next-channel")
(set-buffer-mode-for "*mixer*" "seq-mixer-mode")
