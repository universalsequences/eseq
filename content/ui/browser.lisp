;; ui/browser.lisp — Sample browser mode for Metal Sequencer
;; C-x s to open, type to filter, Enter to audition, +/= to add track, q to quit
;; Uses tree widget inside scroll container for hierarchical browsing.

(module eseq.browser)
;; Compile-time edge (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile.
(import eseq.seq-core-state)

;; Host state comes from the kinds (kind-bindings spec §13 stage 8): the
;; sidebar's track, instrument and presets (`browser`), the editor
;; (`editor`), the current track (`selection.track`). The view's own state
;; is the `:key ()` singletons below; the host reaches them through a local
;; (`(let ((v eseq.browser/browser-view)) (set! v.tab "samples"))`) or the
;; exported helpers (`show-loading!`, `clear-editor-name!`,
;; `show-browser-tab!`, `mark-auditioned!`).

(import eseq.track-collapse)
(import eseq.drum-rack-v2)
(import eseq.view-kit :refer (open-menu! menu-of listed?))
(import eseq.preview-strip :refer (sync-preview! toggle-preview! preview-strip))
(import eseq.kinds :refer (track tracks scenes groups project selection browser editor))

(export browser-view
        sample-pick
        sample-preview
        instrument-pick
        editor-draft
        kit-save
        preset-save
        package-menu
        instrument-menu
        package-draft
        instance-rename
        show-loading!
        clear-editor-name!
        show-browser-tab!
        mark-auditioned!
        editor-status-row
        open-project-save
        open-project-browser
        new-project
        drop-instrument-new-track
        drop-instrument-on-track
        drop-sample-on-track
        drop-sound-on-track
        add-sampler-track
        open-device-picker
        add-rack-track
        add-layer-rack-track
        audition
        select-sample
        activate-sample
        sample-selected-path
        add-selected-rack-layer
        select-tab
        next-tab
        active-tree-key
        list-contains?
        select-audio-effect
        fork-selected-audio-effect
        enter-new-effect-editor
        activate-audio-effect
        select-midi-effect
        activate-midi-effect
        search-header
        create-items
        add-builtin-instrument-track
        select-create-item
        focus-create-item
        fork-selected-instrument
        drop-instrument-on-folder
        enter-kit-save
        drop-preset-on-sounds
        tab-rail
        sample-browser-widget
        active-tab-panel
        tabbed-content
        enter-preset-save
        build-widgets
        refresh-buffer
        sample-browser-here
        ;; Packages tab rows (instance-kinds spec §8.3); exported for the
        ;; host-command tests in src/ui/state_values/tests.rs.
        activate-package-item
        open-package-menu
        package-menu-actions
        select-package-menu-action)

;; ── State ──

;; The sidebar: the tab it shows, its mode ("audition", or "create-sampler"
;; while a sample picks a new track), and the search boxes (the tab's, and
;; the preset list's).
(def-kind browser-view
  :key ()
  :state ((tab "samples")
          (mode "audition")
          (search "")
          (preset-search "")))

;; The Samples tab: the tag and origin filters, the sample the tree selected
;; and the one last auditioned, and the shown track and sample the filters
;; were last reset for (`sync-track-search`).
(def-kind sample-pick
  :key ()
  :state ((tags '())
          (origins '())
          (sample "")
          (auditioned "")
          (shown-track track :default nil)
          (shown-sample "")))

;; The preview strip below the samples tree (Ableton-style): the sample the
;; tree cursor last landed on and its waveform-buffer map from
;; `seq-sample-waveform` (false while nothing decodable is focused), cached
;; so the strip's build path never decodes. While `auto` (the headphone
;; toggle) is on, landing on a sample also plays it once.
(def-kind sample-preview
  :key ()
  :state ((path "")
          (buffer :any :default false)
          (auto false)))

;; The Instruments and Audio FX tabs: the saved-instrument tier filter, the
;; favorites chip, the instrument and custom effect last highlighted (Fork
;; acts on them: the tree has no context menu, so the action lives in the
;; toolbar, docs/instrument-fork-spec.md §3.5), and the saved instrument
;; being loaded (its row spins).
(def-kind instrument-pick
  :key ()
  :state ((origin "")
          (favorites-only false)
          (favorites-epoch 0)
          (instrument "")
          (audio-effect "")
          (loading "")))

;; The instrument / effect editor's Save-as name.
(def-kind editor-draft
  :key ()
  :state ((name "")))

;; Kit saves are initiated by the drum rack's FX-panel header, but completed
;; here so naming and browser placement are explicit before anything is
;; written. `scenes`: the project scenes whose rack clip travels in the kit,
;; as scene indices (rack-clips spec 7.2; the command's address).
(def-kind kit-save
  :key ()
  :state ((open false)
          (name "")
          (group-id -1)
          (scenes '())))

(def-kind preset-save
  :key ()
  :state ((open false)
          (name "")))

;; The Packages tree's context menu and the Instruments tree's (the row each
;; was opened on), the inline New Package field, and the inline rename of a
;; kind instance row (instance-kinds spec §8.3; `target` is its instance id,
;; the rename command's address, -1 while none).
(def-kind package-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (item :any :default nil)))

(def-kind instrument-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (item :any :default nil)))

(def-kind package-draft
  :key ()
  :state ((open false)
          (name "")))

(def-kind instance-rename
  :key ()
  :state ((target -1)
          (draft "")))

(def source-buffer "")

;; The saved instrument being loaded: its tree row spins until the host
;; clears it (`(show-loading! "")`).
(def show-loading! (name)
  (set! instrument-pick.loading name))

(def clear-editor-name! ()
  (set! editor-draft.name ""))

;; Shows tab `name`, keeping the searches (`select-tab` clears them).
(def show-browser-tab! (name)
  (set! browser-view.tab name))

;; Marks `path` as the sample the browser itself loaded, so the sidebar's
;; next sync keeps its filters (`sync-track-search`).
(def mark-auditioned! (path)
  (set! sample-pick.auditioned path))

;; The track at position i (a drop target's address), or nil.
(def track-at (i)
  (if (= i nil) nil (if (>= i 0) (track i) nil)))

;; Whether track t plays an instrument of `type` (track.instrument-type).
(def instrument-type? (t type)
  (and t (= t.instrument-type type)))

(def replaceable? (t)
  (and t (eseq.track-collapse/replaceable-type? t.instrument-type)))

(def no-tracks? ()
  (= (len (tracks)) 0))

;; Track t's position (the current track by default), the host commands'
;; address (0 with none).
(def current-index (&optional (t selection.track))
  (if t t.index 0))

(defwidget editor-spinner
  ;; Sized to sit inside `editor-status-row`'s 1.35-row height — the
  ;; row does not clip, so an oversized spinner paints over its neighbours.
  ;; The dot row spans x = -0.72..0.72 in local space, so keep the box wider
  ;; than it is tall or the outer dots run off the edges.
  :width 3.0 :height 1.25
  :animates true
  :shader
  (let ((phase (* itime 5.4))
        (p0 (+ 0.45 (* 0.55 (sin phase))))
        (p1 (+ 0.45 (* 0.55 (sin (- phase 0.75)))))
        (p2 (+ 0.45 (* 0.55 (sin (- phase 1.5)))))
        (p3 (+ 0.45 (* 0.55 (sin (- phase 2.25)))))
        (p4 (+ 0.45 (* 0.55 (sin (- phase 3.0)))))
        (r0 (+ 0.17 (* 0.12 p0)))
        (r1 (+ 0.17 (* 0.12 p1)))
        (r2 (+ 0.17 (* 0.12 p2)))
        (r3 (+ 0.17 (* 0.12 p3)))
        (r4 (+ 0.17 (* 0.12 p4))))
    (sdf/layer
      (sdf/fill (sdf/translate -0.72 0 (sdf/circle r0))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p0)))))
      (sdf/fill (sdf/translate -0.36 0 (sdf/circle r1))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p1)))))
      (sdf/fill (sdf/translate 0 0 (sdf/circle r2))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p2)))))
      (sdf/fill (sdf/translate 0.36 0 (sdf/circle r3))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p3)))))
      (sdf/fill (sdf/translate 0.72 0 (sdf/circle r4))
        (material :color (rgba 0.22 0.52 1.0 (+ 0.35 (* 0.65 p4))))))))

(def editor-status-row (text color)
  ;; :center, not :baseline — the spinner is a bare SDF with no text baseline
  ;; to align against.
  (h-stack :width :fill :height 1.35 :gap 0.5 :align :center
    (editor-spinner :width 2.8 :height 1.15)
    (label text
      :font-size 9
      :color color
      :bg :transparent)))

(def editor-busy? ()
  (or editor.canceling
    (= editor.error "Preview compiling...")))

(def audition-mode? ()
  (= browser-view.mode "audition"))

(def track-type-mode? ()
  (or (= browser-view.mode "track-type")
    (and (no-tracks?) (= browser-view.mode "audition"))))

(def create-sampler-mode? ()
  (= browser-view.mode "create-sampler"))

(def project-browser-mode? ()
  (= browser-view.mode "project-browser"))

(def editor-mode? ()
  (not (= editor.mode "")))

(def create-mode? ()
  (or (track-type-mode?) (create-sampler-mode?)))

(def mode-label ()
  (if (audition-mode?) "audition" "create"))

(def sync-track-search ()
  (let ((track-changed (not (= sample-pick.shown-track browser.track)))
        (sample-changed (not (= sample-pick.shown-sample browser.sample))))
  (if (or track-changed sample-changed)
    (do
      (set! sample-pick.shown-track browser.track)
      (set! sample-pick.shown-sample browser.sample)
      (if (and (audition-mode?) (= browser.instrument-kind "sampler"))
        (do
          (set! sample-pick.sample browser.sample)
          (if (and (or sample-changed track-changed) (= sample-pick.auditioned browser.sample))
            (set! sample-pick.auditioned "")
            (do
              (set! sample-pick.auditioned "")
              (if (= browser-view.tab "samples")
                (set! browser-view.search ""))
              (set! sample-pick.tags
                (if (= browser.sample "")
                  (list)
                  (seq-sample-tags-for-path browser.sample)))))))))))

(def reset-to-audition ()
  (set! browser-view.mode "audition")
  (set! browser-view.search "")
  (set! sample-pick.tags (list)))

(def leave-create-mode ()
  (set! browser-view.mode "audition")
  (set! browser-view.search "")
  (set! sample-pick.tags (list)))

