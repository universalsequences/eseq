;; ui/mixer.lisp — Horizontal DAW-style mixer.
;; Renders to *mixer* buffer. Loaded by ui/main.lisp.
;;
;; Host state comes from eseq.kinds (docs/kind-bindings-spec.md): tracks,
;; buses, groups, sends, mod routes, pattern cells and rack clips are
;; instances. A field read by value (`t.name`) re-renders its subtree; a `#'`
;; binding (`#'t.volume` on a fader, `#'t.peak` on a meter, `#'t.soloed` on
;; a button) only repaints. Meters, faders, knobs, mute/solo/arm and the
;; selection highlights are all bindings, so playback and mixing never
;; re-render a strip. The menu, the renames, the selection anchor and a mod
;; cable being drawn are this view's own state: `:key ()` singletons below.
;;
;; A binding cannot be negated, so the strips light while a track is heard:
;; `:muted #'t.audible` selects the plain look, and the props without the
;; `muted-` prefix carry the silenced (muted or soloed away) look.

(module eseq.mixer)
;; Compile-time edge (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile.
(import eseq.seq-core-state)

;; Names other code calls by spelling: four positional shims
;; (`track-color-r`/`-g`/`-b`, `track-collapsed-label`; COMPAT(eseq-0l17)
;; below) take a track position for effects/track-panels.lisp, which paints
;; the track-panel header with them, and `seq-ctrl-g` is the global Ctrl+G /
;; Cmd+G dispatcher src/ui/input.rs evals by name.

(import eseq.track-collapse)
(import eseq.drum-rack-v2)
;; Read-only: the step-tab registry names the `(load …)` a script came from.
(import eseq.seq-step-tabs)
(import eseq.effects.drag-drop :as effect-dd)
(import eseq.effects.param-controls :as pc)
;; Shared scene-bank view state: the clip grid below shows only the bank the
;; transport strip is viewing (scene-banks spec 10.1).
(import eseq.scene-banks :refer (scene-viewed-bank clip-in-viewed-bank?))
(import eseq.view-kit :refer (open-menu! menu-of nothing listed? index-of prop-if))
(import eseq.kinds :refer (track tracks buses groups routes graphs selection master project
                           mod-in-level launch-cell! launch-rack-clip! save-rack-clip-as!
                           convert-rack-to-clips!))

(export render-order
        display-buses
        group-bus?
        main-bus
        track-color-r
        track-color-g
        track-color-b
        select-track
        select-track-delete-target
        drop-sample-on-track
        drop-sample-new-track
        drop-on-track
        drop-effect-on-bus
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
        select-bus
        activate-track-control
        clear-delete-target
        track-context-menu
        bus-meter
        track-meter
        pointer-volume
        mod-port-row
        bus-mod-port-row
        strip-menu
        strip-rename
        mod-patch)

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

;; ── View state ──

;; The strip context menu: where it opened and its target, a track or a
;; group (the other one nil).
(def-kind strip-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (track track :default nil)
          (group group :default nil)))

;; The inline rename in progress: the track or group being renamed (nil when
;; none is) and the draft.
(def-kind strip-rename
  :key ()
  :state ((track track :default nil)
          (group group :default nil)
          (draft "")))

;; The stable end of a Finder-style range selection (shift-click). Cmd-click
;; leaves it alone; a plain selection starts a new range.
(def-kind strip-select
  :key ()
  :state ((anchor track :default nil)))

;; A mod cable being drawn: the track whose OUT port was pressed, nil when
;; none is.
(def-kind mod-patch
  :key ()
  :state ((source track :default nil)))

;; Data, rather than menu-specific control flow, is the extension seam for
;; Duplicate/Delete/Group/color actions added later.
(def track-menu-actions
  (list (dict :id :rename :label "Rename" :icon :pencil)))

(def group-menu-actions
  (list
    (dict :id :rename :label "Rename" :icon :pencil)
    (dict :id :convert-drum-rack :label "Convert to Drum Rack" :icon :sampler)
    (dict :id :ungroup :label "Ungroup" :icon :ungroup)))

;; A rack's menu also offers the graph sequencers it could own. A sequencer
;; that merely routes to tracks inside the rack is still project-owned;
;; "Attach … to rack" hands it to the rack (routes become rack members, and
;; the pair travels together into kits), and "Detach …" gives it back
;; (docs/rack-clips-and-break-kits-spec.md §5.1).
;; An instance's sequencer shows the instance's label; a legacy script its
;; published name.
(def graph-sequencer-label (gr)
  (let ((instance (instance-ref gr.gid)))
    (if instance instance.label gr.name)))

(def rack-group-menu-actions (g)
  (append
    (append
      (list
        (dict :id :rename :label "Rename" :icon :pencil))
      ;; Rack clips (docs/rack-clips-and-break-kits-spec.md §6.3). A LEGACY
      ;; rack (no bank) is offered the one-shot conversion; a rack that already
      ;; has clips saves a new one here and deletes from the row's [-] button.
      (if (has-clips? g)
        (list (dict :id :save-rack-clip :label "Save clip as..." :icon :save))
        (list (dict :id :convert-to-clips :label "Convert to clips" :icon :clips))))
    (append
      ;; The kind picker (docs/instance-kinds-spec.md §8.3): a new instance
      ;; of any kind, owned by this rack. The host attaches the kind's
      ;; package to the project first when it is not yet.
      (map
        (lambda (kind)
          (dict :id :new-rack-instance
                :key (str "new-" (get kind :id))
                :kind-id (get kind :id)
                :module (get kind :module)
                :label (str "New " (get kind :name) " in rack")
                :icon :plus))
        (seq-instance-kinds))
      (map
        (lambda (gr)
          (if (= gr.owner g)
            (dict :id :detach-sequencer
                  :key (str "detach-" gr.gid)
                  :sequencer-id gr.gid
                  :sequencer-name gr.name
                  :label (str "Detach " (graph-sequencer-label gr))
                  :icon :unlink)
            (dict :id :move-sequencer-into-rack
                  :key (str "attach-" gr.gid)
                  :sequencer-id gr.gid
                  :sequencer-name gr.name
                  :label (str "Attach " (graph-sequencer-label gr) " to rack")
                  :icon :link)))
        (filter (lambda (gr) (or (= gr.owner nil) (= gr.owner g))) (graphs))))
    (list
      ;; Break kits (docs/rack-clips-and-break-kits-spec.md 7.2): the kit save
      ;; panel, which carries a scene checklist for the clip bank.
      (dict :id :export-kit :label "Export as kit..." :icon :export)
      (dict :id :ungroup :label "Ungroup" :icon :ungroup))))

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

;; ── Colors ──

;; A component (0 r, 1 g, 2 b) of an :rgb value `(rgb r g b)`.
(def rgb-part (c i) (nth c (+ i 1)))

;; Muted strips pull component i (0 r, 1 g, 2 b) of their color toward a
;; dark gray.
(def dimmed (v i muted)
  (if muted (+ (* v 0.34) (* (if (= i 2) 0.11 0.10) 0.66)) v))

(def color-part (c i muted) (dimmed (rgb-part c i) i muted))

;; Color c as an rgba with `alpha`.
(def color-rgba (c alpha)
  (rgba (rgb-part c 0) (rgb-part c 1) (rgb-part c 2) alpha))

(def track-rgba (t muted alpha)
  (let ((c t.color))
    (rgba (color-part c 0 muted) (color-part c 1 muted) (color-part c 2 muted) alpha)))