(def open-device-picker ()
  (set! browser-view.search "")
  (set! browser-view.mode "audition")
  (set! browser-view.tab "instruments")
  (if eseq.seq-core-state/samples-sidebar-visible
    nil
    (eseq.seq-panels/seq-toggle-samples-sidebar)))

(def enter-create-track-mode ()
  (set! browser-view.search "")
  (set! browser-view.mode "audition")
  (set! browser-view.tab "instruments"))

(def toggle-create-track-mode ()
  (enter-create-track-mode))

(def enter-create-sampler-mode ()
  (set! browser-view.search "")
  (set! sample-pick.tags (list))
  (set! browser-view.mode "create-sampler")
  (set! browser-view.tab "samples")
  (status "Create sampler track: choose a sample"))

(def open-project-browser ()
  (set! browser-view.search "")
  (set! browser-view.mode "audition")
  (set! browser-view.tab "projects"))

;; Saving a named project writes it directly; an unnamed project opens the
;; File-menu name modal (eseq.file-dialogs) instead of a sidebar mode.
(def open-project-save ()
  (host-command "project-save-open" (dict :mode "save")))

(def new-project ()
  (host-command "new-project" (dict))
  (set! browser-view.search "")
  (set! browser-view.tab "projects")
  (status "New project"))

(def add-instrument-track (name)
  (show-loading! name)
  (host-command "add-track-instrument" (dict :name name))
  (eseq.seq-panels/seq-show-fx-lower-panel)
  (status (str "Loading instrument: " name)))

;; i: the track's position (a drop target's address).
(def swap-track-instrument (i name preserve-track-selection)
  (if (or (= name nil) (= name ""))
    (status "Drop an instrument, not a folder")
    (if (replaceable? (track-at i))
      (do
        (show-loading! name)
        (host-command "swap-track-instrument"
          (dict :track i :name name
            :preserve-track-selection preserve-track-selection))
        (eseq.seq-panels/seq-show-fx-lower-panel)
        (status (str "Loading instrument swap: " name)))
      (status "This track cannot load an instrument"))))

(def swap-track-builtin-instrument (i name preserve-track-selection)
  ;; Only the sampler has an in-place conversion. A modulator rewrite would be a
  ;; different engine on a track that may already carry pattern data, and the
  ;; racks are group/slot entities with no single track to become — so a drop
  ;; that visually landed adds the builtin as a new track instead of dead-ending
  ;; (eseq-mj8).
  (if (and (= name "sampler") (replaceable? (track-at i)))
    (do
      (host-command "swap-track-builtin-instrument"
        (dict :track i :name name
          :preserve-track-selection preserve-track-selection))
      (set! browser-view.tab "samples")
      (eseq.seq-panels/seq-show-fx-lower-panel)
      (status "Loading sampler"))
    (add-builtin-instrument-track name)))

;; Every "drop onto the new-track / empty zone" site routes through here so the
;; builtin rows never fall into add-track-instrument, which resolves SAVED
;; instruments by name and would look for a file literally called "modulator".
;; Builtins also skip the loading spinner, which is keyed to saved-instrument
;; loads (eseq-mj8).
(def drop-instrument-new-track (payload)
  (let ((name (get payload :name)))
    (if name
      (if (= (get payload :kind) "builtin-instrument")
        (add-builtin-instrument-track name)
        (add-instrument-track name))
      (status "Drop an instrument, not a folder"))))

(def drop-instrument-on-track (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((preserve-track-selection (get target :from-pad)))
      (if (= (get payload :kind) "builtin-instrument")
        (swap-track-builtin-instrument
          (get target :track)
          (get payload :name)
          preserve-track-selection)
        (swap-track-instrument
          (get target :track)
          (get payload :name)
          preserve-track-selection)))))

(def drop-sample-on-track (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((path (get payload :path))
          (i (get target :track)))
      (if path
        (if (instrument-type? (track-at i) "custom")
          (host-command "convert-track-to-sampler"
            (dict :track i :path path :preserve-browser-context true
              :preserve-track-selection (get target :from-pad)))
          (host-command "load-sample-into-track"
            (dict :track i :path path :preserve-browser-context true
              :preserve-track-selection (get target :from-pad))))
        (status "Drop a sample file, not a folder")))))

(def drop-sound-on-track (event)
  (if (= (get event :drag-type) "sound")
    (host-command "load-sound-onto-track"
      (dict :track (get (get event :target) :track)
            :path (get (get event :payload) :path)
            :preserve-track-selection (get (get event :target) :from-pad)))
    (if (= (get event :drag-type) "instrument")
      (drop-instrument-on-track event)
      (if (= (get event :drag-type) "instrument-preset")
        (drop-preset-on-track event)
        (drop-sample-on-track event)))))

;; ── Dragged presets ──
;; A preset row carries its :instrument and :preset (see `seq-preset-tree`), so
;; it drops everywhere an instrument does: onto a track it swaps in that
;; instrument at that preset (or, when the track already runs the instrument,
;; only switches the preset), and onto a new-track zone it adds the instrument
;; at that preset.

;; The dragged row's instrument, or nil when the row names none (a rack
;; track's rack presets are not an instrument's presets).
(def preset-payload-instrument (payload)
  (let ((instrument (get payload :instrument)))
    (if (and instrument (not (= instrument "")) (get payload :preset))
      instrument
      nil)))

;; group-id is nil for a loose track.
(def drop-preset-new-track (payload group-id)
  (let ((instrument (preset-payload-instrument payload)))
    (if instrument
      (do
        (show-loading! instrument)
        (host-command "add-track-instrument"
          (dict :name instrument :preset (get payload :preset) :group-id group-id))
        (eseq.seq-panels/seq-show-fx-lower-panel)
        (status (str "Loading preset: " (get payload :preset))))
      (status "Drop an instrument preset"))))

(def drop-preset-on-track (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((instrument (preset-payload-instrument payload))
          (i (get target :track)))
      (if instrument
        (if (replaceable? (track-at i))
          (do
            (show-loading! instrument)
            (host-command "swap-track-instrument"
              (dict :track i :name instrument :preset (get payload :preset)
                :preserve-track-selection (get target :from-pad)))
            (eseq.seq-panels/seq-show-fx-lower-panel)
            (status (str "Loading preset: " (get payload :preset))))
          (status "This track cannot load an instrument"))
        (status "Drop an instrument preset")))))

(def activate-instrument (name)
  (if (>= (selected-drum-rack-id) 0)
    (status "Saved instruments cannot replace a selected drum rack")
    (if (replaceable? selection.track)
      (swap-track-instrument (current-index) name false)
      (do
        (add-instrument-track name)
        (status (str "Adding instrument track: " name))))))

(def add-sampler-track ()
  (host-command "add-track-sampler" (dict))
  (set! browser-view.tab "samples")
  (status "Add sampler track"))

(def add-modulator-track ()
  (host-command "add-track-modulator" (dict))
  (set! browser-view.tab "instruments")
  (status "Add modulator track"))

(def add-rack-track ()
  (let ((path (sample-selected-path)))
    (if (= path "")
      (host-command "add-track-rack" (dict))
      (do
        (set! sample-pick.auditioned path)
        (host-command "add-track-rack" (dict :path path)))))
  (set! browser-view.tab "samples")
  (status "Add drum rack"))

;; True when a rack panel is actually open in the sidebar — the only thing the
;; old `:routing` string ever distinguished now that Broadcast is the sole
;; routing (docs/drum-rack-v2-spec.md).
;; (The sidebar's track is a rack track: browser.instrument-kind reads
;; "instrument" for it, never "rack".)
(def rack-panel-open? ()
  (let ((t browser.track)) (and t t.rack)))

;; A new instrument-rack track, holding the selected sample when there is one.
(def add-new-layer-rack-track ()
  (let ((path (sample-selected-path)))
    (if (= path "")
      (host-command "add-track-layer-rack" (dict))
      (do
        (set! sample-pick.auditioned path)
        (host-command "add-track-layer-rack" (dict :path path)))))
  (set! browser-view.tab "samples")
  (status "Add instrument rack"))

;; The selected sample as a layer of the open rack, else a new rack track.
(def add-layer-rack-track ()
  (let ((path (sample-selected-path)))
    (if (and (rack-panel-open?)
             (not (= path "")))
      (do
        (set! sample-pick.auditioned path)
        (host-command "add-rack-sample-slot"
          (dict :track (current-index) :path path :preserve-browser-context true))
        (status "Add layer"))
      (add-new-layer-rack-track))))

;; ── SDF widgets ──

(defwidget browser-panel-bg
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect (* width 1) (* height 1) 0.02)
              (material :color :buffer-bg))))

(defwidget browser-pill-btn-bg
  :width 1 :height 1
  :paint-margin 0.3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height height)
      (material
        :color (rgba 0.00 0.35 0.82 1.0)
        :shadow (shadow :color (rgba 0 0 0 0.42) :blur 0.06 :offset (vec2 0 0.02))))))

(defwidget mag-glass
  :width 2 :height 2
  :paint-margin 0.4
  :shader
  (let (
      (__cx -0.05) (__cy 0.08) (__r 0.5)
      (__lens (- (sqrt (+ (* (- x __cx) (- x __cx))
                          (* (- y __cy) (- y __cy)))) __r))
      (__ring (- (abs __lens) 0.07))
      (__px (- x 0.35)) (__py (- y 0.385))
      (__cos 0.866) (__sin 0.5)
      (__rx (+ (* __cos __px) (* __sin __py)))
      (__ry (- (* __cos __py) (* __sin __px)))
      (__hx (- __rx (clamp __rx 0.0 0.4)))
      (__handle (- (sqrt (+ (* __hx __hx) (* __ry __ry))) 0.08))
      (__shape (min __ring __handle)))
    (sdf/layer
      (sdf/fill __shape
        (material
          :color :search-icon)))))

;; ── Actions ──

(def audition (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sample-pick.auditioned path)
        (host-command "audition-sample" (dict :path path))
        (status (str "Audition: " (get item :label))))
      (status (str (get item :label))))))

(def sync-sample-preview (path)
  (sync-preview! sample-preview path))

(def select-sample (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sample-pick.sample path)
        (sync-sample-preview path))
      (status (str (get item :label))))))

(def toggle-auto-preview ()
  (status (if (toggle-preview! sample-preview) "Sample preview on" "Sample preview off")))

(def selected-drum-rack-id ()
  (let ((g (eseq.drum-rack-v2/selected-bus-rack)))
    (if g g.gid -1)))

(def activate-sample (item)
  (let ((path (get item :path)))
    (if path
      (if (or (create-sampler-mode?) (no-tracks?))
        (add-track item)
        (if (instrument-type? selection.track "empty")
          (host-command "load-sample-into-track"
            (dict :track (current-index) :path path :preserve-browser-context true))
          (if (= browser.instrument-kind "sampler")
            (audition item)
            (status "Drop samples onto a sampler track or the new-track drop zone"))))
      (status "Choose a sample file, not a folder"))))

(def choose-learn-target (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sample-pick.sample path)
        (sync-sample-preview path)
        (host-command "set-learn-target"
          (dict :path path :name (get item :label))))
      (status "Choose a sample file, not a folder"))))

(def change-learn-target ()
  (host-command "set-learn-target" (dict :path "")))

(def sample-selected-path ()
  (if (= sample-pick.sample "")
    browser.sample
    sample-pick.sample))

(def add-track (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sample-pick.auditioned path)
        (host-command "add-track-sample" (dict :path path))
        (leave-create-mode)
        (status (str "Add track: " (get item :label))))
      (status "Select a sample file, not a folder"))))

(def add-rack-layer (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sample-pick.auditioned path)
        (host-command "add-rack-sample-slot"
          (dict :track (current-index) :path path :preserve-browser-context true))
        (status (str "Add layer: " (get item :label))))
      (status "Select a sample file, not a folder"))))

(def add-selected-rack-layer ()
  (add-layer-rack-track))

(def modified-activate-sample (item)
  (if (rack-panel-open?)
    (add-rack-layer item)
    (add-track item)))

(def select-item (item)
  (if (or (create-sampler-mode?) (no-tracks?) (= browser.instrument-kind "instrument"))
    (add-track item)
    (audition item)))

(def select-tab (name)
  (let ((changed (not (= browser-view.tab name))))
    (set! browser-view.tab name)
    (if changed
      (do
        (set! browser-view.search "")
        (set! browser-view.preset-search "")))
    (if (not (= name "samples"))
      (set! sample-pick.tags (list)))))

(def next-tab-name ()
  (if (= browser-view.tab "samples") "sounds"
    (if (= browser-view.tab "sounds") "instruments"
    (if (= browser-view.tab "instruments") "audio-fx"
      (if (= browser-view.tab "audio-fx") "midi-fx"
        (if (= browser-view.tab "midi-fx") "presets"
          (if (= browser-view.tab "presets") "packages"
            (if (= browser-view.tab "packages") "projects"
              "samples"))))))))

(def next-tab ()
  (select-tab (next-tab-name)))

;; Hazard (l): this hands a widget key *out* to Rust —
;; `sample_browser_active_tree_key` (src/ui/input.rs) feeds the result straight
;; into `focus_widget_by_stable_key`, an exact match.  Auto-qualification happens
;; on the widget, not on a string this file builds, so the module name has to be
;; written into the value.
(def tree-key (base)
  (str "eseq.browser/" base))

(def active-tree-key ()
  (if (= browser-view.tab "samples") (tree-key "samples-tab-tree")
    (if (= browser-view.tab "sounds") (tree-key "sounds-tab-tree")
    (if (= browser-view.tab "kits") (tree-key "kits-tab-tree")
    (if (= browser-view.tab "instruments") (tree-key "instruments-tab-tree")
      (if (= browser-view.tab "audio-fx") (tree-key "audio-fx-tab-tree")
        (if (= browser-view.tab "midi-fx") (tree-key "midi-fx-tab-tree")
          (if (= browser-view.tab "presets") (tree-key "presets-tab-tree")
            (if (= browser-view.tab "packages") (tree-key "packages-tab-tree")
              (tree-key "projects-tab-tree"))))))))))

;; COMPAT(eseq-0l17): eseq.view-kit/listed? with its arguments swapped,
;; the target of the alias table's `sbrowser-list-contains?`.
(def list-contains? (items value) (listed? value items))

(def list-remove (items value)
  (filter (lambda (item) (not (= item value))) items))

(def toggle-tag (tag)
  (if (listed? tag sample-pick.tags)
    (set! sample-pick.tags (list-remove sample-pick.tags tag))
    (set! sample-pick.tags (append sample-pick.tags (list tag)))))

(def clear-tags ()
  (set! sample-pick.tags (list)))

(def toggle-origin (origin)
  (if (listed? origin sample-pick.origins)
    (set! sample-pick.origins (list-remove sample-pick.origins origin))
    (set! sample-pick.origins (append sample-pick.origins (list origin)))))

(def clear-sample-filters ()
  (set! sample-pick.tags (list))
  (set! sample-pick.origins (list)))

(def set-search-filter (value)
  (if (and (= browser-view.tab "samples") (not (= value browser-view.search)))
    (clear-tags))
  (set! browser-view.search value))

(def tag-chip (tag)
  (let ((name (get tag :name))
      (selected (get tag :selected)))
    (button name
      :variant :ghost
      :background-color (if selected 
        :mixer-strip-selected-bg
        :mixer-control-bg
        )
      :color (if selected :fg :dimmer)
      :border-color (if selected :dim  :none)
      :height 1.0
      :padding 0.8532
      :font-size 12.0
      :corner-radius 13
      :on-click |x y r| (do 
        (set! browser-view.search "") 
        (toggle-tag name)
        ))))

(def origin-chip (origin)
  (let ((name (get origin :name))
      (selected (get origin :selected)))
    (button (get origin :label)
      :variant :ghost
      :background-color (if selected 
        :mixer-strip-selected-bg
        :mixer-control-bg
        )
      :color (if selected :fg :dimmer)
      :border-color (if selected :buffer-bg  :none)
      :height 1.0
      :padding 0.8532
      :font-size 12.0
      :corner-radius 24
      :on-click |x y r| (toggle-origin name))))

(def search-placeholder ()
  (if (= browser-view.tab "samples") "Search samples..."
    (if (= browser-view.tab "sounds") "Search sounds..."
    (if (= browser-view.tab "instruments") "Search instruments..."
      (if (= browser-view.tab "audio-fx") "Search audio effects..."
        (if (= browser-view.tab "midi-fx") "Search MIDI effects..."
          (if (= browser-view.tab "presets") "Search presets..."
            (if (= browser-view.tab "packages") "Search packages..."
              "Search projects..."))))))))

(def empty-message (message)
  (box :width :fill :height :fill :padding 1
    (label message
      :font-size 10
      :color :gray
      :bg :transparent)))

(def select-audio-effect (item)
  (let ((kind (get item :kind)) (label (get item :label)))
    (do
      ;; Only custom (dsp.lisp-backed) effects can be forked; builtins are Rust.
      (set! instrument-pick.audio-effect
        (if (= kind "custom-audio-effect") (get item :name) ""))
      (if (= kind "header")
        false
        (if (or (= kind "builtin-audio-effect") (= kind "custom-audio-effect"))
          (status (str label))
          (status "Choose an effect"))))))

(def fork-selected-audio-effect ()
  (if (= instrument-pick.audio-effect "")
    (status "Select a custom effect to fork")
    (do
      (clear-editor-name!)
      (host-command "enter-fork-effect-editor"
        (dict :source instrument-pick.audio-effect)))))

(def enter-new-effect-editor ()
  (clear-editor-name!)
  (host-command "enter-new-effect-editor" (dict)))

(def activate-audio-effect (item)
  (let ((kind (get item :kind)) (name (get item :name)))
    (if (= kind "header")
      false
      (if (= kind "builtin-audio-effect")
        (do
          (if (eseq.seq-core-state/seq-has-selected-bus?)
            (host-command "add-builtin-bus-effect" (dict :bus eseq.seq-core-state/selected-bus :name name))
            (host-command "add-builtin-effect" (dict :name name)))
          (eseq.seq-panels/seq-show-fx-lower-panel)
          (status (str "Add built-in effect: " name)))
        (if (= kind "custom-audio-effect")
          (do
            (if (eseq.seq-core-state/seq-has-selected-bus?)
              (host-command "add-bus-effect" (dict :bus eseq.seq-core-state/selected-bus :name name))
              (host-command "add-effect" (dict :name name)))
            (eseq.seq-panels/seq-show-fx-lower-panel)
            (status (str "Add effect: " name)))
          (status "Choose an effect"))))))

(def select-midi-effect (item)
  (let ((kind (get item :kind)) (label (get item :label)))
    (if (= kind "midi-effect")
      (status (str label))
      (status "Choose a MIDI effect"))))

(def activate-midi-effect (item)
  (let ((kind (get item :kind)) (name (get item :name)))
    (if (= kind "midi-effect")
      (do
        (host-command "add-midi-fx" (dict :name name))
        (eseq.seq-panels/seq-show-fx-lower-panel)
        (status (str "Add MIDI FX: " name)))
      (status "Choose a MIDI effect"))))

;; The drum rack slot whose presets the Presets tab shows instead of the
;; track's: the slot selected as the delete target (only that explicit
;; selection opts the browser into a slot's presets), or nil.
(def selected-rack-preset-context ()
  (nth (filter |s| (let ((d s.device)) (and d d.delete-target))
         browser.rack-slots)
       0))

(def browser-preset-items ()
  (let ((s (selected-rack-preset-context)))
    (if s s.presets browser.presets)))

;; The presets the user saved themselves, listed under Library (the rest are
;; Factory).
(def browser-user-preset-items ()
  (let ((s (selected-rack-preset-context)))
    (if s s.user-presets browser.user-presets)))

(def browser-loaded-preset ()
  (let ((s (selected-rack-preset-context)))
    (if s s.preset browser.preset)))

;; The instrument whose presets the list shows, stamped on each row so a dragged
;; preset knows what to load; "" for a rack track's rack presets.
(def browser-preset-instrument ()
  (let ((s (selected-rack-preset-context)))
    (if s
      s.instrument
      (if (instrument-type? browser.track "custom")
        browser.instrument
        ""))))

(def load-preset (name)
  (let ((s (selected-rack-preset-context)))
    (host-command "load-instrument-preset"
      (if s
        (dict :name name :track s.device.track.index :rack-slot s.index
          :instrument s.instrument)
        (dict :name name))))
  (eseq.seq-panels/seq-show-fx-lower-panel)
  (status (str "Load preset: " name)))

(def load-project (name)
  (host-command "load-project" (dict :name name))
  (reset-to-audition)
  (status (str "Open project: " name)))

;; ── Search bar widget ──

(def search-header ()
  (box :key "header" :width :fill :height 2.0 :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (text-input
        :key "search-input"
        :width :fill
        :value browser-view.search
        :placeholder (search-placeholder)
        :on-change (lambda (v) (set-search-filter v))
        :height 1.5
        :font-size 12
        (mag-glass)))))