;; COMPAT(eseq-0l17): positional shims. Track i's color components, dimmed
;; when muted (the track panel header paints itself with them;
;; effects/track-panels.lisp); a stock blue when there is no track i.
(def track-color-part (i muted part)
  (let ((t (track i)))
    (if t
      (color-part t.color part muted)
      (dimmed (nth (list 0.34 0.48 0.98) part) part muted))))
(def track-color-r (i muted) (track-color-part i muted 0))
(def track-color-g (i muted) (track-color-part i muted 1))
(def track-color-b (i muted) (track-color-part i muted 2))

(def arm-color (rgba 0.95 0.20 0.18 1.0))

(def pointer-volume (sy)
  (max 0.0 (min 1.0 (* 0.5 (- 1.0 sy)))))

(def event-volume (event)
  (pointer-volume event.sy))

;; ── Selection ──

(def clear-delete-target ()
  (seq-clear-delete-target))

(def reveal-track (t)
  (host-command "reveal-sequencer-track" (dict :track t.index)))

(def select-track (t)
  (set! strip-select.anchor t)
  (set! eseq.seq-core-state/selected-bus -1)
  (clear-delete-target)
  (set! selection.track t)
  (reveal-track t))

;; The label's plain click also arms the track as the delete target (alone).
(def select-track-delete-target (t)
  (select-track t)
  (seq-set-delete-target :mixer-track (dict :track t.index)))

(def activate-track-control ()
  (set! eseq.seq-core-state/selected-bus -1)
  (clear-delete-target))

;; The platform selection gesture toggles membership: Command-click on macOS,
;; Alt-click on Linux. Rust supplies the semantic field so this behavior does
;; not depend on window-manager ownership of Super.
(def multi-select-click? (event)
  event.additive-selection)

(def toggle-track-select (t)
  (set! eseq.seq-core-state/selected-bus -1)
  (seq-toggle-track-selected t.index)
  (reveal-track t))

;; Track indices from `anchor` to `target` in the mixer's visible order.
(def visual-track-range (anchor target)
  (eseq.seq-core-state/track-range-in-order
    (eseq.drum-rack-v2/mixer-visible-track-order) anchor target))

;; A held anchor whose track is gone falls back to the current track.
(def range-track-select (t)
  (let ((held strip-select.anchor)
        (anchor (if (listed? held (tracks)) held (or selection.track t))))
    (set! strip-select.anchor anchor)
    (set! eseq.seq-core-state/selected-bus -1)
    (seq-select-tracks (visual-track-range anchor.index t.index) t.index)
    (reveal-track t)))

;; Shift-click = replace with the anchored range; cmd-click = toggle
;; membership without moving the range anchor; a plain click calls `plain`
;; on t (`select-track` on a strip; on its label `select-track-delete-target`,
;; which also arms the delete target that shift- and cmd-clicks clear).
(def track-click (event t plain)
  (if event.shift
    (range-track-select t)
    (if (multi-select-click? event)
      (toggle-track-select t)
      (plain t))))

(def select-bus (b)
  (seq-clear-selection)
  (seq-clear-delete-target)
  (set! eseq.seq-core-state/selected-bus b.index))

;; ── Drops ──
;; Drop events carry the host's drop meta: a track or bus by index, the
;; address the add/drop host commands take.

(def drop-sample-on-track (event)
  (clear-delete-target)
  (if event.payload.path
    (eseq.browser/drop-sample-on-track event)
    (status "Drop a sample file, not a folder")))

(def drop-sample-new-track (event)
  (if (= event.drag-type "track-badge")
    (drop-track-out-of-group event)
    (let ((payload event.payload)
          (path payload.path))
      (clear-delete-target)
      (match event.drag-type
        "sound"
        (if path
          (host-command "add-track-from-sound" (dict :path path))
          (status "Drop a Sound item, not a folder"))
        "instrument" (eseq.browser/drop-instrument-new-track payload)
        "instrument-preset" (eseq.browser/drop-preset-new-track payload nil)
        _
        (if path
          (host-command "add-track-sample" (dict :path path :preserve-browser-context true))
          (status "Drop a sample file, not a folder"))))))

;; Drag a track badge onto a group container -> add it to that group.
(def drop-track-into-group (event g)
  (let ((trk event.payload.track))
    (clear-delete-target)
    (if (>= trk 0)
      (host-command "move-track-to-group" (dict :track trk :gidx g.index))
      false)))

;; Drag a track badge onto the "Drop samples here" zone -> remove it from its group.
(def drop-track-out-of-group (event)
  (let ((trk event.payload.track))
    (clear-delete-target)
    (if (>= trk 0)
      (host-command "remove-track-from-group" (dict :track trk))
      false)))

(def drop-effect-on-track (event)
  (let ((kind event.payload.kind)
        (name event.payload.name)
        (i event.target.track)
        (t (track i)))
    (when t (select-track t))
    (match kind
      "builtin-audio-effect" (host-command "add-builtin-effect-to-track" (dict :track i :name name))
      "custom-audio-effect" (host-command "add-effect-to-track" (dict :track i :name name))
      "midi-effect" (host-command "add-midi-fx-to-track" (dict :track i :name name))
      _ (status "Drop an audio or MIDI effect"))))

(def drop-on-track (event)
  (match event.drag-type
    "sound" (eseq.browser/drop-sound-on-track event)
    "instrument" (eseq.browser/drop-sound-on-track event)
    "instrument-preset" (eseq.browser/drop-sound-on-track event)
    "sample" (drop-sample-on-track event)
    "effect-instance"
    (effect-dd/drop-existing-effect event.payload
      (dict :chain "append" :track event.target.track :bus -1 :slot -1))
    "audio-effect" (drop-effect-on-track event)
    "midi-effect" (drop-effect-on-track event)
    _ (status "Unsupported drop")))

(def track-drop-types (t)
  (if (eseq.track-collapse/replaceable-type? t.instrument-type)
    (list "sample" "instrument" "instrument-preset" "sound" "audio-effect" "midi-effect"
      "effect-instance")
    (list "sample" "audio-effect" "midi-effect" "effect-instance")))

(def drop-effect-on-bus (event)
  (let ((kind event.payload.kind)
        (name event.payload.name)
        (i event.target.bus)
        (b (nth (buses) i)))
    (when b (select-bus b))
    (match kind
      "builtin-audio-effect" (host-command "add-builtin-bus-effect" (dict :bus i :name name))
      "custom-audio-effect" (host-command "add-bus-effect" (dict :bus i :name name))
      _ (status "Drop an audio effect on a bus"))))

;; ── Mod routes ──
;; Port props speak the host's patch-cable addresses: track positions, bus
;; ids and inputs 0-3 (route.input is 1-4).

(def track-mod-output? (t)
  (or (= t.instrument-type "modulator") t.mod-output))

(def route-into? (r dest input)
  (and (= r.input (+ input 1))
       (if r.dest (= r.dest dest) (= r.dest-bus dest))))

;; Source track positions of the routes into `dest` (a track or a bus) at
;; `input`; only the selected ones with `selected`.
(def route-sources (dest input selected)
  (map (lambda (r) r.source.index)
    (filter (lambda (r) (and (route-into? r dest input) (or (not selected) r.selected)))
      (routes))))

;; The route between track positions / a bus id, if any.
(def route-at (source dest-kind dest input)
  (first
    (filter
      (lambda (r)
        (and (= r.source.index source)
             (= r.input (+ input 1))
             (if (= dest-kind "bus")
               (and r.dest-bus (= r.dest-bus.bid dest))
               (and r.dest (= r.dest.index dest)))))
      (routes))))

;; Only a connected OUT port lights with its signal.
(def track-mod-out-connected? (t)
  (> (len (filter (lambda (r) (= r.source t)) (routes))) 0))