(def instrument-header ()
  (box :key "instrument-header" :width :fill :height 1.1 :padding 0.15
    (label (let ((s (selected-rack-preset-context)))
      (if s s.instrument-label
        (if (= browser.instrument-label "") "Instrument" browser.instrument-label)))
      :font-size 12
      :color :white
      :bg :transparent)))

(def create-header ()
  (box :key "create-header" :width :fill :height 1.1 :padding 0.15
    (label "Create track"
      :font-size 12
      :color :white
      :bg :transparent)))

(def project-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.2
      (h-stack :width :fill :gap 0.5 :align :center
        (text-input
          :flex 1
          :value browser-view.search
          :placeholder "Search projects..."
          :on-change (lambda (v) (set-search-filter v))
          :height 1.5
          :font-size 12
          (mag-glass))
        (box :bg :dark-gray :width 8.2 :height 1.5 :align :center
          (label "projects"
            :font-size 9
            :color :white
            :bg :transparent)))
      (label
        (str "Current project: "
          (if (= project.name "") "none" project.name))
        :font-size 9
        :color :gray
        :bg :transparent))))

;; Saved-instrument tier filter: "" shows the shipped Factory tree, the
;; user's Library and every installed package; "factory", "user" or "pkg"
;; narrows to one. Single-select so a second click on the active chip clears
;; it (instrument-pick.origin).

(def toggle-instrument-origin (origin)
  (set! instrument-pick.origin
    (if (= instrument-pick.origin origin) "" origin)))

(def instrument-origin-chip (origin label)
  (let ((selected (= instrument-pick.origin origin)))
    (button label
      :variant :ghost
      :background-color (if selected
        :mixer-strip-selected-bg
        :mixer-control-bg)
      :color (if selected :fg :gray)
      :border-color (if selected :buffer-bg :none)
      :height 1.0
      :padding 0.8532
      :font-size 12.0
      :corner-radius 24
      :on-click |x y r| (toggle-instrument-origin origin))))

;; Favorites: right-click an instrument row to heart it; the heart chip
;; narrows every section to hearted rows (it combines with the tier chips and
;; the search). The set lives in favorites.json, owned by the host
;; (src/ui/instrument_favorites.rs); `instrument-pick.favorites-epoch` only
;; exists so a toggle re-renders the tree, which reads the file-backed set
;; natively.

(def instrument-favorites-chip ()
  (let ((selected instrument-pick.favorites-only))
    (button "Favorites"
      :key "instrument-favorites-chip"
      :icon :heart
      :icon-color (if selected :red :gray)
      :width 10.5
      :variant :ghost
      :background-color (if selected
        :mixer-strip-selected-bg
        :mixer-control-bg)
      :color (if selected :fg :gray)
      :border-color (if selected :buffer-bg :none)
      :height 1.0
      :font-size 12.0
      :corner-radius 24
      :on-click |x y r| (set! instrument-pick.favorites-only (not instrument-pick.favorites-only)))))

(def instrument-origin-filter-row ()
  (box :key "instrument-origin-filter" :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0.35
    (h-stack :width :fill :gap 0.25 :align :center
      (instrument-origin-chip "factory" "Factory")
      (instrument-origin-chip "user" "Library")
      (instrument-origin-chip "pkg" "Packages"))))

(def create-items ()
  (do
    instrument-pick.favorites-epoch
    ;; Bumped by the host when an instrument/effect folder changes on disk, so
    ;; a folder a coding agent just wrote lists without a restart.
    browser.library-epoch
    (seq-saved-instrument-tree browser-view.search browser.engines
      instrument-pick.origin instrument-pick.favorites-only)))

(def toggle-instrument-favorite (item)
  (let ((now (seq-toggle-favorite-instrument (get item :favorite-id))))
    (do
      (set! instrument-pick.favorites-epoch (+ instrument-pick.favorites-epoch 1))
      (status (str (if now "Added to favorites: " "Removed from favorites: ")
                   (get item :label))))))

;; Only saved instruments (Factory / Library / package / Engines rows) can be
;; hearted: builtins, headers and folders open no menu.
(def open-instrument-menu (event)
  (let ((item (get event :item)))
    (if (and (not (= item nil)) (= (get item :kind) "instrument") (get item :favorite-id))
      (do
        (set! instrument-menu.item item)
        (open-menu! instrument-menu event))
      nil)))

(def instrument-context-menu ()
  (menu-of instrument-menu
    (menu-item
      (if (and instrument-menu.item
               (seq-favorite-instrument? (get instrument-menu.item :favorite-id)))
        "Remove from Favorites"
        "Add to Favorites")
      :key "instrument-menu-favorite"
      :on-select (lambda (event)
        (do
          (set! instrument-menu.open false)
          (toggle-instrument-favorite instrument-menu.item))))))

(def enter-new-instrument-editor ()
  (clear-editor-name!)
  (host-command "enter-new-instrument-editor" (dict)))

(def add-builtin-instrument-track (name)
  (if (or (= name "sampler") (= name "modulator") (= name "rack") (= name "layer-rack"))
    (eseq.seq-panels/seq-show-fx-lower-panel)
    nil)
  (if (= name "sampler")
    (add-sampler-track)
    (if (= name "modulator")
      (add-modulator-track)
      (if (= name "rack")
        (add-rack-track)
        (if (= name "layer-rack")
          (add-new-layer-rack-track)
          (status "Choose an instrument"))))))

(def activate-builtin-instrument (name)
  ;; With a drum rack selected selection.track is whatever was selected
  ;; before the rack, so converting it in place would rewrite an unrelated
  ;; track behind the visible selection: add a new track instead.
  (if (and (= name "sampler")
           (< (selected-drum-rack-id) 0)
           (replaceable? selection.track))
    (swap-track-builtin-instrument (current-index) name false)
    (add-builtin-instrument-track name)))

(def select-create-item (item)
  (let ((kind (get item :kind)))
    (if (= kind "header")
      false
      (if (= kind "builtin-instrument")
        (activate-builtin-instrument (get item :name))
        (if (= kind "sampler")
          (enter-create-sampler-mode)
          (if (= kind "new-instrument")
            (enter-new-instrument-editor)
            (if (= kind "instrument")
              (activate-instrument (get item :name))
              (status "Choose an instrument"))))))))

(def focus-create-item (item)
  (let ((kind (get item :kind)))
    (do
      (set! instrument-pick.instrument
        (if (= kind "instrument") (get item :name) ""))
      (if (= kind "header")
        false
        (if (or (= kind "instrument") (= kind "builtin-instrument"))
          (status (str (get item :label)))
          (if (= kind "folder")
            (status (str "Folder: " (get item :label)))
            (status "Choose an instrument")))))))

(def fork-selected-instrument ()
  (if (= instrument-pick.instrument "")
    (status "Select a saved instrument to fork")
    (do
      (clear-editor-name!)
      (host-command "enter-fork-instrument-editor"
        (dict :source instrument-pick.instrument)))))

(def drop-instrument-on-folder (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((name (get payload :name))
          (folder (get target :folder)))
      (if (and (= (get payload :kind) "instrument") name folder)
        (do
          (host-command "move-saved-instrument" (dict :name name :folder folder))
          (status (str "Move instrument to " (get target :label))))
        ;; Builtins are not files on disk, so there is nothing to move.
        (if (= (get payload :kind) "builtin-instrument")
          (status "Builtin instruments cannot be moved into a folder")
          (status "Drop instruments onto a folder"))))))

;; The instrument the selected track plays, as the tree rows' :instrument-id.
;; The tree greys that row (Finder's unfocused selection), leaving blue to
;; mean "Enter loads this". The load itself is announced by the app toast;
;; the row being compiled carries a spinner (:loading-value).
(def current-track-instrument-id ()
  (let ((t selection.track))
    (if t t.instrument-id "")))

(def create-picker ()
  (v-stack :key "create-picker-panel" :width :fill :gap 0.5 :flex 1
    (instrument-origin-filter-row)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (scroll :key "create-picker-scroll" :width :fill :flex 1
        (tree
          :key "create-picker-tree"
          :width :fill
          :background-color :buffer-bg
          :items (create-items)
          :expand-all (not (= browser-view.search ""))
          :drag-type "instrument"
          :drop-types (list "instrument")
          :on-drop (lambda (event) (drop-instrument-on-folder event))
          :on-select (lambda (item) (focus-create-item item))
          :on-activate (lambda (item) (select-create-item item))
          :current-key "instrument-id"
          :current-value (current-track-instrument-id)
          :loading-value instrument-pick.loading)))))

(def tab-items ()
  (list
    (dict :name "samples" :label "Samples" :icon :waveform)
    (dict :name "sounds" :label "Sounds" :icon :piano)
    (dict :name "kits" :label "Kits" :icon :sampler)
    (dict :name "instruments" :label "Instruments" :icon :piano)
    (dict :name "audio-fx" :label "Audio FX" :icon :drop)
    (dict :name "midi-fx" :label "MIDI FX" :icon :note-arrow)
    (dict :name "presets" :label "Presets" :icon :dial)
    (dict :name "packages" :label "Packages" :icon :project)
    (dict :name "projects" :label "Projects" :icon :project)))

;; A saved Sound or kit (preset-file) as a tree row; a dragged row carries
;; its :path.
(def preset-row (p)
  (dict :kind p.type :icon p.icon :label p.name :name p.name :path p.path
        :pads p.pads :author p.author :tags p.tags))

;; The saved Sounds or kits whose name has the search text, as tree rows.
(def preset-rows (files)
  (let ((search (string-downcase browser-view.search)))
    (map preset-row
      (if (= search "")
        files
        (filter |p| (string-contains? (string-downcase p.name) search) files)))))

(def visible-sounds ()
  (preset-rows browser.sound-presets))

(def load-sound (item)
  (let ((rack-id (selected-drum-rack-id)))
    (if (>= rack-id 0)
      (host-command "audition-sound-on-rack"
        (dict :group-id rack-id :path (get item :path)))
      (if (no-tracks?)
        (status "Create a track before loading a Sound")
        (host-command "load-sound-onto-track"
          (dict :track (current-index) :path (get item :path)))))))

(def sounds-panel ()
  (let ((items (visible-sounds)))
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (if (= (len items) 0)
        (empty-message "No Sounds found. Drag an instrument preset onto the Sounds tab to add one.")
        (scroll :key "sounds-tab-scroll" :width :fill :flex 1
          (tree :key "sounds-tab-tree"
            :width :fill
            :background-color :buffer-bg
            :items items
            :font-size 12
            :focusable true
            :drag-type "sound"
            :on-activate (lambda (item) (load-sound item))))))))

;; ── Kits (docs/drum-rack-v2-spec.md, "Polish") ──────────────────────────
;; A kit is a drum rack saved as a browser object: group config + one Sound
;; per pad, no patterns. Activating one replaces the selected drum rack's kit;
;; with no rack selected it builds a new rack beside the existing tracks.

(def visible-kits ()
  (preset-rows browser.kit-presets))

(def load-kit (item)
  (let ((rack-id (selected-drum-rack-id)))
    (if (>= rack-id 0)
      (host-command "load-kit" (dict :path (get item :path) :group-id rack-id))
      (host-command "load-kit" (dict :path (get item :path))))))

(def enter-kit-save (g)
  (set! browser-view.search "")
  (set! browser-view.mode "audition")
  (set! preset-save.open false)
  (set! kit-save.group-id g.gid)
  (set! kit-save.name g.name)
  ;; Default: every scene this rack actually plays. A LEGACY rack (no bank)
  ;; answers true for every scene and the export drops the empty ones itself.
  (set! kit-save.scenes
    (map (lambda (s) s.index)
      (filter (lambda (s) (eseq.drum-rack-v2/scene-plays-clip? g s)) (scenes))))
  (set! kit-save.open true)
  (select-tab "kits"))

(def kit-scene-selected? (i)
  (listed? i kit-save.scenes))

(def kit-toggle-scene (i)
  (let ((now (if (kit-scene-selected? i)
               (list-remove kit-save.scenes i)
               (append kit-save.scenes (list i)))))
    ;; Keep the selection in scene order: clip 1..n follow the project's scene
    ;; order, not the order the boxes were ticked.
    (set! kit-save.scenes
      (filter (lambda (s) (listed? s now)) (range 0 (len (scenes)))))))

(def exit-kit-save ()
  (set! kit-save.open false)
  (set! kit-save.scenes (list))
  (set! kit-save.group-id -1))

(def save-kit ()
  (if (= (len kit-save.name) 0)
    (status "Enter a kit name")
    (do
      (host-command "save-rack-as-kit"
        (dict :group-id kit-save.group-id
              :name kit-save.name
              :scenes kit-save.scenes
              :overwrite false))
      (exit-kit-save))))

;; One row per project scene. A ticked scene becomes a clip in the kit, named
;; after the scene; untick every scene to save the old kind of kit (pads and
;; bus chain only).
(def kit-scene-row (sc)
  (h-stack :key (str "kit-save-scene-" sc.index) :width :fill :gap 0.5 :align :center
    (toggle :value (kit-scene-selected? sc.index)
      :on-change (lambda (value) (kit-toggle-scene sc.index)))
    (label sc.name
      :font-size 11 :color :white :bg :transparent :flex 1)))

(def kit-scene-checklist ()
  (v-stack :width :fill :gap 0.3 :flex 1
    (label "Scenes to export as clips"
      :key "kit-save-scenes-title"
      :font-size 10 :color :dim :bg :transparent)
    (scroll :key "kit-save-scenes-scroll" :width :fill :flex 1
      (v-stack :width :fill :gap 0.15
        (each (scenes) |sc| (kit-scene-row sc))))))

(def kit-save-panel ()
  (box :key "kit-save-panel" :width :fill :padding 0.5 :flex 1
    (v-stack :width :fill :gap 0.5 :flex 1
      (h-stack :width :fill :gap 0.5 :align :center
        (label "Save Kit" :font-size 12 :color :white :bg :transparent)
        (box :flex 1 :height 0)
        (button "Cancel"
          :key "kit-save-cancel"
          :variant :ghost
          :width 5.5 :height 1.2 :font-size 9
          :on-click |x y r| (exit-kit-save)
          :color :gray))
      (text-input
        :key "kit-save-name"
        :width :fill
        :value kit-save.name
        :placeholder "kit name..."
        :on-change (lambda (value) (set! kit-save.name value))
        :height 1.5
        :font-size 12)
      (kit-scene-checklist)
      (button "Save Kit"
        :key "kit-save-confirm"
        :variant :primary
        :width 10 :height 1.2 :font-size 11
        :on-click |x y r| (save-kit)
        :color :white))))

(def kits-panel ()
  (if kit-save.open
    (kit-save-panel)
    (let ((items (visible-kits)))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (empty-message "No kits yet. Use the save icon in a drum rack's FX-panel header.")
          (scroll :key "kits-tab-scroll" :width :fill :flex 1
            (tree :key "kits-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :font-size 12
              :focusable true
              :drag-type "kit"
              :on-activate (lambda (item) (load-kit item)))))))))

(def drop-preset-on-sounds (event)
  (if (= (get event :drag-type) "instrument-preset")
    (let ((name (get (get event :payload) :label)))
      (if name
        (host-command "promote-preset-to-sound"
          (dict :track (current-index browser.track) :name name))
        (status "Drop a preset item onto Sounds")))
    false))

(def tab-button (name label icon)
  (button label
    :key (str "tab-" name)
    :variant :ghost
    :icon icon
    :active (= browser-view.tab name)
    :width :fill
    :height 1.45
    :font-size 11.5
    :h-align :left
    :background-color '(rgba 1 1 1 0.0)
    :active-background-color :dark-gray
    :border-color '(rgba 1 1 1 0.0)
    :highlight-color '(rgba 1 1 1 0.0)
    :shadow-color '(rgba 0 0 0 0.0)
    :corner-radius 16
    :drop-types (if (= name "sounds") (list "instrument-preset") (list))
    :drop-meta (dict :kind "browser-tab" :name name)
    :drop-hover-background-color '(rgba 0.15 0.45 0.70 0.28)
    :on-drop (lambda (event) (drop-preset-on-sounds event))
    :on-click |x y r| (select-tab name)
    :color :widget-label-fg
    :active-color :blue))

(def tab-rail ()
  (let ((tabs (tab-items)))
    (box :key "tabs" :width 12.5 :height :fill 
      (h-stack 
        :width :fill
        :gap 0
        :align :start
        (box :width 0.5)
        (v-stack :width 12.0 :gap 0.08
          (box :height 0.5)
          (each (range 0 (len tabs)) |i|
            (let ((tab (nth tabs i)))
              (tab-button
                (get tab :name)
                (get tab :label)
                (get tab :icon)))))))))

(def sample-preview-strip ()
  (preview-strip sample-preview "sample-" :buffer-bg (box :height 0)
    (lambda () (toggle-auto-preview))))

(def samples-panel-with-activation (tree-key activation)
  (let ((listing (seq-sample-browser browser-view.search sample-pick.tags sample-pick.origins)))
    (let ((tags (get listing :tags))
        (origins (get listing :origins))
        (items (get listing :items)))
      (v-stack :key "samples-browser-panel" :width :fill :gap 0.35 :flex 1
        ;; The filter chips size to their content but shrink (scrolling) once
        ;; the results list would drop below its :min-height.
        (box :key "sample-tag-filter" :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0.35 :shrink 1 :min-height 4
          (scroll :key "sample-tag-filter-scroll" :width :fill :fit-content true
            (v-stack :width :fill :gap 0.35
              (if (or (> (len sample-pick.tags) 0) (> (len sample-pick.origins) 0))
                (button "Clear"
                  :variant :ghost
                  :width :fill
                  :height 1.35
                  :border-color :transparent
                  :font-size 12
                  :on-click |x y r| (clear-sample-filters)
                  :color :white))
              (if (> (len origins) 0)
                (wrap :width :fill :gap 0.25 :row-gap 0.18 :align :center
                  (each (range 0 (len origins)) |i|
                    (origin-chip (nth origins i)))))
              (wrap :width :fill :gap 0.20 :row-gap 0.10 :align :center
                (each (range 0 (len tags)) |i|
                  (tag-chip (nth tags i)))))))
        (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
          ;; With no results only the empty message needs room, so the
          ;; chips can take the rest instead of scrolling.
          :min-height (if (= (len items) 0) 3 10)
          (if (= (len items) 0)
            (empty-message
              (if (and (= browser-view.search "") (= (len sample-pick.tags) 0) (= (len sample-pick.origins) 0))
                "Choose a tag or search samples."
                "No samples found."))
            (scroll :key "samples-tab-scroll" :width :fill :flex 1
              (tree
                :key tree-key
                :width :fill
                :focusable true
                :background-color :buffer-bg
                :items items
                :font-size 12
                :selected-path (sample-selected-path)
                :expand-all true
                :drag-type "sample"
                :on-select (lambda (item) (select-sample item))
                :on-cursor-change (lambda (item) (select-sample item))
                :on-activate activation
                :on-modified-activate (lambda (item) (modified-activate-sample item))))))
        (sample-preview-strip)))))

(def samples-panel ()
  (samples-panel-with-activation
    "samples-tab-tree"
    (lambda (item) (activate-sample item))))

;; Embedded patch-learning browser. `samples-only` is an explicit widget mode,
;; not a second browser implementation: search, tag filters, selection,
;; waveform loading, and auto-preview all stay on the same path as the main
;; browser. Tree activation is the double-click/Enter action and chooses the
;; target instead of loading it into a track.
(def learn-target-display-name (target-path target-name)
  (let ((name (if (= target-name "") (path-filename target-path) target-name))
        (max-chars 30))
    (if (> (len name) max-chars)
      (str (substring name 0 (- max-chars 1)) "…")
      name)))

(def sample-browser-widget (samples-only target-path target-name)
  (if samples-only
    (if (= target-path "")
      (v-stack :key "learn-target-picker" :width :fill :height :fill :gap 0.15 :flex 1
        (search-header)
        (samples-panel-with-activation
          "learn-target-samples-tree"
          (lambda (item) (choose-learn-target item))))
      (box :key "learn-target-row" :width :fill :height 3.2
        :background-color :buffer-bg :corner-radius 8 :padding 0.35
        (h-stack :width :fill :height :fill :gap 0.5 :align :center
          (v-stack :width 0 :flex 1 :gap 0.2
            (label "LEARN TARGET" :font-size 8 :color :gray :bg :transparent)
            (label (learn-target-display-name target-path target-name)
              :width :fill :font-size 11 :color :white :bg :transparent)
            (if sample-preview.buffer
              (waveform
                :height 1.25
                :header-height 0
                :bg :buffer-bg
                :waveform-color :dim
                :grid-major-color :transparent
                :grid-minor-color :transparent
                :view-start 0
                :view-duration (get sample-preview.buffer :duration)
                :selection-start 0
                :selection-end (get sample-preview.buffer :duration)
                :buffer sample-preview.buffer)
              (box :height 1.25)))
          (button "Change"
            :key "learn-target-change"
            :variant :secondary
            :width 5.5
            :height 1.3
            :font-size 9.5
            :on-click |x y r| (change-learn-target)
            :color :white))))
    (tabbed-content)))