(def mod-out-level (t)
  (if (track-mod-out-connected? t) #'t.mod-out-level 0))

(def select-mod-route (source dest-kind dest input)
  (let ((r (route-at source dest-kind dest input)))
    (when r
      (set! r.selected true)
      (status (if (= dest-kind "bus")
        (str "Selected mod route: track " (+ source 1) " out -> group Ext" (+ input 1))
        (str "Selected mod route: track " (+ source 1) " out -> track " (+ dest 1) " Ext" (+ input 1)))))))

(def mod-out-click (t)
  (clear-delete-target)
  (if (track-mod-output? t)
    (do
      (set! mod-patch.source t)
      (status (str "Mod out: track " (+ t.index 1))))
    (status "This track has no mod output")))

(def cancel-mod-draw ()
  (set! mod-patch.source nil)
  true)

(def connect-mod-route (source dest-kind dest input)
  (clear-delete-target)
  (if (and (= dest-kind "track") (= source dest))
    (status "Mod self-routes are not allowed")
    (if (route-at source dest-kind dest input)
      (status "Mod route already connected")
      (host-command "set-mod-route"
        (dict :source source :dest-kind dest-kind :dest dest :input input)))))

;; Release over an input port while a cable is drawn.
(def mod-in-click (dest-kind dest input)
  (let ((source mod-patch.source))
    (if (and source (listed? source (tracks)))
      (do
        (connect-mod-route source.index dest-kind dest input)
        (set! mod-patch.source nil))
      false)))

(defwidget mixer-v2-mod-port
  :width 1.55 :height 1.55
  :paint-margin 0.12
  :state (active pending output selected level)
  :shader
  ;; `level` is the live mod signal at this port (0..1, bound to the port's
  ;; kind field at meter rate). It lights the port like a VCV Rack jack LED:
  ;; the dark centre fills with the ring colour and the ring itself
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

;; ── Clip grid ──

;; Launch cell c on its track, with the transport's launch quantization:
;; the host assigns the cell now and defers the audible launch to the
;; boundary. The cell becomes the delete target at once, so BS or clone
;; right after the click act on it.
(def launch-track-pattern (c)
  (activate-track-control)
  (set! selection.track c.track)
  (launch-cell! c)
  (set! c.selected true))

;; Cells of `t` in the viewed scene bank (spec 10.1). A bank holds at most
;; 24 scenes, so at most 24 referenced clips per track — the grid's 6x4
;; capacity — plus any orphan clips no scene references yet.
(def viewed-bank-track-pattern-cells (t)
  (let ((viewed (scene-viewed-bank)))
    (filter (lambda (c) (clip-in-viewed-bank? c viewed)) t.cells)))

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

;; Its own subtree: a launch, a clone or a bank switch re-renders the grid
;; alone.
(def track-pattern-grid (t)
  (subtree :key (str "mixer-v2-track-pattern-grid-" t.index)
    (track-pattern-cell-grid t)))

(def track-pattern-cell-grid (t)
  (let ((cells (viewed-bank-track-pattern-cells t))
        (k (clip-cell-scale))
        (c t.color))
    (box :width :fill :height (clip-area-height) :align :top :bg :black :background-color :buffer-bg
      (scroll
      (grid :cols 6 :col-width (* 2.0 k) :row-height (* 1.0 k) :align :center
        (each cells |cell|
          (box
            :key (str "track-pattern-cell-" t.index "-" cell.pid)
            :drag-type (str "track-pattern-" t.tid)
            :drag-modifier :none
            :drag-payload (dict :track t.index :track-id t.tid :pattern-id cell.pid)
            :width (* 1.90 k) :height (* 0.95 k)
            :padding (* 0.35 k)
            :bg :transparent
            :background (if cell.queued
              "track-pattern-cell-queued-bg"
              "track-pattern-cell-bg")
            :active #'cell.active
            :assigned #'cell.assigned
            :override #'cell.override
            :selected #'cell.selected
            ;; Dimmed so the sound glyph on top carries the cell's identity;
            ;; the launch/selection states still read through the shader.
            :track-r (* 0.65 (rgb-part c 0))
            :track-g (* 0.65 (rgb-part c 1))
            :track-b (* 0.65 (rgb-part c 2))
            :on-click (lambda (event) (launch-track-pattern cell))
            ;; The pattern's bound sound, as its palette glyph (host feed:
            ;; sync_pattern_cell_glyph_frames). The tuned shader styling is
            ;; the widget default (TUNING_PROPS); the substrate body tints
            ;; with the track color so cells keep their track identity.
            (sound-glyph
              :key (str "cell-glyph-" t.index "-" cell.pid)
              :source (str "pattern-glyph:track:" t.index ":pattern:" cell.pid)
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
              :tint-r (* 0.1 (rgb-part c 0))
              :tint-g (* 0.1 (rgb-part c 1))
              :tint-b (* 0.1 (rgb-part c 2))))))))))

;; ── Mod ports ──

(def mod-output-style
  (ui/style
    :hover (dict
      :brightness 1.45
      :transition (dict :brightness 0.08 :ease :smoothstep))))

;; An input port of `dest` (a track or a bus) at `input` (0-3). `props` are
;; the host's patch address for the port.
(def mod-in-port (dest dest-kind address input &rest props)
  (let ((selected (route-sources dest input true)))
    (apply mixer-v2-mod-port
      (append props
        (list :patch-port true
              :direction :in
              :input input
              :connected-sources (route-sources dest input false)
              :selected-sources selected
              :active true
              :pending false
              :output false
              :selected (> (len selected) 0)
              :level (mod-in-level dest (+ input 1))
              :on-patch-drop (lambda (source to input)
                (connect-mod-route source dest-kind address input)
                (set! mod-patch.source nil))
              :on-cable-click (lambda (source to input)
                (select-mod-route source dest-kind address input))
              :on-click |x y r| (mod-in-click dest-kind address input)
              :on-mouse-up |x y r| (mod-in-click dest-kind address input))))))

;; The port rows are their own subtrees: a route patched or a cable drawn
;; re-renders the rows alone.
(def mod-port-row (t)
  (subtree :key (str "mixer-v2-mod-port-row-" t.index)
    (track-mod-ports t)))

(def track-mod-ports (t)
  (box :height 0.8 :width :fill
    (h-stack :key (str "mod-ports-" t.index)
      :width 9.8 :height 0.1 :gap 0.42 :align :center
      (mixer-v2-mod-port
        :key (str "mod-out-" t.index)
        :patch-port true
        :direction :out
        :track t.index
        :active (track-mod-output? t)
        :pending (= mod-patch.source t)
        :output true
        :selected false
        :level (mod-out-level t)
        :style (if (track-mod-output? t) mod-output-style nil)
        :on-click |x y r| (mod-out-click t)
        :on-mouse-down |x y r| (mod-out-click t)
        :on-patch-cancel (lambda (source) (cancel-mod-draw))
        :on-patch-miss (lambda () (clear-delete-target)))
      (if (= t.instrument-type "modulator")
        (each (range 0 4) |input|
          (box :key (str "mod-in-spacer-" t.index "-" input)
            :width 1.05 :height 1.05))
        (each (range 0 4) |input|
          (mod-in-port t "track" t.index input
            :key (str "mod-in-" t.index "-" input)
            :track t.index))))))

(def bus-mod-port-row (b)
  (subtree :key (str "mixer-v2-bus-mod-port-row-" b.bid)
    (bus-mod-ports b)))

(def bus-mod-ports (b)
  (box :height 0.8 :width :fill
    (h-stack :key (str "bus-mod-ports-" b.bid)
      :width 7.1 :height 0.1 :gap 0.42 :align :center
      (each (range 0 4) |input|
        (mod-in-port b "bus" b.bid input
          :key (str "bus-mod-in-" b.bid "-" input)
          :dest-kind "bus"
          :dest b.bid)))))

;; ── Faders and meters ──

(defwidget mixer-v2-volume-triangle
  :width 1.35 :height 4.24
  :paint-margin 0.15
  :state (value)
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

(def meter-display (level-l level-r)
  (mixer-meter
    :level-l level-l :level-r level-r
    :width 2.22 :height 4.24
    :font-size 7 :label-height 0.42 :label-top-inset 0.0
    :label-color :dim))

(def track-meter (t)
  (subtree :key (str "mixer-v2-track-meter-" t.index)
    (meter-display #'t.peak #'t.peak)))

;; The main mix (the graph output, bus id 0) meters the master output.
(def main-bus? (b) (= b.bid 0))

(def bus-meter (b)
  (subtree :key (str "mixer-v2-bus-meter-" b.index)
    (if (main-bus? b)
      (meter-display #'master.peak-l #'master.peak-r)
      (meter-display #'b.peak #'b.peak))))

;; A fader `value` binding; `set` takes the pointer's level.
(def volume-fader (value set)
  (mixer-v2-volume-triangle
    :value value
    :on-click (lambda (sx sy region) (set (pointer-volume sy)))
    :on-drag (lambda (sx sy region) (set (pointer-volume sy)))))

(def set-track-volume! (t v)
  (clear-delete-target)
  (set! t.volume v))

(def track-meter-control (t)
  (box :width 3.65 :height 4.24
    :on-click (lambda (event) (set-track-volume! t (event-volume event)))
    :on-drag (lambda (event) (set-track-volume! t (event-volume event)))
    (h-stack :gap 0.06 :align :center
      (volume-fader #'t.volume (lambda (v) (set-track-volume! t v)))
      (track-meter t))))

;; `spacer` is the room left above the meter: the clip-area equivalent that
;; keeps a bus/group meter level with the track strips' meters, or 0 when the
;; caller already filled that room (a rack's clip column, mixer §6.2).
(def bus-meter-control-with-spacer (b is-group spacer)
  (box :width 3.65 :height 4.24
    :on-click (lambda (event) (select-bus b))
    :on-drag (lambda (event) (select-bus b))
    (v-stack
      ;; Level with the track strips' meters, which sit below the clip area.
      (box :width :fill :height spacer)
      (h-stack :gap 0.06
        (box :width (if is-group 6 2))
        (volume-fader #'b.volume
          (lambda (v)
            (select-bus b)
            (set! b.volume v)))
        (bus-meter b)))))

(def bus-meter-control (b is-group)
  (bus-meter-control-with-spacer b is-group (if (compact?) 0.2 4.2)))

;; ── Track strips ──

(def send-label (name)
  (if (= name "Bus A")
    "A"
    (if (= name "Bus B")
      "B"
      (substring name 0 3))))

;; A send as the p-lock menu and the process mapping address it.
(def send-target (s)
  (dict :bus-id s.bus.bid :bus-idx s.bus.index :name s.bus.name))

;; The knob shows the displayed level (a p-lock's at the selected or playing
;; step); turning it is the legacy send edit, which p-locks the selected
;; steps of the current track and sets the base elsewhere.
(def send-knob (t s)
  (let ((bus-idx s.bus.index)
        (key (str "track-" t.index "-send-" bus-idx))
        (has-locks s.has-locks)
        (target (dict :track t.index :target "bus-send" :param-idx bus-idx)))
    (box :debug-name (str key "-plock")
      :plock-any (if has-locks 1 0)
      :on-right-click (lambda (event) (pc/open-host-plock-menu event "mixer" target has-locks))
      (pc/process-send-map-wrapper t.index (send-target s) key
      (knob-number :label (send-label s.bus.name)
        :key key
        :value #'s.display
        :plock-active #'s.locked
        :plock-default #'s.amount
        :plock-color-r (pc/param-plock-color-r)
        :plock-color-g (pc/param-plock-color-g)
        :plock-color-b (pc/param-plock-color-b)
        ;; A process OUT port writing this send: its last value as the
        ;; amber knob dot (same read-only overlay as instrument params).
        :process-value (if s.process-mapped #'s.process-value false)
        :min 0 :max 1 :decimals 2
        :show-value false
        :font-size 9 :label-font-size 5
        :text-color :dim :label-color :dim
        :width 3.4 :height 2.0 :knob-size 1.44
        :on-change (lambda (v)
          (clear-delete-target)
          (host-command "set-track-bus-send"
            (dict :track t.index :bus bus-idx :amount v))))))))

;; ── Outputs ──

(def main-bus () (first (filter main-bus? (buses))))

;; The output dropdown's labels: "main" for the main mix, "sends only" for
;; none, else the bus name.
(def track-output-label (b)
  (if b (if (main-bus? b) "main" b.name) "sends only"))

(def track-output-options ()
  (append (list "main" "sends only")
    (map track-output-label
      (filter (lambda (b) (not (main-bus? b))) (project-output-options)))))

(def project-output-options () project.output-options)

(def track-output-of (label)
  (if (= label "sends only")
    nil
    (first (filter (lambda (b) (= (track-output-label b) label)) (project-output-options)))))

(def track-output-dropdown (t)
  (subtree :key (str "mixer-v2-track-output-sub-" t.index)
    (dropdown :value (track-output-label t.output)
      :key (str "track-output-" t.index)
      :options (track-output-options)
      :on-change (lambda (v)
        (clear-delete-target)
        (let ((b (track-output-of v)))
          (when (or b (= v "sends only")) (set! t.output b))))
      :width :fill :height 1.2 :font-size 10
      :corner-radius (eseq.seq-core-state/radius 20))))

;; The bus names more than one bus has (read once per dropdown).
(def shared-bus-names ()
  (let ((names (map (lambda (x) x.name) (buses))))
    (filter (lambda (n) (> (len (filter (lambda (m) (= m n)) names)) 1)) names)))

;; A bus as its output dropdown names it: "main" for the main mix, and the
;; id after a name that is "main" or in `shared` (shared-bus-names).
(def bus-output-label (b shared)
  (if (main-bus? b)
    "main"
    (if (or (= b.name "main") (listed? b.name shared))
      (str b.name " (" b.bid ")")
      b.name)))

(def bus-output-dropdown (b)
  (subtree :key (str "mixer-bus-output-" (if b b.index -1))
    (let ((options (if b b.output-options (list)))
          (shared (if (> (len options) 0) (shared-bus-names) (list)))
          (label-of (lambda (x) (bus-output-label x shared))))
      (if (> (len options) 0)
        (dropdown :key (str "bus-output-" b.index)
          :value (if b.output (label-of b.output) "main")
          :options (map label-of options)
          :width :fill :height 1.2 :font-size 10
          :corner-radius (eseq.seq-core-state/radius 20)
          :on-change (lambda (label)
            (let ((dest (first (filter (lambda (o) (= (label-of o) label)) options))))
              (when dest (set! b.output dest)))))
        (box :height 1.2 :width :fill)))))

;; A track's strip frame, `width` by `height`: lit while the track is in
;; the selection, in the silenced look while it is not heard. `props` are
;; more props (a list), `body` the content.
(def track-frame (t width height border radius props &rest body)
  (apply box
    (append
      (list :width width :height height
            :selected #'t.in-selection
            :muted #'t.audible
            :background-color :mixer-strip-muted-bg
            :selected-background-color :mixer-strip-selected-bg
            :muted-background-color :mixer-strip-bg
            :border-width border
            :corner-radius (eseq.seq-core-state/radius radius)
            :border-color :mixer-strip-border
            :selected-border-color :mixer-strip-selected-border
            :muted-border-color :mixer-strip-border
            :padding 0.45)
      props
      body)))

;; A mixer strip's drop target and pointer gestures (full or collapsed).
(def track-strip-props (t)
  (list :drop-hover-border-color :mixer-strip-selected-border
        :drop-types (track-drop-types t)
        :drop-meta (dict :kind "track" :track t.index)
        :on-drop (lambda (event) (drop-on-track event))
        :on-click (lambda (event) (track-click event t select-track))
        :on-right-click (lambda (event) (open-strip-menu event t nil))))

;; A track strip: the output, the clip grid, sends, pan, the fader and meter,
;; the buttons, the mod ports and the label. Everything that changes while
;; mixing or playing is a binding, so only the label, the clip grid and the
;; mod ports ever re-render (a rename, a launch, a patch).
(def track-strip (t)
  ;; Grouped strips drop the output dropdown, so they are shorter to avoid
  ;; dead space at the bottom inside the group container.
  (track-frame t track-strip-width (if t.group (grouped-strip-height) (strip-height)) 4 16
    (track-strip-props t)
    (v-stack :gap 0.18 :width :fill
      ;; Grouped tracks drop the output dropdown (their output is the group
      ;; bus); the container provides the color above. A small spacer keeps
      ;; the pattern grid aligned with loose strips.
      (if t.group nil (track-output-dropdown t))
      (if (compact?) nil (track-pattern-grid t))

      (box :width :fill
      (h-stack :gap 0 :align :center :width :fill
        (v-stack
          (h-stack :gap 0.05
            ;; Stable identities keep only Bus A / Bus B here even when
            ;; other buses are created or the bus list is reordered.
            (each (filter (lambda (s) (or (= s.bus.bid 1) (= s.bus.bid 2))) t.sends) |s|
              (send-knob t s)))
          (knob-number :label "pan"
            :key (str "track-pan-" t.index)
            :value #'t.pan
            :min -1 :max 1 :origin 0 :decimals 2
            :font-size 9 :label-font-size 8
            :text-color :dim :label-color :dim
            :width 6.5 :height 2.35 :knob-size 2.58
            :on-change (lambda (v)
              (clear-delete-target)
              (set! t.pan v))))
        (box :flex 1 :height 1)
        (track-meter-control t)))

      (box :width :fill :height 0.05)
      (subtree :key (str "mixer-v2-strip-buttons-" t.index)
        (strip-buttons t))
      (mod-port-row t)
      (subtree :key (str "mixer-v2-strip-label-" t.index)
        (strip-label t)))))

;; A strip button: `active` binds its lit state; `bg` and `idle-color` are
;; its unlit look, `active-bg` and `color` its lit one.
(def strip-button (text active &key (key nil) (width 2.1) (color :control-on-fg)
                   (active-bg :control-on-bg) (bg :mixer-control-bg) (idle-color :dim)
                   (on-click nil))
  (apply button text
    (append (prop-if :key key)
      (list :width width :height 1.0 :padding 0 :font-size 10
            :border-color :transparent
            :active active
            :background-color bg
            :active-background-color active-bg
            :color idle-color
            :active-color color
            :on-click on-click))))

;; Mute, solo and arm only repaint their bound states. The number lights
;; while the track is heard.
(def strip-buttons (t)
  (h-stack :gap 0.35
    (strip-button (str (+ t.index 1)) #'t.audible
      :on-click (lambda (event) (activate-track-control) (toggle! t.muted)))
    (strip-button "S" #'t.soloed
      :on-click (lambda (event) (activate-track-control) (toggle! t.soloed)))
    (strip-button "R" #'t.armed :color :black :active-bg arm-color
      :on-click (lambda (event) (activate-track-control) (toggle! t.armed)))))

;; ── Menus and renames ──

;; The strip menu of track t or group g (the other one nil).
(def open-strip-menu (event t g)
  (set! strip-menu.track t)
  (set! strip-menu.group g)
  (open-menu! strip-menu event))

;; End the rename of track t or group g (the other one nil), when it is the
;; one in progress; `commit` sends the draft to a target still listed.
(def finish-rename (t g commit)
  (when (and (= strip-rename.track t) (= strip-rename.group g))
    (when commit
      (if g
        (when (listed? g (groups))
          (host-command "rename-group" (dict :group-id g.gid :name strip-rename.draft)))
        (when (listed? t (tracks))
          (host-command "rename-track" (dict :track t.index :name strip-rename.draft)))))
    (set! strip-rename.track nil)
    (set! strip-rename.group nil)
    (set! strip-rename.draft "")))

;; Rename track t or group g (the other one nil). A rename in progress
;; commits first: its input only blurs after this rebuild, too late for the
;; draft it held.
(def begin-rename (t g)
  (set! strip-menu.open false)
  (when (or strip-rename.track strip-rename.group)
    (finish-rename strip-rename.track strip-rename.group true))
  (set! strip-rename.draft (if g g.name t.name))
  (set! strip-rename.track t)
  (set! strip-rename.group g))

(def track-context-menu-actions ()
  (let ((g strip-menu.group)
        (t strip-menu.track))
    (if g
      (if (listed? g (groups))
        (if g.rack (rack-group-menu-actions g) group-menu-actions)
        (list))
      (if (and t (listed? t (tracks)))
        (append
          track-menu-actions
          (if t.group
            (list (dict :id :ungroup-track :label "Ungroup track" :icon :ungroup))
            (list))
          (if (and (>= (len selection.tracks) 2) (listed? t selection.tracks))
            (list (dict :id :group :label "Group Tracks" :icon :folder))
            (list)))
        (list)))))

;; Every action closes the menu; the target is checked to be still listed
;; when the menu built the action list.
(def select-track-menu-action (action)
  (let ((g strip-menu.group)
        (t strip-menu.track))
    (set! strip-menu.open false)
    (match (get action :id)
      :rename (begin-rename t g)
      :convert-drum-rack (host-command "convert-group-to-drum-rack" (dict :group-id g.gid))
      :group (group-selected)
      :ungroup-track (host-command "remove-track-from-group" (dict :track t.index))
      :ungroup (host-command "ungroup-tracks" (dict :group-id g.gid))
      :export-kit (eseq.browser/enter-kit-save g.gid g.name)
      :new-rack-instance
      (host-command "packages-new-instance"
        (dict :module (get action :module) :kind (get action :kind-id) :group-id g.gid))
      :move-sequencer-into-rack
      (host-command "move-sequencer-into-rack"
        (dict :group-id g.gid
              :sequencer-id (get action :sequencer-id)
              :source-path (sequencer-source-path (get action :sequencer-name))))
      :detach-sequencer
      (host-command "detach-rack-sequencer"
        (dict :group-id g.gid :sequencer-id (get action :sequencer-id)))
      :convert-to-clips (convert-rack-to-clips! g)
      :save-rack-clip (save-rack-clip-as! g "")
      _ nil)))

(def track-context-menu ()
  ;; Build the items only while the menu is open: the rack branch asks the
  ;; host for its kind list (`seq-instance-kinds`), which a closed menu
  ;; must not pay for (or depend on) on every mixer render.
  (apply menu-of strip-menu
    (each (if strip-menu.open (track-context-menu-actions) (list)) |action|
      (menu-item (get action :label)
        :key (str "track-menu-" (get action :id) (or (get action :key) ""))
        :icon (get action :icon)
        :on-select (lambda (event) (select-track-menu-action action))))))

(def rename-input (key width font-size on-done)
  (text-input
    :key key
    :width width :height 1.0 :font-size font-size
    :value strip-rename.draft
    :auto-focus true
    :select-all-on-focus true
    :on-change (lambda (name) (set! strip-rename.draft name))
    :on-submit (lambda () (on-done true))
    :on-cancel (lambda () (on-done false))
    :on-blur (lambda () (on-done true))))

(def track-rename-input (t key-prefix width font-size)
  (rename-input (str key-prefix t.index) width font-size
    (lambda (commit) (finish-rename t nil commit))))

(def group-rename-input (g)
  (rename-input (str "group-rename-input-" g.gid) 6.8 9
    (lambda (commit) (finish-rename nil g commit))))

;; The track's name badge over its color; it lights white as the delete
;; target and dims while the track is silent.
;; The props of text drawn straight on its parent: no fill, border or
;; shading of its own.
(def bare-text
  (list :background-color :transparent :border-color :transparent
        :highlight-color :transparent :shadow-color :transparent :bg :transparent))

(def track-badge (t text key &key (width 3.65) (font-size 9) (h-align :center)
                  (v-align nil))
  (apply badge text
    (append
      (prop-if :v-align v-align)
      (list :key key
            :icon (eseq.track-collapse/instrument-icon t.instrument-type)
            :width width
            :height 1.0
            :padding 0
            :font-size font-size
            :h-align h-align
            :muted #'t.audible
            :color :dim
            :muted-color :black
            :active #'t.delete-target
            :active-color :white)
      bare-text)))

;; The label box: the track color, dimmed while silent, the delete target's
;; color while it is one.
(def track-label-box (t key width &rest body)
  (apply box
    (append
      (list :key key
            :width width :height 1.0
            :padding 0
            :selected #'t.delete-target
            :muted #'t.audible
            :background-color (track-rgba t true 1.0)
            :muted-background-color (track-rgba t false 1.0)
            :selected-background-color :fx-panel-header-selected-bg
            :on-click (lambda (event) (track-click event t select-track-delete-target))
            :on-double-click (lambda (event) (eseq.sequencer/open-piano-roll-for-track t.index)))
      body)))

;; Renames rebuild the label; mute/solo and selection only repaint bindings.
(def strip-label (t)
  (track-label-box t (str "track-label-" t.index) :fill
    :corner-radius (eseq.seq-core-state/radius 30)
    :drag-type "track-badge"
    :drag-payload (dict :track t.index)
    (if (= strip-rename.track t)
      (track-rename-input t "track-rename-input-" (* 9.8 (clip-cell-scale)) 10)
      (track-badge t (substring t.name 0 (name-chars 12)) (str "track-label-content-" t.index)
        :width (* 9.8 (clip-cell-scale)) :font-size 10 :h-align :left :v-align :center))))

;; COMPAT(eseq-0l17): positional shim (effects/track-panels.lisp).
(def track-collapsed-label (i)
  (let ((t (track i)))
    (str (+ i 1) " " (substring (if t t.name "") 0 3))))

(def track-collapsed-strip (t)
  (track-frame t 4.7 (collapsed-strip-height) 2 10 (track-strip-props t)
    (v-stack :gap 0.42 :align :center
      ;; Spacer absorbs the clip-area growth so the meter stays level with
      ;; the full strips' meters.
      (box :width :fill :height (max 0 (+ 3.45 (- (clip-area-height) 4.0))) :bg :transparent)
      (track-meter-control t)
      (strip-button "M" #'t.muted :key (str "track-collapsed-mute-" t.index) :width 3.65
        :on-click (lambda (event) (activate-track-control) (toggle! t.muted)))
      (track-label-box t (str "track-collapsed-label-" t.index) 3.65
        (if (= strip-rename.track t)
          (track-rename-input t "track-collapsed-rename-input-" 3.65 9)
          (track-badge t (track-collapsed-label t.index)
            (str "track-collapsed-label-content-" t.index)))))))

;; ── Buses ──

(def bus-label (b)
  (if (main-bus? b) "Main" b.name))

(def bus-mute-label (b)
  (if (main-bus? b)
    "M"
    (match b.index
      1 "A"
      2 "B"
      _ (str b.index))))

;; The buses in display order: the main mix last.
(def display-buses ()
  (let ((all (buses)))
    (append (filter (lambda (b) (not (main-bus? b))) all) (filter main-bus? all))))

;; The tracks, then the buses, as LEFT / RIGHT walk them.
(def channels ()
  (append (tracks) (display-buses)))

(def current-channel ()
  (let ((bi eseq.seq-core-state/selected-bus)
        (all (buses)))
    (if (and (>= bi 0) (< bi (len all)))
      (nth all bi)
      (or (first (filter (lambda (t) t.in-selection) (tracks))) (first (tracks))))))

(def select-channel (delta)
  (let ((all (channels))
        (at (index-of all (current-channel)))
        (target (nth all (min (max (+ (max at 0) delta) 0) (- (max (len all) 1) 1))))
        (bus? (listed? target (buses))))
    (when target
      (clear-delete-target)
      (if bus?
        (do
          (seq-clear-selection)
          (set! eseq.seq-core-state/selected-bus target.index))
        (do
          (set! eseq.seq-core-state/selected-bus -1)
          (set! selection.track target)))))
  true)

(def select-prev-channel () (select-channel -1))

(def select-next-channel () (select-channel 1))

(def delete-selected-track ()
  (seq-delete-active-target))

;; BS/Delete: an armed delete target (an explicit click on a strip's delete
;; badge) wins; with nothing armed the handler declines the key so the
;; inherited sequencer keymap deletes the selected steps instead.
(def delete-key ()
  (if (seq-delete-active-target) true false))

(def handle-key (key text)
  (match key
    "LEFT" (select-prev-channel)
    "RIGHT" (select-next-channel)
    "BS" (delete-key)
    "Delete" (delete-key)
    _ false))

(def bus-strip (b)
  (box :key (str "bus-strip-" b.index)
    :width bus-strip-width :height (bus-strip-height)
    ;; Bound selection state (eseq-4jv): a raw `selected-bus` read here
    ;; re-rendered every bus strip on each selection.
    :selected (eseq.seq-core-state/bus-selected-vis-binding b.index)
    :muted #'b.muted
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
    :drop-meta (dict :kind "bus" :bus b.index)
    :on-drop (lambda (event) (drop-effect-on-bus event))
    :on-click (lambda (event) (select-bus b))
    (v-stack :gap 0.25
      (bus-output-dropdown b)
      (clip-growth-spacer)
      (h-stack :gap 0.45 :align :center
        (box :width 3.0 :height (if (compact?) 1.0 5.0))
        (bus-meter-control b false))
      (box :height (if (compact?) 0 2.8))
      (bus-buttons b nil)
      ;; Mix/Main is the graph output and has no external modulation inputs.
      ;; Every other bus has the same four backend inputs used by group buses.
      (if (main-bus? b)
        (box :height (if (compact?) 0 0.8))
        (bus-mod-port-row b))
      (button (substring (bus-label b) 0 (bus-name-chars))
        :width :fill :height 1.0 :padding 0 :font-size 10
        :background-color :mixer-label-bg
        :border-color :transparent
        :color :white
        :on-click (lambda (event) (select-bus b))))))

;; Mute and solo of bus b (a group's, with `g`). Mute is lit while the bus
;; passes audio, matching the track strips.
(def bus-buttons (b g)
  (let ((select-strip (lambda () (if g (select-group g) (select-bus b)))))
    (h-stack :gap 0.35
      (strip-button (if g "M" (bus-mute-label b)) #'b.muted
        :key (if g (str "group-mute-" g.gid) nil)
        :bg :control-on-bg :active-bg :mixer-control-bg
        :idle-color :control-on-fg :color :dim
        :on-click (lambda (event) (select-strip) (toggle! b.muted)))
      (strip-button "S" #'b.soloed :key (if g (str "group-solo-" g.gid) nil)
        :on-click (lambda (event) (select-strip) (toggle! b.soloed)))
      (if g
        ;; A rack owns a pad-play arm; a plain group is no input target.
        (if g.rack
          (strip-button "R" #'g.armed :key (str "group-arm-" g.gid) :color :black
            :active-bg arm-color
            :on-click (lambda (event) (select-strip) (toggle! g.armed)))
          (nothing))
        (box :width 2.1 :height 1.0)))))

;; --- Track groups -------------------------------------------------------

;; A group drawn inside another group's block (a rack in a plain group,
;; docs/drum-rack-v2-spec.md) is not a top-level render item: its parent
;; draws it.
(def group-nested? (g)
  (not (= g.parent nil)))

;; The lowest position among tracks, or -1 for none.
(def lowest-index (ts)
  (reduce |acc t| (if (or (< acc 0) (< t.index acc)) t.index acc) -1 ts))

;; Where a group sits in the flat track order: the lowest member track of the
;; group itself or of any rack nested inside it. -1 when nothing is claimed yet.
(def group-anchor (g)
  (reduce |acc child|
    (let ((a (lowest-index child.tracks)))
      (if (< acc 0) a (if (< a 0) acc (min a acc))))
    (lowest-index g.tracks)
    g.racks))

(def has-clips? (g)
  (> (len g.clips) 0))

;; Whether bus b backs a group (hidden from the ordinary bus list).
(def group-bus? (b)
  (> (len (filter (lambda (g) (= g.bus b)) (groups))) 0))

;; The flat mixer render-item list: loose tracks and group containers, each
;; group anchored at its lowest member position (visual contiguity without
;; reindexing the track list). Top-level groups that have claimed no track
;; yet (an empty rack: its pads are lazy) have no anchor, so they follow the
;; tracks — the grid does the same.
(def render-order ()
  (let ((top (filter (lambda (g) (not (group-nested? g))) (groups)))
        (anchored (map (lambda (g) (list g (group-anchor g))) top)))
    (append
      (reduce |acc t|
        (let ((hit (first (filter (lambda (ga) (= (nth ga 1) t.index)) anchored))))
          (if hit
            (append acc (list (dict :kind "group" :group (nth hit 0))))
            (if t.group
              acc
              (append acc (list (dict :kind "loose" :track t))))))
        (list)
        (tracks))
      (map (lambda (ga) (dict :kind "group" :group (nth ga 0)))
        (filter (lambda (ga) (< (nth ga 1) 0)) anchored)))))

(def select-group (g)
  (when g.bus (select-bus g.bus)))

(def select-group-delete-target (g)
  (select-group g)
  (set! g.delete-target true))

;; Play glyph for a clip row: a rounded tile with a disclosure triangle that
;; lights when the row is the clip the current scene plays.
(defwidget rack-clip-play
  :width 1.4 :height 1.4
  :state (playing)
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

;; Sized so column + the v-stack gap + the meter box equal the spacer-plus-
;; 8.1 meter box a clipless group strip has: the meters stay level. Past the
;; visible rows the list scrolls, following the playing clip the way the
;; session-view package follows a track's effective clip: only a changed row
;; scrolls, so a manual scroll in between holds. A launch re-renders this
;; column alone (the follow row); the play glyphs only repaint.
(def rack-clip-column (g)
  (subtree :key (str "rack-clip-column-sub-" g.gid)
    (let ((playing g.rack-clip)
          (row-bg (color-rgba g.color 1.0)))
      (box :key (str "rack-clip-column-" g.gid)
        :width :fill :height (- (clip-area-height) 0.1) :align :top
        :bg :black :background-color :buffer-bg
        (scroll :key (str "rack-clip-scroll-" g.gid) :width :fill :height :fill
          :center-row (if playing (* playing.index (+ rack-clip-row-height 0.01)) -1)
          :center-span rack-clip-row-height
          (v-stack :gap 0.01
            (each g.clips |rc|
              (box :key (str "mixer-rack-clip-" g.gid "-" rc.cid)
                :debug-name "mixer-rack-clip-cell"
                :width :fill :height rack-clip-row-height :padding 0.01
                :corner-radius 0
                :background-color row-bg
                :on-click (lambda (event) (launch-rack-clip! rc))
                (h-stack :gap 0.3 :align :center
                  (rack-clip-play :playing #'rc.active)
                  (apply label (substring rc.name 0 (name-chars 9))
                    :key (str "mixer-rack-clip-label-" g.gid "-" rc.cid)
                    :font-size 9 :h-align :left :v-align :center
                    :color :black
                    bare-text))))))))))

;; The group's own channel slot (collapse toggle + name) shown at the left of
;; the container, over the container color. It matches a bus strip's width so
;; the full rack mute/solo/arm row has room without crowding the container.
;; Meter, fader, mute and solo are the group's bus; selecting or dragging
;; them selects that bus. With the clip column above, the meter gives up the
;; spacer that kept it level.
(def group-header-slot (g)
  (let ((b g.bus)
        (bus-idx (if b b.index -1))
        ;; Compact mode has no clip area for the column to stand in.
        (show-clips (and (not (compact?)) (has-clips? g))))
    (box :key (str "group-bus-strip-" bus-idx)
      :width 10.2 :height (group-bus-strip-height)
      :corner-radius (eseq.seq-core-state/radius 12)
      :padding 0.1
      :background-color :mixer-strip-bg
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types (if b
        (list "sample" "instrument" "instrument-preset" "audio-effect")
        (list))
      :drop-meta (dict :kind "bus" :bus bus-idx)
      :on-drop (lambda (event) (drop-on-group-header event g))
      (v-stack :gap 0.3 :align :center
        (bus-output-dropdown b)
        ;; Where a plain track strip shows its pattern grid, a clip-bearing
        ;; rack shows its clip run vertically (§6.2). Everything else keeps the
        ;; spacer that levels the meters.
        (if show-clips (rack-clip-column g) (clip-growth-spacer))
        (if b
          (v-stack :gap 0.4 :align :center
            (box :width :fill :height (if (compact?) 4.1 (if show-clips 3.9 8.1))
              (if show-clips
                (bus-meter-control-with-spacer b true 0.0)
                (bus-meter-control b true))))
          (nothing))
        (if b
          (h-stack :gap 0.35 :align :left :padding 0.1 :width :fill
            (bus-buttons b g))
          (nothing))
        (if b (bus-mod-port-row b) nil)
        (box :corner-radius (eseq.seq-core-state/radius 34) :background-color g.color :width 9.5 :padding 0.1
          :key (str "group-badge-" g.gid)
          :selected #'g.delete-target
          :selected-background-color :fx-panel-header-selected-bg
          :on-click (lambda (event) (select-group-delete-target g))
          :on-double-click (lambda (event) (eseq.sequencer/show-fx-for-group g.index))
          :on-right-click (lambda (event) (open-strip-menu event nil g))
          (h-stack :gap 0.2 :align :center
            (box :width 0.05)
            (box :background "disclosure-button"
              :width 1.7 :height 0.8
              :collapsed g.collapsed
              :surface-alpha 0.35
              :col 0.1
              :on-click (lambda (event)
                (select-group g)
                (toggle! g.collapsed)))
            (box :width 0.05)
            (if (= strip-rename.group g)
              (group-rename-input g)
              (apply label (substring g.name 0 (name-chars 10))
                :key (str "group-name-label-" g.gid)
                :font-size 11
                :height 0.9
                :v-align :center
                :h-align :center
                :color :black
                bare-text))))))))

(def drop-new-track-into-group (event g)
  (let ((payload event.payload)
        (path payload.path)
        (name payload.name))
    (select-group g)
    (match event.drag-type
      "instrument-preset" (eseq.browser/drop-preset-new-track payload g.gid)
      ;; The builtin add-track host commands take no :group-id (a rack even
      ;; creates its own group), so a builtin dropped on a group header is
      ;; refused rather than silently landing outside the group (eseq-mj8).
      "instrument"
      (if (= payload.kind "builtin-instrument")
        (status "Builtin instruments cannot be added inside a group")
        (if name
          (do
            (set! sbrowser-loading-instrument-name name)
            (host-command "add-track-instrument" (dict :name name :group-id g.gid)))
          (status "Drop an instrument, not a folder")))
      _
      (if path
        (host-command "add-track-sample"
          (dict :path path :group-id g.gid :preserve-browser-context true))
        (status "Drop a sample file, not a folder")))))

(def drop-on-group-header (event g)
  (if (= event.drag-type "audio-effect")
    (drop-effect-on-bus event)
    (drop-new-track-into-group event g)))

(def track-strip-of (t)
  (if t.collapsed (track-collapsed-strip t) (track-strip t)))

;; A group rendered as a real container: a colored box wrapping the group's
;; channel slot and its member strips, with a top spacer so the color shows
;; above the contained tracks.
(def group-container (g)
  (let ((c g.color))
    (box
      :corner-radius (eseq.seq-core-state/radius 16)
      :padding 0.3
      ;; Selection is a bound state, not a computed color (eseq-4jv): the
      ;; selected look keeps a constant border width so it never relayouts.
      :selected (eseq.seq-core-state/group-selected-vis-binding g.gid)
      :background-color (color-rgba c 0.78)
      :selected-background-color (color-rgba c 1.0)
      :border-width 2
      :border-color :mixer-strip-border
      :selected-border-color :mixer-strip-selected-border
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-types (list "track-badge")
      :drop-meta (dict :kind "group" :gidx g.index)
      :on-drop (lambda (event) (drop-track-into-group event g))
      :on-click (lambda (event) (select-group g))
      (h-stack :gap 0.0 :align :start
        (group-header-slot g)
        (if g.collapsed
          (nothing)
          (v-stack :gap 0.0
            (box :width :fill :height 0.85 :bg :transparent)
            (h-stack :gap 0.1
              (each g.tracks |t|
                (subtree :key (str "mixer-v2-track-" t.index)
                  (track-strip-of t)))
              ;; Child racks draw as their own container inside this block —
              ;; collapsed, that is a single header strip; expanded, the rack
              ;; header plus its members (docs/drum-rack-v2-spec.md).
              (each g.racks |child|
                (subtree :key (str "mixer-v2-group-" child.gid)
                  (group-container child))))))))))

(def render-item (item)
  (let ((g (get item :group))
        (t (get item :track)))
    (if g
      (subtree :key (str "mixer-v2-group-" g.gid)
        (group-container g))
      (subtree :key (str "mixer-v2-track-" t.index)
        (track-strip-of t)))))

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
    :drop-types (list "sample" "instrument" "instrument-preset" "sound" "track-badge")
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

;; What the poly / voices controls edit: on a drum rack the selected slot
;; (its playback polyphony is per slot), else the track.
(def rack-slot-device (t)
  (let ((rack (first t.devices))
        (slot selection.rack-slot))
    (if (and t.rack rack (>= slot 0)) (nth rack.devices slot) nil)))

(def voices-of (t)
  (let ((d (rack-slot-device t)))
    (if d d.voices t.max-polyphony)))

(def poly-of? (t)
  (let ((d (rack-slot-device t)))
    (if d (> d.voices 1) t.poly)))

(def set-voices! (t v)
  (eseq.seq-core-state/cool-off-follow)
  (let ((d (rack-slot-device t)))
    (if d (set! d.voices v) (set! t.max-polyphony v))))

(def toggle-poly! (t)
  (eseq.seq-core-state/cool-off-follow)
  (let ((d (rack-slot-device t)))
    (if d
      (set! d.voices (if (> d.voices 1) 1 6))
      (toggle! t.poly))))

(def patch-mixer-strip (t)
  (track-frame t 10.0 10.7 2 16 (list)
    (v-stack :gap 0.35 :align :right
      ;; poly/voices — the same track settings the *track* buffer's parameter
      ;; panel edits, surfaced here so a patch can be auditioned in mono
      ;; without leaving the patch editor.
      (subtree :key (str "patch-mixer-strip-poly-" t.index)
        (let ((poly (poly-of? t)))
          (box :width :fill :height 3
            (h-stack :gap 0.7 :align :center
              (v-stack :align :center :gap 0.34
                (label "poly" :font-size 8 :color :dim :bg :transparent)
                (button (if poly "ON" "OFF") :width 3.2 :height 1.3
                  :background-color (if poly :control-on-bg :poly-off-bg)
                  :border-color :none
                  :font-size 11
                  :color (if poly :control-on-fg :poly-off-fg)
                  :on-click |x y r| (toggle-poly! t)))
              (v-stack :align :center :gap 0.5
                (label "voices" :font-size 8 :color :dim :bg :transparent)
                (number-picker :value (voices-of t) :min 1 :max 16 :decimals 0
                  :noui false :font-size 8 :text-color :white
                  :background-color :mixer-strip-bg
                  :border-color :none
                  :on-change (lambda (v) (set-voices! t v))
                  :width 3.4 :height 1.15))))))
      (h-stack
        (box :width 3.5)
        (track-meter-control t))
      (subtree :key (str "patch-mixer-strip-buttons-" t.index)
        (h-stack
          (strip-buttons t)))
      (subtree :key (str "patch-mixer-strip-label-" t.index)
        (strip-label t)))))

;; The patch mixer has no keys of its own; it takes the shared sequencer keymap.
(set-buffer-mode-for "*patch-mixer*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*patch-mixer*"
  (box :padding 0.2
    (let ((t selection.track))
      (subtree :key (str "patch-mixer-track-" (if t t.index -1))
        (when t (patch-mixer-strip t))))
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
      (each (display-buses) |b|
        (subtree :key (str "mixer-v2-bus-" b.index)
          (if (group-bus? b)
            (nothing)
            (bus-strip b))))
      (track-context-menu)
      (subtree :key "mixer-param-plock-menu" (pc/param-plock-context-menu "mixer")))))

(effect-buffer "*mixer*"
  (mixer-body))

;; Ctrl+G / Cmd+G — fold the multi-selected tracks into a new group.
(def group-selected ()
  (host-command "group-selected-tracks" (dict))
  true)

;; Global grouping dispatcher. Multi-selection is shared across the sequencer
;; UI, so both shortcuts work from any tile that accepts global UI shortcuts.
(def seq-ctrl-g ()
  (if (>= (len selection.tracks) 2)
    (group-selected)
    (status "Select 2+ tracks to group")))

(define-mode "seq-mixer-mode" :read-only true :live-keys true :on-key "handle-key"
  :inherit "eseq.sequencer-keys/sequencer-keys")
(mode-bind-key "seq-mixer-mode" "LEFT" "select-prev-channel")
(mode-bind-key "seq-mixer-mode" "RIGHT" "select-next-channel")
(set-buffer-mode-for "*mixer*" "seq-mixer-mode")