(def instruments-filter-row ()
  (h-stack :key "instruments-filter-row" :width :fill :gap 0.25 :align :center
    (instrument-favorites-chip)))

(def instruments-panel ()
  (let ((items (create-items)))
   (v-stack :key "instrument-tab-panel" :width :fill :gap 0.1 :flex 1
    (instruments-filter-row)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
     (if (and instrument-pick.favorites-only (= (len items) 0))
      (empty-message "No favorites yet. Right-click an instrument to add it.")
      (scroll :key "instruments-tab-scroll" :width :fill :flex 1
        (tree
          :key "instruments-tab-tree"
          :width :fill
          :background-color :buffer-bg
          :items items
          :font-size 12
          :expand-all (if instrument-pick.favorites-only true (not (= browser-view.search "")))
          :focusable true
          :drag-type "instrument"
          :drop-types (list "instrument")
          :on-drop (lambda (event) (drop-instrument-on-folder event))
          :on-select (lambda (item) (focus-create-item item))
          :on-activate (lambda (item) (select-create-item item))
          :on-modified-activate (lambda (item) (select-create-item item))
          :on-right-click (lambda (event) (open-instrument-menu event))
          :current-key "instrument-id"
          :current-value (current-track-instrument-id)
          :loading-value instrument-pick.loading)))))))

(def audio-fx-toolbar ()
  (box :width :fill :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (button "+ New Effect"
        :variant :secondary
        :flex 1
        :height 1.3
        :font-size 10.5
        :on-click |x y r| (enter-new-effect-editor)
        :color :white)
      (button
        (if (= instrument-pick.audio-effect "")
          "Fork…"
          (str "Fork " instrument-pick.audio-effect))
        :variant :secondary
        :flex 1
        :height 1.3
        :font-size 10.5
        :on-click |x y r| (fork-selected-audio-effect)
        :color :white))))

(def audio-fx-panel ()
  (let ((items (do browser.library-epoch (seq-audio-effect-tree browser-view.search))))
    (v-stack :key "audio-fx-tab-panel" :width :fill :gap 0.5 :flex 1
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (empty-message "No audio effects found.")
          (scroll :key "audio-fx-tab-scroll" :width :fill :flex 1
            (tree
              :key "audio-fx-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :expand-all (not (= browser-view.search ""))
              :font-size 12
              :focusable true
              :drag-type "audio-effect"
              :on-select (lambda (item) (select-audio-effect item))
              :on-cursor-change (lambda (item) (select-audio-effect item))
              :on-activate (lambda (item) (activate-audio-effect item))
              :on-modified-activate (lambda (item) (activate-audio-effect item)))))))))

(def midi-fx-panel ()
  (let ((items (seq-midi-effect-tree browser-view.search)))
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (if (= (len items) 0)
        (empty-message "No MIDI effects found.")
        (scroll :key "midi-fx-tab-scroll" :width :fill :flex 1
          (tree
            :key "midi-fx-tab-tree"
            :width :fill
            :background-color :buffer-bg
            :items items
            :font-size 12
            :expand-all (not (= browser-view.search ""))
            :focusable true
            :drag-type "midi-effect"
            :on-select (lambda (item) (select-midi-effect item))
            :on-cursor-change (lambda (item) (select-midi-effect item))
            :on-activate (lambda (item) (activate-midi-effect item))
            :on-modified-activate (lambda (item) (activate-midi-effect item))))))))

(def presets-tab-panel ()
  (v-stack :key "presets-tab-panel" :width :fill :gap 0.22 :padding 0.25 :flex 1
    (instrument-header)
    (if (= browser.instrument-kind "instrument")
      (let ((items (seq-preset-tree (browser-preset-items) browser-view.search
                     (browser-preset-instrument) (browser-user-preset-items))))
        (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
          (if (= (len items) 0)
            (empty-message "No presets found.")
            (scroll :key "presets-tab-scroll" :width :fill :flex 1
              (tree
                :key "presets-tab-tree"
                :width :fill
                :background-color :buffer-bg
                ;; Like the instrument and effect lists, a click only
                ;; selects (so the row can be dragged); double-click or
                ;; Enter loads it onto the current instrument.
                :items items
                :font-size 12
                :current-key "label"
                :current-value (browser-loaded-preset)
                :expand-all false
                :focusable true
                :drag-type "instrument-preset"
                :on-activate (lambda (item) (load-preset (get item :label))))))))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (empty-message "Presets are available for instrument tracks.")))))

;; Rows of the Packages tree (see `seq-package-tree`): "module" rows carry
;; :module plus, when the module defines instance kinds, :kinds (list of
;; {:id :name}) and :instance-count, and expand to one "instance" row per
;; instance (:instance-id :kind-id :owner :owner-rack :registered?). An
;; "orphan" row groups instances whose package is missing (or whose kind
;; lives in project code) at the end of Loaded.
(def package-item-attachable? (item)
  (let ((module (get item :module))
        (kind (get item :kind)))
    (and (not (= module nil))
         (not (= module ""))
         (or (= kind "module") (= kind "package")))))

(def package-item-kinds (item)
  (let ((kinds (get item :kinds)))
    (if (= kinds nil) (list) kinds)))

(def package-item-instance-count (item)
  (let ((count (get item :instance-count)))
    (if (= count nil) 0 count)))

(def instance-item? (item)
  (= (get item :kind) "instance"))

(def describe-package-item (item)
  (let ((kind (get item :kind))
        (label (get item :label)))
    (if (= kind "module")
      (status (str (get item :module)
        (if (get item :attached?) "  (attached to project)" "")
        (if (get item :always?) "  (always loaded)" "")
        (if (> (package-item-instance-count item) 0)
          (str "  " (package-item-instance-count item) " instance"
            (if (= (package-item-instance-count item) 1) "" "s"))
          "")))
      (if (= kind "instance")
        (status (str label ": " (get item :kind-id) " instance, owned by " (get item :owner)
          (if (get item :registered?) "" " (its package is not loaded)")))
        (if (= kind "package")
          (status (str label " " (get item :detail)))
          (if (= kind "file")
            (status (str label ": no (module ...) header, so it cannot be attached"))
            (status (str label))))))))

(def new-instance-of (item kind-id group-id)
  (host-command "packages-new-instance"
    (if (< group-id 0)
      (dict :module (get item :module) :kind kind-id)
      (dict :module (get item :module) :kind kind-id :group-id group-id))))

;; Enter or double-click (instance-kinds spec §8.3). An instance row opens
;; its tab (a placeholder row only says why it cannot). A module row: no kinds -> attach it (the host loads the module
;; first, writes the import line once it evaluates, and answers "already
;; attached" for a second press); exactly one kind and no instances ->
;; create the first instance, attaching first. A module row with instances
;; is a parent: its first click already toggled it open or shut, which is
;; the whole gesture. Package and folder rows only toggle.
(def activate-package-item (item)
  (let ((kind (get item :kind)))
    (if (= kind "instance")
      ;; A placeholder (its kind not registered) has no view to open; its
      ;; menu leaves Open out too.
      (if (get item :registered?)
        (host-command "instance-open" (dict :id (get item :instance-id)))
        (status (str (get item :label) ": its package is not loaded, so it has no view to open")))
      (if (= kind "module")
        (if (> (package-item-instance-count item) 0)
          nil
          (if (= (len (package-item-kinds item)) 1)
            (new-instance-of item (get (nth (package-item-kinds item) 0) :id) -1)
            (host-command "packages-attach" (dict :module (get item :module)))))
        (if (and (= kind "package") (= (get item :children) nil)
                 (package-item-attachable? item))
          ;; A package with no source rows to expand attaches its entry.
          (host-command "packages-attach" (dict :module (get item :module)))
          (if (= kind "file")
            (status "This file has no (module ...) header, so it cannot be attached")
            nil))))))

(def open-package-menu (event)
  (let ((item (get event :item)))
    (if (and (not (= item nil))
             (not (= (get item :kind) "header"))
             (not (= (get item :kind) "folder")))
      (do
        (set! package-menu.item item)
        (open-menu! package-menu event))
      nil)))

;; `icon` is a `button-icon` name or nil; `group` sections the menu, and
;; `package-menu-rows` puts a divider wherever it changes.
(def package-action (id key label icon group)
  (dict :id id :key key :label label :icon icon :group group))

(def rack-groups ()
  (filter |g| g.rack (groups)))

;; Instance row: Open, Rename, Duplicate, Move to <rack> per other rack /
;; Give back to project, Delete. A placeholder (kind not loaded) has no view
;; to open and cannot be duplicated until its package comes back.
(def instance-menu-actions (item)
  (let ((owner-rack (get item :owner-rack))
        (live (get item :registered?)))
    (append
      (append
        (if live
          (list (package-action :open "open" "Open" :document :edit)
                (package-action :rename "rename" "Rename" :pencil :edit)
                (package-action :duplicate "duplicate" "Duplicate" nil :edit))
          (list (package-action :rename "rename" "Rename" :pencil :edit)))
        (append
          (map (lambda (g)
                 (dict :id :move-to-rack
                       :key (str "move-" g.gid)
                       :group-id g.gid
                       :label (str "Move to " g.name)
                       :icon nil
                       :group :move))
            (filter |g| (not (= g.gid owner-rack)) (rack-groups)))
          (if (= owner-rack nil)
            (list)
            (list (package-action :give-back "give-back" "Give back to project" nil :move)))))
      (list (package-action :delete-instance "delete" "Delete" nil :delete)))))

;; Module row: `New <kind>` per kind first (attaching first if needed) --
;; the reason a kind module is in the tree -- then Attach/Remove + Always
;; Load, then the source actions, each section divided from the next.
(def module-menu-actions (item)
  (append
    (append
      (map (lambda (kind)
             (dict :id :new-instance
                   :key (str "new-" (get kind :id))
                   :kind-id (get kind :id)
                   :label (str "New " (get kind :name))
                   :icon :plus
                   :group :create))
        (package-item-kinds item))
      (if (package-item-attachable? item)
        (list
          (if (get item :attached?)
            (package-action :detach "detach" "Remove from Project" :unlink :load)
            (package-action :attach "attach" "Attach to Project" :link :load))
          (if (get item :always?)
            (package-action :stop-always "stop-always" "Stop Always Loading" :bookmark :load)
            (package-action :always "always" "Always Load" :bookmark :load)))
        (list)))
    (if (and (not (= (get item :path) nil)) (not (= (get item :kind) "package")))
      (append
        (list (if (get item :read-only?)
                (package-action :view "view" "View Source" :document :source)
                (package-action :view "view" "Edit Source" :pencil :source)))
        (if (and (get item :read-only?) (package-item-attachable? item))
          (list (package-action :copy "copy" "Copy to Local" :download :source))
          (list)))
      (list))))

(def package-menu-actions ()
  (let ((item package-menu.item))
    (if (= item nil)
      (list)
      (if (instance-item? item)
        (instance-menu-actions item)
        (module-menu-actions item)))))

(def begin-instance-rename (item)
  (do
    (set! instance-rename.draft (get item :label))
    (set! instance-rename.target (get item :instance-id))))

(def cancel-instance-rename ()
  (set! instance-rename.target -1))

(def commit-instance-rename ()
  (if (< instance-rename.target 0)
    nil
    (do
      (if (> (len instance-rename.draft) 0)
        (host-command "instance-rename"
          (dict :id instance-rename.target :label instance-rename.draft))
        nil)
      (set! instance-rename.target -1))))

(def instance-rename-panel ()
  (box :key "instance-rename-panel" :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.4
      (text-input
        :key "instance-rename-name"
        :width :fill
        :value instance-rename.draft
        :placeholder "instance name..."
        :auto-focus true
        :select-all-on-focus true
        :on-change (lambda (value) (set! instance-rename.draft value))
        :on-submit (lambda () (commit-instance-rename))
        :on-cancel (lambda () (cancel-instance-rename))
        :height 1.5
        :font-size 12)
      (h-stack :width :fill :gap 0.5 :align :center
        (button "Rename"
          :key "instance-rename-confirm"
          :variant :primary
          :flex 1 :height 1.2 :font-size 10
          :on-click |x y r| (commit-instance-rename)
          :color :white)
        (button "Cancel"
          :key "instance-rename-cancel"
          :variant :ghost
          :flex 1 :height 1.2 :font-size 10
          :on-click |x y r| (cancel-instance-rename)
          :color :gray)))))

(def select-instance-menu-action (item action)
  (let ((id (get action :id))
        (instance (get item :instance-id)))
    (if (= id :open)
      (host-command "instance-open" (dict :id instance))
      (if (= id :rename)
        (begin-instance-rename item)
        (if (= id :duplicate)
          (host-command "instance-duplicate" (dict :id instance))
          (if (= id :move-to-rack)
            (host-command "instance-move" (dict :id instance :group-id (get action :group-id)))
            (if (= id :give-back)
              (host-command "instance-move" (dict :id instance))
              (if (= id :delete-instance)
                (host-command "instance-delete" (dict :id instance))
                nil))))))))

(def select-module-menu-action (item action)
  (let ((id (get action :id)))
    (if (= id :new-instance)
      (new-instance-of item (get action :kind-id) -1)
      (if (= id :attach)
        (host-command "packages-attach" (dict :module (get item :module)))
        (if (= id :detach)
          ;; The host asks first when the module's kinds have instances.
          (host-command "packages-detach" (dict :module (get item :module)))
          (if (= id :always)
            (host-command "packages-always-load" (dict :module (get item :module)))
            (if (= id :stop-always)
              (host-command "packages-stop-always-load" (dict :module (get item :module)))
              (if (= id :view)
                (host-command "packages-open-source"
                  (dict :path (get item :path) :read-only (get item :read-only?)))
                (if (= id :copy)
                  (host-command "packages-copy-to-local" (dict :path (get item :path)))
                  nil)))))))))

(def select-package-menu-action (action)
  (let ((item package-menu.item))
    (do
      (set! package-menu.open false)
      (if (instance-item? item)
        (select-instance-menu-action item action)
        (select-module-menu-action item action)))))

;; The menu's rows: the actions with a divider before each one whose
;; :group differs from the action above it.
(def package-menu-rows (actions)
  (reduce
    (lambda (rows action)
      (if (and (> (len rows) 0)
               (not (= (get (nth rows (- (len rows) 1)) :group) (get action :group))))
        (append rows (list (dict :separator? true :key (get action :key)) action))
        (append rows (list action))))
    (list)
    actions))

(def package-context-menu ()
  (apply menu-of package-menu
    (each (package-menu-rows (if package-menu.open (package-menu-actions) (list))) |action|
      (if (get action :separator?)
        (menu-separator :key (str "package-menu-sep-" (get action :key)))
        (menu-item (get action :label)
          :key (str "package-menu-" (get action :key))
          :icon (get action :icon)
          :on-select (lambda (event) (select-package-menu-action action)))))))

(def begin-new-package ()
  (do
    (set! package-draft.name "")
    (set! package-draft.open true)))

(def cancel-new-package ()
  (set! package-draft.open false))

(def create-new-package ()
  (if (= (len package-draft.name) 0)
    (status "Type a package name, for example euclid or my.euclid.sparse")
    (do
      (host-command "packages-create" (dict :name package-draft.name))
      (set! package-draft.open false))))

(def package-new-panel ()
  (box :key "package-new-panel" :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.4
      (text-input
        :key "package-new-name"
        :width :fill
        :value package-draft.name
        :placeholder "name (euclid or my.euclid.sparse)..."
        :auto-focus true
        :on-change (lambda (value) (set! package-draft.name value))
        :on-submit (lambda () (create-new-package))
        :on-cancel (lambda () (cancel-new-package))
        :height 1.5
        :font-size 12)
      (h-stack :width :fill :gap 0.5 :align :center
        (button "Create"
          :key "package-new-confirm"
          :variant :primary
          :flex 1 :height 1.2 :font-size 10
          :on-click |x y r| (create-new-package)
          :color :white)
        (button "Cancel"
          :key "package-new-cancel"
          :variant :ghost
          :flex 1 :height 1.2 :font-size 10
          :on-click |x y r| (cancel-new-package)
          :color :gray)))))

;; Packages tab: everything importable, sectioned Local / Installed /
;; Factory by `seq-package-tree` (src/ui/host_commands/packages.rs). It is
;; sugar over the textual import record: attach writes the (import …) line
;; into the project scratch, "Always Load" into ~/.eseq.d/init.lisp, and the
;; check / bookmark glyphs read those files back. The C-x p text view
;; drives the same host commands.
(def packages-tab-panel ()
  (let ((items (seq-package-tree browser-view.search project.instances)))
    (v-stack :key "packages-tab-panel" :width :fill :gap 0.5 :flex 1
      (if package-draft.open
        (package-new-panel)
        (box :width :fill :height 0))
      (if (>= instance-rename.target 0)
        (instance-rename-panel)
        (box :width :fill :height 0))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (empty-message "No packages found.")
          (scroll :key "packages-tab-scroll" :width :fill :flex 1
            (tree
              :key "packages-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :font-size 12
              :expand-all (not (= browser-view.search ""))
              :focusable true
              ;; A module row with instances is a parent; its double-click
              ;; still reaches `activate-package-item` (spec §8.3).
              :activate-parents true
              :on-select (lambda (item) (describe-package-item item))
              :on-cursor-change (lambda (item) (describe-package-item item))
              :on-activate (lambda (item) (activate-package-item item))
              :on-right-click (lambda (event) (open-package-menu event)))))))))

(def projects-tab-panel ()
  (let ((items (seq-project-tree browser-view.search)))
    (v-stack :key "projects-tab-panel" :width :fill :gap 0.5 :flex 1
      (box :width :fill :padding 0.25
        (h-stack :width :fill :gap 0.5 :align :center
          (button "New Project"
            :key "project-new-button"
            :variant :secondary
            :flex 1
            :height 1.3
            :font-size 10.5
            :on-click |x y r| (new-project)
            :color :white)))
      (box :width :fill :padding 0.25
        (label "Projects"
          :font-size 10
          :color :gray
          :bg :transparent))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (empty-message "No projects found.")
          (scroll :key "projects-tab-scroll" :width :fill :flex 1
            (tree
              :key "projects-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :font-size 12
              :selected-label project.name
              :expand-all false
              :focusable true
              :on-select (lambda (item) (load-project (get item :label)))
              :on-activate (lambda (item) (load-project (get item :label))))))))))

(def active-tab-panel ()
  (if (= browser-view.tab "samples") (samples-panel)
    (if (= browser-view.tab "sounds") (sounds-panel)
    (if (= browser-view.tab "kits") (kits-panel)
    (if (= browser-view.tab "instruments") (instruments-panel)
      (if (= browser-view.tab "audio-fx") (audio-fx-panel)
        (if (= browser-view.tab "midi-fx") (midi-fx-panel)
          (if (= browser-view.tab "presets") (presets-tab-panel)
            (if (= browser-view.tab "packages") (packages-tab-panel)
              (projects-tab-panel))))))))))

(def tabbed-content ()
  (h-stack :key "tabbed-content" :width :fill :gap 0.5 :flex 1 :align :stretch
    (tab-rail)
    (box 
      :width 0.2 
      :height :fill 
      :background-color :bg
      )
    (box
      :key "active-tab-panel" :width 0 :flex 1 :padding 0
      (v-stack
        :key "active-tab-column" :width :fill :height :fill :gap 0.15 :flex 1
        (search-header)
        (active-tab-panel)))))

(def main-panel ()
  (v-stack :key "tabbed-browser" :width :fill :height :fill :gap 0.45 :flex 1
    (tabbed-content)))

(def preset-search-bar ()
  (box :key "preset-search-bar" :width :fill :height 1.8 :padding 0.15
    (h-stack :width :fill :gap 0.5 :align :center
      (text-input
        :flex 1
        :value browser-view.preset-search
        :placeholder "Search presets..."
        :on-change (lambda (v) (set! browser-view.preset-search v))
        :height 1.5
        :font-size 12
        (mag-glass)))))

(def presets-panel ()
  (v-stack :key "preset-list-panel" :width :fill :gap 0.22 :flex 1
    (instrument-header)
    (preset-search-bar)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (scroll :key "preset-list-scroll" :width :fill :flex 1
        (tree
          :key "preset-list-tree"
          :width :fill
          :background-color :buffer-bg
          :items (seq-preset-tree (browser-preset-items) browser-view.preset-search
                   (browser-preset-instrument))
          :current-key "label"
          :current-value (browser-loaded-preset)
          :expand-all false
          :focusable true
          :drag-type "instrument-preset"
          :on-activate (lambda (item) (load-preset (get item :label))))))))

(def projects-panel ()
  (let ((items (seq-project-tree browser-view.search)))
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (if (= (len items) 0)
        (box :padding 1
          (label "No projects found."
            :font-size 10
            :color :gray
            :bg :transparent))
        (scroll :key "project-list-scroll" :width :fill :flex 1
          (tree
            :key "project-list-tree"
            :width :fill
            :background-color :buffer-bg
            :items items
            :selected-label project.name
            :expand-all false
            :on-select (lambda (item) (load-project (get item :label)))
            :on-activate (lambda (item) (load-project (get item :label)))))))))

;; ── Preset save sidebar ──

(def preset-save-mode? ()
  preset-save.open)

(def enter-preset-save ()
  (set! preset-save.name "")
  (set! preset-save.open true))

(def exit-preset-save ()
  (set! preset-save.open false))

(def preset-save-header ()
  (box :width :fill :padding 0.55
    (v-stack :width :fill :gap 0.4
      (h-stack :width :fill :gap 0.5 :align :center
        (label "Save Preset"
          :font-size 12
          :color :white
          :bg :transparent)
        (box :bg :dark-gray :width 6 :height 1.5 :align :center
          :on-click |x y r| (exit-preset-save)
          (label "cancel"
            :font-size 9
            :color :gray
            :bg :transparent)))
      (text-input
        :width :fill
        :value preset-save.name
        :placeholder "preset name..."
        :on-change (lambda (v) (set! preset-save.name v))
        :height 1.5
        :font-size 12)
      ;; Save as New button
      (button "Save as New"
        :variant :primary
        :width 10
        :height 1.2
        :font-size 11
        :on-click |x y r|
          (do
            (host-command "save-preset" (dict :name preset-save.name :overwrite false))
            (exit-preset-save))
        :color :white)
      ;; Overwrite button (only if a preset is currently loaded)
      (if (not (= browser.preset ""))
        (button (str "Overwrite: " browser.preset)
          :variant :secondary
          :width 16
          :height 1.2
          :font-size 10
          :on-click |x y r|
            (do
              (host-command "overwrite-preset" (dict))
              (exit-preset-save))
          :color :white)
        (box)))))

(def preset-save-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

;; ── Editor sidebar panels ──

(def editor-macro-action? ()
  (or (= editor.active-macro-action "save-to-library")
      (= editor.active-macro-action "fork")))

(def editor-macro-action-label ()
  (if (= editor.active-macro-action "fork")
    "Fork Macro"
    "Save Macro to Library"))

;; Fork is offered exactly when the primary button means "overwrite the shared
;; definition" — i.e. edit-instrument / edit-effect. It has nothing to fork from
;; in the new-* draft modes, and hides behind the macro action like everything
;; else in that stack.
(def editor-fork-available? ()
  (and (not (editor-macro-action?))
       (or (= editor.mode "edit-instrument")
           (= editor.mode "edit-effect"))))

(def editor-header ()
  (box :width :fill :padding 0.25 :height :fill
    (v-stack :width :fill :gap 0.4 :height :fill
      ;(h-stack :width :fill :gap 0.5 :align :center
      ;(label
      ;  (if (editor-macro-action?) "Defmacro"
      ;    (if (= editor.mode "new-instrument") "New Instrument"
      ;      (if (= editor.mode "edit-instrument")
      ;        (if (= editor.surface "code") "Edit Instrument (code)" "Edit Instrument")
      ;        (if (= editor.mode "new-effect") "New Effect"
      ;          (if (= editor.mode "edit-effect")
      ;            (if (= editor.surface "code") "Edit Effect (code)" "Edit Effect")
      ;            "Editor")))))
      ;  :font-size 12
      ;  :color :white
      ;  :bg :transparent))
      
      (if (editor-macro-action?)
        (v-stack :width :fill :gap 0.35
          (label "Current macro"
            :font-size 9
            :color :gray
            :bg :transparent)
          (label editor.active-macro
            :font-size 11
            :color :white
            :bg :transparent)
          (label "Action"
            :font-size 9
            :color :gray
            :bg :transparent)
          (label (editor-macro-action-label)
            :font-size 11
            :color :white
            :bg :transparent))
        (if (= editor.mode "new-instrument")
          (v-stack :width :fill :height :fill :gap 0.35
            
            (h-stack :width :fill :gap 0.35
              (button "Instrument"
                :background-color (if (= editor.run-mode "instrument") :mixer-strip-bg :transparent)
                :border-color :black
                :width 8.5
                :height 1.2
                :font-size 11
                :on-click |x y r|
                (set! editor.run-mode "instrument")
                :color (if (= editor.run-mode "instrument") :white :dimmer))
              (button "Free Patch"
                :background-color (if (= editor.run-mode "free_patch") :mixer-strip-bg :transparent)
                :border-color :black
                :border-width 1
                :width 8.5
                :height 1.2
                :font-size 11
                :on-click |x y r|
                (set! editor.run-mode "free_patch")
                :color (if (= editor.run-mode "free_patch") :white :dim)))
            (label "Save as"
              :font-size 12
              :color :gray
              :bg :transparent)
            (text-input
              :width :fill
              :value editor-draft.name
              :placeholder "instrument-name"
              :on-change (lambda (v) (set! editor-draft.name v))
              :height 1.5
              :font-size 12))
          (if (= editor.mode "new-effect")
            (v-stack :width :fill :gap 0.35
              (label "Draft patch"
                :font-size 9
                :color :gray
                :bg :transparent)
              (label (str "track " (+ (current-index) 1))
                :font-size 11
                :color :white
                :bg :transparent)
              (label "Save as"
                :font-size 9
                :color :gray
                :bg :transparent)
              (text-input
                :width :fill
                :value editor-draft.name
                :placeholder "effect-name"
                :on-change (lambda (v) (set! editor-draft.name v))
                :height 1.5
                :font-size 12))
            ;; For edit modes, show the file name
            (label editor.buffer
              :font-size 10
              :color :gray
              :bg :transparent))))
      ;; Status display
      (if editor.canceling
        (editor-status-row "Canceling..." :gray)
        (if (= editor.error "Preview compiling...")
          (editor-status-row editor.error :gray)
          (if (not (= editor.error ""))
            (label editor.error
              :font-size 9
              :color :red
              :bg :transparent)
            (box))))
      ;; Eval button (code editor only): compile + hot-swap the buffer
      (if (= editor.surface "code")
        (button "Eval (C-c C-c)"
          :variant :secondary
          :width 13
          :height 1.2
          :font-size 10
          :on-click |x y r|
          (host-command "evaluate-editor-source" (dict))
          :color :white)
        (box))
      ;; Open as patch (code editor, edit-existing): promote to the patch editor
      (if (and (= editor.surface "code")
          (or (= editor.mode "edit-instrument") (= editor.mode "edit-effect")))
        (button "Open as patch"
          :variant :secondary
          :width 13
          :height 1.2
          :font-size 10
          :on-click |x y r|
          (host-command "promote-editor-to-patch" (dict))
          :color :white)
        (box))
      ;; Eject to code (patch editor, edit-existing only)
      (if (and (= editor.surface "patch")
          (or (= editor.mode "edit-instrument") (= editor.mode "edit-effect")))
          (button "View code"
            :variant :ghost
            :corner-radius 16
            :width 13
            :font-size 13
            :on-click |x y r|
            (host-command "eject-editor-to-code" (dict))
            :color :white)
        (box))
      (box :width 1 :flex 1)
      ;; Save button
      (if (editor-busy?)
        (box :height 1.2)
        (h-stack :align :center :width :fill :gap 0.5 :padding 0.1
          (box :flex 1 :height 1)
          (box :bg :dark-gray :width 6 :height 1.5 :align :center
            :on-click |x y r|
            (if editor.canceling nil (host-command "cancel-editor" (dict)))
            (button "Cancel"
              :variant :secondary
              :font-size 13
              :width 8
              :color (if editor.canceling :white :white)
              ))          
          (box :width 0.7 :height 1)
          (button
            (if (editor-macro-action?)
              (editor-macro-action-label)
              (if (= editor.mode "new-instrument")
                "Finalize"
                (if (= editor.mode "new-effect")
                  "Save & Add"
                  "Save")))
            :variant :primary
            :width (if (editor-macro-action?) 14.5 10)
            :font-size 13
            :on-click |x y r|
            (if (editor-macro-action?)
              (host-command "save-active-editor-macro" (dict))
              (if (= editor.mode "new-instrument")
                (host-command "save-new-instrument" (dict :name editor-draft.name))
                (if (= editor.mode "edit-instrument")
                  (host-command "update-instrument" (dict :name browser.instrument))
                  (if (= editor.mode "new-effect")
                    (host-command "save-new-effect" (dict :name editor-draft.name))
                    (host-command "update-effect" (dict))))))
            :color :browser-primary-fg)
          ;; Fork sits next to the clobbering path on purpose: in edit modes the
          ;; primary button overwrites an instrument every project shares, and
          ;; the safe alternative should not require leaving the buffer.
          (if (editor-fork-available?)
            (button "Fork"
              :variant :secondary
              :font-size 13
              :on-click |x y r| (host-command "fork-editor-session" (dict))
              :color :white)
            (box))
          )            
        )
      )))

(def editor-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

;; ── Build widgets ──

(def build-widgets ()
  (do
    (sync-track-search)
    (if (editor-mode?)
      (list
        (editor-header)
        (editor-panel))
      (if (preset-save-mode?)
        (list
          (preset-save-header)
          (preset-save-panel))
        (list
          (tabbed-content))))))

;; ── Reactive rendering (like ui/main.lisp) ──

(def root-widget ()
  (v-stack :width :fill :height :fill :gap 0.4 :padding 0.15
    (build-widgets)
    (package-context-menu)
    (instrument-context-menu)))

(def refresh-buffer ()
  (render-widget-to-buffer "*samples*" (root-widget)))

;; Widget-only buffer: take the shared sequencer keymap (was an implicit host default).
(set-buffer-mode-for "*samples*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*samples*"
  (root-widget))

;; ── Entry point: just switch to the buffer ──

(def sample-browser-here ()
  (set! source-buffer (current-buffer-name))
  (set! browser-view.search "")
  (set! browser-view.mode "audition")
  (set! browser-view.tab (if (= browser.instrument-kind "instrument") "presets" "samples"))
  (switch-to-buffer "*samples*"))

(bind-key "C-x s" "sample-browser-here")
