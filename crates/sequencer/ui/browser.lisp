;; ui/browser.lisp — Sample browser mode for Metal Sequencer
;; C-x s to open, type to filter, Enter to audition, +/= to add track, q to quit
;; Uses tree widget inside scroll container for hierarchical browsing.

(module eseq.browser)

;; Migration aliases (module spec §10 step 2) for the names unconverted callers
;; still spell flat.  Six are lisp-side — mixer.lisp / sequencer.lisp route
;; track drops here, effects/instrument-panel.lisp and effects/sampler-panel.lisp
;; drop sounds and open the preset-save sidebar, transport.lisp opens the
;; project-save sheet — and `sample-browser-here` keeps its spelling for the
;; "C-x s" binding and src/ui/input.rs.  The rest are entry points src/ui/input.rs
;; and the Rust state_values tests drive by name.  Deleted as each consumer
;; converts.
;;
;; NOT aliased, deliberately: the six names in the `eseq.vanilla/` block below
;; (hazard i — production Rust *writes* them by bare spelling, so they stay flat
;; and are a host→script protocol, not this module's API).
(module-compat-alias sbrowser-activate-audio-effect activate-audio-effect)
(module-compat-alias sbrowser-activate-midi-effect activate-midi-effect)
(module-compat-alias sbrowser-activate-sample activate-sample)
(module-compat-alias sbrowser-active-tab-panel active-tab-panel)
(module-compat-alias sbrowser-active-tree-key active-tree-key)
(module-compat-alias sbrowser-add-layer-rack-track add-layer-rack-track)
(module-compat-alias sbrowser-add-rack-track add-rack-track)
(module-compat-alias sbrowser-add-sampler-track add-sampler-track)
(module-compat-alias sbrowser-add-selected-rack-layer add-selected-rack-layer)
(module-compat-alias sbrowser-audition audition)
(module-compat-alias sbrowser-build-widgets build-widgets)
(module-compat-alias sbrowser-create-items create-items)
(module-compat-alias sbrowser-drop-instrument-on-folder drop-instrument-on-folder)
(module-compat-alias sbrowser-drop-instrument-on-track drop-instrument-on-track)
(module-compat-alias sbrowser-drop-preset-on-sounds drop-preset-on-sounds)
(module-compat-alias sbrowser-drop-sample-on-track drop-sample-on-track)
(module-compat-alias sbrowser-drop-sound-on-track drop-sound-on-track)
(module-compat-alias sbrowser-editor-status-row editor-status-row)
(module-compat-alias sbrowser-enter-new-effect-editor enter-new-effect-editor)
(module-compat-alias sbrowser-enter-new-script enter-new-script)
(module-compat-alias sbrowser-enter-preset-save enter-preset-save)
(module-compat-alias sbrowser-filter search-filter)
(module-compat-alias sbrowser-focus-create-item focus-create-item)
(module-compat-alias sbrowser-fork-selected-audio-effect fork-selected-audio-effect)
(module-compat-alias sbrowser-fork-selected-instrument fork-selected-instrument)
(module-compat-alias sbrowser-header search-header)
(module-compat-alias sbrowser-list-contains? list-contains?)
(module-compat-alias sbrowser-new-project new-project)
(module-compat-alias sbrowser-next-tab next-tab)
(module-compat-alias sbrowser-open-project-save open-project-save)
(module-compat-alias sbrowser-preset-filter preset-filter)
(module-compat-alias sbrowser-project-save-mode? project-save-mode?)
(module-compat-alias sbrowser-refresh-buffer refresh-buffer)
(module-compat-alias sbrowser-sample-selected-path sample-selected-path)
(module-compat-alias sbrowser-select-audio-effect select-audio-effect)
(module-compat-alias sbrowser-select-create-item select-create-item)
(module-compat-alias sbrowser-select-midi-effect select-midi-effect)
(module-compat-alias sbrowser-select-sample select-sample)
(module-compat-alias sbrowser-select-tab select-tab)
(module-compat-alias sbrowser-selected-audio-effect-name selected-audio-effect-name)
(module-compat-alias sbrowser-selected-instrument-name selected-instrument-name)
(module-compat-alias sbrowser-selected-sample selected-sample)
(module-compat-alias sbrowser-selected-tags selected-tags)
(module-compat-alias sbrowser-tabbed-content tabbed-content)
(module-compat-alias sbrowser-tabs tab-rail)
(module-compat-alias sample-browser-here sample-browser-here)

(import eseq.track-collapse)

;; ── State ──
;; The `eseq.vanilla/` names below are the §3 cross-module def escape hatch
;; (spec §10 hazard i): production Rust emits `(set! <name> …)` lisp for each of
;; them — src/ui/event_loop.rs, src/ui/edit_sessions.rs and
;; src/ui/host_commands/{scripts,tracks,instrument_authoring}.rs — and the
;; late-binding heal is read-side only, so a pre-conversion *writer* is never
;; rescued.  Pinned flat, and given no compat alias.  Five of the six are
;; `defstate`s, so hazard (b) compounds: the registration key has to stay flat
;; too, which `Compiler::qualify_registration_name` now guarantees by stripping
;; an explicit `eseq.vanilla/` prefix (vanilla's registry keyspace *is* the flat
;; keyspace).  Inside this module a bare read/write still compiles to
;; Load/StoreState on that flat key, so both sides hit one node.
(def search-filter (state ""))
(def %source-buffer "")
(defstate %mode "audition")
(defstate eseq.vanilla/sbrowser-tab "samples")
(defstate %project-name "")
(defstate %last-track-index -1)
(defstate %last-sidebar-sample "")
(defstate selected-sample "")
(defstate selected-tags (list))
(defstate eseq.vanilla/sbrowser-auditioned-sample "")
(defstate eseq.vanilla/sbrowser-loading-instrument-name "")
;; Last instrument / custom effect highlighted in the browser tree. Fork acts on
;; it — the tree widget has no context menu, so the action lives in the panel
;; toolbar instead (docs/instrument-fork-spec.md §3.5).
(defstate selected-instrument-name "")
(defstate selected-audio-effect-name "")

;; Editor state for inline instrument/effect creation
(def eseq.vanilla/sbrowser-editor-name (state ""))
;; Preset save state
(defstate %preset-name "")
(defstate %preset-save-mode "")  ;; "" or "save-preset"
(defstate preset-filter "")
(defstate eseq.vanilla/sbrowser-script-name "")
(defstate eseq.vanilla/sbrowser-script-save-mode "")  ;; "" or "new-script"

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

(def %editor-busy? ()
  (or SEQ.editor-canceling
    (= SEQ.editor-error "Preview compiling...")))

(def %audition-mode? ()
  (= %mode "audition"))

(def %track-type-mode? ()
  (or (= %mode "track-type")
    (and (= SEQ.num-tracks 0) (= %mode "audition"))))

(def %create-sampler-mode? ()
  (= %mode "create-sampler"))

(def %project-browser-mode? ()
  (= %mode "project-browser"))

(def project-save-mode? ()
  (= %mode "project-save"))

(def %editor-mode? ()
  (not (= SEQ.editor-mode "")))

(def %create-mode? ()
  (or (%track-type-mode?) (%create-sampler-mode?)))

(def %mode-label ()
  (if (%audition-mode?) "audition" "create"))

(def %sync-track-search ()
  (let ((track-changed (not (= %last-track-index SEQ.sidebar-track-index)))
        (sample-changed (not (= %last-sidebar-sample SEQ.sidebar-selected-sample))))
  (if (or track-changed sample-changed)
    (do
      (set! %last-track-index SEQ.sidebar-track-index)
      (set! %last-sidebar-sample SEQ.sidebar-selected-sample)
      (if (and (%audition-mode?) (= SEQ.sidebar-kind "sampler"))
        (do
          (set! selected-sample SEQ.sidebar-selected-sample)
          (if (and (or sample-changed track-changed) (= sbrowser-auditioned-sample SEQ.sidebar-selected-sample))
            (set! sbrowser-auditioned-sample "")
            (do
              (set! sbrowser-auditioned-sample "")
              (if (= sbrowser-tab "samples")
                (set! search-filter ""))
              (set! selected-tags
                (if (= SEQ.sidebar-selected-sample "")
                  (list)
                  (seq-sample-tags-for-path SEQ.sidebar-selected-sample)))))))))))

(def %reset-to-audition ()
  (set! %mode "audition")
  (set! search-filter "")
  (set! selected-tags (list)))

(def %leave-create-mode ()
  (set! %mode "audition")
  (set! search-filter "")
  (set! selected-tags (list)))

(def %enter-create-track-mode ()
  (set! search-filter "")
  (set! %mode "audition")
  (set! sbrowser-tab "instruments"))

(def %toggle-create-track-mode ()
  (%enter-create-track-mode))

(def %enter-create-sampler-mode ()
  (set! search-filter "")
  (set! selected-tags (list))
  (set! %mode "create-sampler")
  (set! sbrowser-tab "samples")
  (status "Create sampler track: choose a sample"))

(def %open-project-browser ()
  (set! search-filter "")
  (set! %mode "audition")
  (set! sbrowser-tab "projects"))

(def open-project-save ()
  (if (= SEQ.current-project-name "")
    (do
      (set! search-filter "")
      (set! %project-name "")
      (set! %mode "project-save"))
    (do
      (host-command "save-project" (dict :name SEQ.current-project-name))
      (status (str "Save project: " SEQ.current-project-name)))))

(def new-project ()
  (host-command "new-project" (dict))
  (set! search-filter "")
  (set! sbrowser-tab "projects")
  (status "New project"))

(def %add-instrument-track (name)
  (set! sbrowser-loading-instrument-name name)
  (host-command "add-track-instrument" (dict :name name))
  (status (str "Loading instrument: " name)))

(def %swap-track-instrument (track name)
  (if (or (= name nil) (= name ""))
    (status "Drop an instrument, not a folder")
    (if (seq-track-replaceable-instrument? track)
      (do
        (set! sbrowser-loading-instrument-name name)
        (host-command "swap-track-instrument" (dict :track track :name name))
        (status (str "Loading instrument swap: " name)))
      (status "Saved instruments can replace sampler or custom instrument tracks"))))

(def %swap-track-builtin-instrument (track name)
  (if (and (= name "sampler") (seq-track-replaceable-instrument? track))
    (do
      (host-command "swap-track-builtin-instrument" (dict :track track :name name))
      (set! sbrowser-tab "samples")
      (status "Loading sampler"))
    (status "Only sampler conversion is supported")))

(def drop-instrument-on-track (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (if (= (get payload :kind) "builtin-instrument")
      (%swap-track-builtin-instrument
        (get target :track)
        (get payload :name))
      (%swap-track-instrument
        (get target :track)
        (get payload :name)))))

(def drop-sample-on-track (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((path (get payload :path))
          (track (get target :track)))
      (if path
        (if (seq-track-custom-instrument? track)
          (host-command "convert-track-to-sampler"
            (dict :track track :path path :preserve-browser-context true))
          (host-command "load-sample-into-track"
            (dict :track track :path path :preserve-browser-context true)))
        (status "Drop a sample file, not a folder")))))

(def drop-sound-on-track (event)
  (if (= (get event :drag-type) "sound")
    (host-command "load-sound-onto-track"
      (dict :track (get (get event :target) :track)
            :path (get (get event :payload) :path)))
    (if (= (get event :drag-type) "instrument")
      (drop-instrument-on-track event)
      (drop-sample-on-track event))))

(def %activate-instrument (name)
  (if (seq-track-replaceable-instrument? SEQ.current-track)
    (%swap-track-instrument SEQ.current-track name)
    (do
      (%add-instrument-track name)
      (status (str "Adding instrument track: " name)))))

(def add-sampler-track ()
  (host-command "add-track-sampler" (dict))
  (set! sbrowser-tab "samples")
  (status "Add sampler track"))

(def %add-modulator-track ()
  (host-command "add-track-modulator" (dict))
  (set! sbrowser-tab "instruments")
  (status "Add modulator track"))

(def add-rack-track ()
  (let ((path (sample-selected-path)))
    (if (= path "")
      (host-command "add-track-rack" (dict))
      (do
        (set! sbrowser-auditioned-sample path)
        (host-command "add-track-rack" (dict :path path)))))
  (set! sbrowser-tab "samples")
  (status "Add drum rack"))

(def %current-rack-routing ()
  (if (= SEQ.sidebar-kind "rack")
    (if (> (len SEQ.instrument-panel) 0)
      (get (nth SEQ.instrument-panel 0) :routing)
      "")
    ""))

(def add-layer-rack-track ()
  (let ((path (sample-selected-path)))
    (if (and (= (%current-rack-routing) "broadcast")
             (not (= path "")))
      (do
        (set! sbrowser-auditioned-sample path)
        (host-command "add-rack-sample-slot"
          (dict :track SEQ.current-track :path path :preserve-browser-context true))
        (status "Add layer"))
      (do
        (if (= path "")
          (host-command "add-track-layer-rack" (dict))
          (do
            (set! sbrowser-auditioned-sample path)
            (host-command "add-track-layer-rack" (dict :path path))))
        (set! sbrowser-tab "samples")
        (status "Add instrument rack")))))

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
          :color (rgba 0.45 0.47 0.50 1.0))))))

;; ── Actions ──

(def audition (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sbrowser-auditioned-sample path)
        (host-command "audition-sample" (dict :path path))
        (status (str "Audition: " (get item :label))))
      (status (str (get item :label))))))

(def select-sample (item)
  (let ((path (get item :path)))
    (if path
      (set! selected-sample path)
      (status (str (get item :label))))))

(def activate-sample (item)
  (let ((path (get item :path)))
    (if path
      (if (or (%create-sampler-mode?) (= SEQ.num-tracks 0))
        (%add-track item)
        (if (= SEQ.sidebar-kind "sampler")
          (audition item)
          (status "Drop samples onto a sampler track or the new-track drop zone")))
      (status "Choose a sample file, not a folder"))))

(def sample-selected-path ()
  (if (= selected-sample "")
    SEQ.sidebar-selected-sample
    selected-sample))

(def %add-track (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sbrowser-auditioned-sample path)
        (host-command "add-track-sample" (dict :path path))
        (%leave-create-mode)
        (status (str "Add track: " (get item :label))))
      (status "Select a sample file, not a folder"))))

(def %add-rack-layer (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sbrowser-auditioned-sample path)
        (host-command "add-rack-sample-slot"
          (dict :track SEQ.current-track :path path :preserve-browser-context true))
        (status (str "Add layer: " (get item :label))))
      (status "Select a sample file, not a folder"))))

(def add-selected-rack-layer ()
  (add-layer-rack-track))

(def %modified-activate-sample (item)
  (if (= SEQ.sidebar-kind "rack")
    (%add-rack-layer item)
    (%add-track item)))

(def %select-item (item)
  (if (or (%create-sampler-mode?) (= SEQ.num-tracks 0) (= SEQ.sidebar-kind "instrument"))
    (%add-track item)
    (audition item)))

(def select-tab (name)
  (let ((changed (not (= sbrowser-tab name))))
    (set! sbrowser-tab name)
    (if changed
      (do
        (set! search-filter "")
        (set! preset-filter "")))
    (if (not (= name "samples"))
      (set! selected-tags (list)))))

(def %next-tab-name ()
  (if (= sbrowser-tab "samples") "sounds"
    (if (= sbrowser-tab "sounds") "instruments"
    (if (= sbrowser-tab "instruments") "audio-fx"
      (if (= sbrowser-tab "audio-fx") "midi-fx"
        (if (= sbrowser-tab "midi-fx") "presets"
          (if (= sbrowser-tab "presets") "scripts"
            (if (= sbrowser-tab "scripts") "projects"
              "samples"))))))))

(def next-tab ()
  (select-tab (%next-tab-name)))

;; Hazard (l): this hands a widget key *out* to Rust —
;; `sample_browser_active_tree_key` (src/ui/input.rs) feeds the result straight
;; into `focus_widget_by_stable_key`, an exact match.  Auto-qualification happens
;; on the widget, not on a string this file builds, so the module name has to be
;; written into the value.
(def %tree-key (base)
  (str "eseq.browser/" base))

(def active-tree-key ()
  (if (= sbrowser-tab "samples") (%tree-key "samples-tab-tree")
    (if (= sbrowser-tab "sounds") (%tree-key "sounds-tab-tree")
    (if (= sbrowser-tab "instruments") (%tree-key "instruments-tab-tree")
      (if (= sbrowser-tab "audio-fx") (%tree-key "audio-fx-tab-tree")
        (if (= sbrowser-tab "midi-fx") (%tree-key "midi-fx-tab-tree")
          (if (= sbrowser-tab "presets") (%tree-key "presets-tab-tree")
            (if (= sbrowser-tab "scripts") (%tree-key "scripts-tab-tree")
              (%tree-key "projects-tab-tree")))))))))

(def list-contains? (items value)
  (> (len (filter (lambda (item) (= item value)) items)) 0))

(def %list-remove (items value)
  (filter (lambda (item) (not (= item value))) items))

(def %toggle-tag (tag)
  (if (list-contains? selected-tags tag)
    (set! selected-tags (%list-remove selected-tags tag))
    (set! selected-tags (append selected-tags (list tag)))))

(def %clear-tags ()
  (set! selected-tags (list)))

(def %set-search-filter (value)
  (if (and (= sbrowser-tab "samples") (not (= value search-filter)))
    (%clear-tags))
  (set! search-filter value))

(def %tag-chip (tag)
  (let ((name (get tag :name))
      (selected (get tag :selected)))
    (button name
      :variant :ghost
      :background-color (if selected 
        (rgba 1 0.6 0.3 1)
        '(rgba 1 1 1 0.1))
      :color (if selected (rgba 0.1 0.1 0.2 1) :white)
      :border-color 
        :transparent
        
      :height 0.9
      :padding 0.532
      :font-size 11.0
      :on-click |x y r| (%toggle-tag name))))

(def %search-placeholder ()
  (if (= sbrowser-tab "samples") "Search samples..."
    (if (= sbrowser-tab "sounds") "Search sounds..."
    (if (= sbrowser-tab "instruments") "Search instruments..."
      (if (= sbrowser-tab "audio-fx") "Search audio effects..."
        (if (= sbrowser-tab "midi-fx") "Search MIDI effects..."
          (if (= sbrowser-tab "presets") "Search presets..."
            (if (= sbrowser-tab "scripts") "Search scripts..."
              "Search projects..."))))))))

(def %empty-message (message)
  (box :width :fill :height :fill :padding 1
    (label message
      :font-size 10
      :color :gray
      :bg :transparent)))

(def select-audio-effect (item)
  (let ((kind (get item :kind)) (label (get item :label)))
    (do
      ;; Only custom (dsp.lisp-backed) effects can be forked; builtins are Rust.
      (set! selected-audio-effect-name
        (if (= kind "custom-audio-effect") (get item :name) ""))
      (if (= kind "header")
        false
        (if (or (= kind "builtin-audio-effect") (= kind "custom-audio-effect"))
          (status (str label))
          (status "Choose an effect"))))))

(def fork-selected-audio-effect ()
  (if (= selected-audio-effect-name "")
    (status "Select a custom effect to fork")
    (do
      (set! sbrowser-editor-name "")
      (host-command "enter-fork-effect-editor"
        (dict :source selected-audio-effect-name)))))

(def enter-new-effect-editor ()
  (set! sbrowser-editor-name "")
  (host-command "enter-new-effect-editor" (dict)))

(def activate-audio-effect (item)
  (let ((kind (get item :kind)) (name (get item :name)))
    (if (= kind "header")
      false
      (if (= kind "builtin-audio-effect")
        (do
          (if (seq-has-selected-bus?)
            (host-command "add-builtin-bus-effect" (dict :bus selected-bus :name name))
            (host-command "add-builtin-effect" (dict :name name)))
          (status (str "Add built-in effect: " name)))
        (if (= kind "custom-audio-effect")
          (do
            (if (seq-has-selected-bus?)
              (host-command "add-bus-effect" (dict :bus selected-bus :name name))
              (host-command "add-effect" (dict :name name)))
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
        (status (str "Add MIDI FX: " name)))
      (status "Choose a MIDI effect"))))

(def %load-preset (name)
  (host-command "load-instrument-preset" (dict :name name))
  (status (str "Load preset: " name)))

(def enter-new-script ()
  (host-command "new-script" (dict))
  (set! sbrowser-script-name "")
  (set! sbrowser-script-save-mode "new-script")
  (set! search-filter "")
  (set! sbrowser-tab "scripts")
  (status "New script"))

(def %script-save-mode? ()
  (= sbrowser-script-save-mode "new-script"))

(def %save-new-script ()
  (if (= (len sbrowser-script-name) 0)
    (status "Enter a script name")
    (host-command "save-new-script" (dict :name sbrowser-script-name))))

(def %cancel-new-script ()
  (do
    (host-command "cancel-new-script" (dict))
    (set! sbrowser-script-name "")
    (set! sbrowser-script-save-mode "")
    (set! sbrowser-tab "scripts")))

(def %select-script (item)
  (let ((kind (get item :kind))
        (label (get item :label)))
    (if (= kind "script")
      (status (str label))
      (if (= kind "folder")
        (status (str "Folder: " label))
        (status "Choose a script")))))

(def %activate-script (item)
  (let ((kind (get item :kind))
        (path (get item :path)))
    (if (and (= kind "script") path)
      (do
        ;; Hazard (m): `seq-script-picker-source-buffer` is a mutable vanilla
        ;; `def` owned by ui/seq-script-picker.lisp.  A bare `set!` from inside
        ;; this module writes eseq.browser's own slot and never reaches the
        ;; owner, so the return-to-source hop after loading a script would
        ;; silently stop working.  Go through the owner's accessor.
        (seq-script-remember-source-buffer)
        (seq-script-load-file path)
        (status (str "Load script: " (get item :label))))
      (status "Choose a script file"))))

(def %save-project ()
  (if (= (len %project-name) 0)
    (status "Enter a project name")
    (do
      (host-command "save-project" (dict :name %project-name))
      (%reset-to-audition)
      (status (str "Save project: " %project-name)))))

(def %load-project (name)
  (host-command "load-project" (dict :name name))
  (%reset-to-audition)
  (status (str "Open project: " name)))

(def %select-project-for-save (item)
  (set! %project-name (get item :label)))

;; ── Search bar widget ──

(def search-header ()
  (box :key "header" :width :fill :height 2.0 :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (text-input
        :key "search-input"
        :width :fill
        :value search-filter
        :placeholder (%search-placeholder)
        :on-change (lambda (v) (%set-search-filter v))
        :height 1.5
        :font-size 12
        (mag-glass)))))

(def %instrument-header ()
  (box :key "instrument-header" :width :fill :height 1.1 :padding 0.15
    (label (if (= SEQ.sidebar-instrument-display-name "") "Instrument" SEQ.sidebar-instrument-display-name)
      :font-size 12
      :color :white
      :bg :transparent)))

(def %create-header ()
  (box :key "create-header" :width :fill :height 1.1 :padding 0.15
    (label "Create track"
      :font-size 12
      :color :white
      :bg :transparent)))

(def %project-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.2
      (h-stack :width :fill :gap 0.5 :align :center
        (text-input
          :flex 1
          :value search-filter
          :placeholder "Search projects..."
          :on-change (lambda (v) (%set-search-filter v))
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
          (if (= SEQ.current-project-name "") "none" SEQ.current-project-name))
        :font-size 9
        :color :gray
        :bg :transparent))))

(def %project-save-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.5
      (text-input
        :width :fill
        :value %project-name
        :placeholder "Project name..."
        :on-change (lambda (v) (set! %project-name v))
        :height 1.5
        :font-size 12
        (mag-glass))
      (button "Save"
        :variant :primary
        :width 8.0
        :height 1.2
        :font-size 11
        :on-click |x y r| (%save-project)
        :color :white))))

(def %script-save-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.4
      (h-stack :width :fill :gap 0.5 :align :center
        (label "Save Script"
          :font-size 12
          :color :white
          :bg :transparent)
        (button "Cancel"
          :variant :ghost
          :width 5.8
          :height 1.2
          :font-size 9
          :on-click |x y r| (%cancel-new-script)
          :color :gray))
      (text-input
        :width :fill
        :value sbrowser-script-name
        :placeholder "script name..."
        :on-change (lambda (v) (set! sbrowser-script-name v))
        :height 1.5
        :font-size 12
        (mag-glass))
      (button "Save Script"
        :variant :primary
        :width 10
        :height 1.2
        :font-size 11
        :on-click |x y r| (%save-new-script)
        :color :white))))

(def %script-save-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

(def create-items ()
  (seq-saved-instrument-tree search-filter SEQ.project-instrument-engines))

(def %enter-new-instrument-editor ()
  (set! sbrowser-editor-name "")
  (host-command "enter-new-instrument-editor" (dict)))

(def %add-builtin-instrument-track (name)
  (if (= name "sampler")
    (add-sampler-track)
    (if (= name "modulator")
      (%add-modulator-track)
      (if (= name "rack")
        (add-rack-track)
        (if (= name "layer-rack")
          (add-layer-rack-track)
          (status "Choose an instrument"))))))

(def %activate-builtin-instrument (name)
  (if (and (= name "sampler")
           (seq-track-replaceable-instrument? SEQ.current-track))
    (%swap-track-builtin-instrument SEQ.current-track name)
    (%add-builtin-instrument-track name)))

(def select-create-item (item)
  (let ((kind (get item :kind)))
    (if (= kind "header")
      false
      (if (= kind "builtin-instrument")
        (%activate-builtin-instrument (get item :name))
        (if (= kind "sampler")
          (%enter-create-sampler-mode)
          (if (= kind "new-instrument")
            (%enter-new-instrument-editor)
            (if (= kind "instrument")
              (%activate-instrument (get item :name))
              (status "Choose an instrument"))))))))

(def focus-create-item (item)
  (let ((kind (get item :kind)))
    (do
      (set! selected-instrument-name
        (if (= kind "instrument") (get item :name) ""))
      (if (= kind "header")
        false
        (if (or (= kind "instrument") (= kind "builtin-instrument"))
          (status (str (get item :label)))
          (if (= kind "folder")
            (status (str "Folder: " (get item :label)))
            (status "Choose an instrument")))))))

(def fork-selected-instrument ()
  (if (= selected-instrument-name "")
    (status "Select a saved instrument to fork")
    (do
      (set! sbrowser-editor-name "")
      (host-command "enter-fork-instrument-editor"
        (dict :source selected-instrument-name)))))

(def drop-instrument-on-folder (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((name (get payload :name))
          (folder (get target :folder)))
      (if (and (= (get payload :kind) "instrument") name folder)
        (do
          (host-command "move-saved-instrument" (dict :name name :folder folder))
          (status (str "Move instrument to " (get target :label))))
        (status "Drop instruments onto a folder")))))

(def %loading-instrument? ()
  (not (= sbrowser-loading-instrument-name "")))

(def %instrument-loading-row ()
  (if (%loading-instrument?)
    (box :key "instrument-loading-row" :width :fill :padding 0.25
      (editor-status-row
        (str "Loading " sbrowser-loading-instrument-name "...")
        :gray))
    (box :height 0)))

(def %create-picker ()
  (v-stack :key "create-picker-panel" :width :fill :gap 0.5 :flex 1
    (%instrument-loading-row)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (scroll :key "create-picker-scroll" :width :fill :flex 1
        (tree
          :key "create-picker-tree"
          :width :fill
          :background-color :buffer-bg
          :items (create-items)
          :expand-all (not (= search-filter ""))
          :drag-type "instrument"
          :drop-types (list "instrument")
          :on-drop (lambda (event) (drop-instrument-on-folder event))
          :on-select (lambda (item) (focus-create-item item))
          :on-activate (lambda (item) (select-create-item item)))))))

(def %tab-items ()
  (list
    (dict :name "samples" :label "Samples" :icon :waveform)
    (dict :name "sounds" :label "Sounds" :icon :piano)
    (dict :name "instruments" :label "Instruments" :icon :piano)
    (dict :name "audio-fx" :label "Audio FX" :icon :sliders)
    (dict :name "midi-fx" :label "MIDI FX" :icon :note-arrow)
    (dict :name "presets" :label "Presets" :icon :dial)
    (dict :name "scripts" :label "Scripts" :icon :folder)
    (dict :name "projects" :label "Projects" :icon :folder)))

(def %visible-sounds ()
  (if (= search-filter "") SEQ.sound-presets
    (filter (lambda (item)
      (string-contains? (lowercase (get item :label)) (lowercase search-filter)))
      SEQ.sound-presets)))

(def %load-sound (item)
  (if (= SEQ.num-tracks 0)
    (status "Create a track before loading a Sound")
    (host-command "load-sound-onto-track"
      (dict :track SEQ.current-track :path (get item :path)))))

(def %sounds-panel ()
  (let ((items (%visible-sounds)))
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (if (= (len items) 0)
        (%empty-message "No Sounds found. Drag an instrument preset onto the Sounds tab to add one.")
        (scroll :key "sounds-tab-scroll" :width :fill :flex 1
          (tree :key "sounds-tab-tree"
                :width :fill
                :background-color :buffer-bg
                :items items
                :focusable true
                :drag-type "sound"
                :on-activate (lambda (item) (%load-sound item))))))))

(def drop-preset-on-sounds (event)
  (if (= (get event :drag-type) "instrument-preset")
    (let ((name (get (get event :payload) :label)))
      (if name
        (host-command "promote-preset-to-sound"
          (dict :track SEQ.sidebar-track-index :name name))
        (status "Drop a preset item onto Sounds")))
    false))

(def %tab-button (name label icon)
  (button label
    :key (str "tab-" name)
    :variant :ghost
    :icon icon
    :active (= sbrowser-tab name)
    :width :fill
    :height 1.45
    :font-size 11.0
    :h-align :left
    :background-color '(rgba 1 1 1 0.0)
    :active-background-color '(rgba 1 1 1 0.07)
    :border-color '(rgba 1 1 1 0.0)
    :highlight-color '(rgba 1 1 1 0.0)
    :shadow-color '(rgba 0 0 0 0.0)
    :corner-radius 8
    :drop-types (if (= name "sounds") (list "instrument-preset") (list))
    :drop-meta (dict :kind "browser-tab" :name name)
    :drop-hover-background-color '(rgba 0.15 0.45 0.70 0.28)
    :on-drop (lambda (event) (drop-preset-on-sounds event))
    :on-click |x y r| (select-tab name)
    :color :widget-label-fg
    :active-color :blue))

(def tab-rail ()
  (let ((tabs (%tab-items)))
    (box :key "tabs" :width 11.4 :height :fill :padding 0.35
      (v-stack :width :fill :gap 0.18
        (each (range 0 (len tabs)) |i|
          (let ((tab (nth tabs i)))
            (%tab-button
              (get tab :name)
              (get tab :label)
              (get tab :icon))))))))

(def %samples-panel ()
  (let ((browser (seq-sample-browser search-filter selected-tags)))
    (let ((tags (get browser :tags))
        (items (get browser :items)))
      (v-stack :key "samples-browser-panel" :width :fill :gap 0.35 :flex 1
        (box :key "sample-tag-filter" :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0.35
          (v-stack :width :fill :gap 0.35
            (if (> (len selected-tags) 0)
              (button "Clear"
                :variant :ghost
                :width :fill
                :height 1.15
                :font-size 9
                :on-click |x y r| (%clear-tags)
                :color :white))
            (wrap :width :fill :gap 0.25 :row-gap 0.18 :align :center
              (each (range 0 (len tags)) |i|
                (%tag-chip (nth tags i))))))
        (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
          (if (= (len items) 0)
            (%empty-message
              (if (and (= search-filter "") (= (len selected-tags) 0))
                "Choose a tag or search samples."
                "No samples found."))
            (scroll :key "samples-tab-scroll" :width :fill :flex 1
              (tree
                :key "samples-tab-tree"
                :width :fill
                :focusable true
                :background-color :buffer-bg
                :items items
                :selected-path (sample-selected-path)
                :expand-all true
                :drag-type "sample"
                :on-select (lambda (item) (select-sample item))
                :on-cursor-change (lambda (item) (select-sample item))
                :on-activate (lambda (item) (activate-sample item))
                :on-modified-activate (lambda (item) (%modified-activate-sample item))))))))))

(def %instruments-toolbar ()
  (box :width :fill :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (button
        (if (= selected-instrument-name "")
          "Fork…"
          (str "Fork " selected-instrument-name))
        :variant :secondary
        :flex 1
        :height 1.3
        :font-size 10.5
        :on-click |x y r| (fork-selected-instrument)
        :color :white))))

(def %instruments-panel ()
  (v-stack :key "instrument-tab-panel" :width :fill :gap 0.5 :flex 1
    (%instruments-toolbar)
    (%instrument-loading-row)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (scroll :key "instruments-tab-scroll" :width :fill :flex 1
        (tree
          :key "instruments-tab-tree"
          :width :fill
          :background-color :buffer-bg
          :items (create-items)
          :expand-all (not (= search-filter ""))
          :focusable true
          :drag-type "instrument"
          :drop-types (list "instrument")
          :on-drop (lambda (event) (drop-instrument-on-folder event))
          :on-select (lambda (item) (focus-create-item item))
          :on-activate (lambda (item) (select-create-item item))
          :on-modified-activate (lambda (item) (select-create-item item)))))))

(def %audio-fx-toolbar ()
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
        (if (= selected-audio-effect-name "")
          "Fork…"
          (str "Fork " selected-audio-effect-name))
        :variant :secondary
        :flex 1
        :height 1.3
        :font-size 10.5
        :on-click |x y r| (fork-selected-audio-effect)
        :color :white))))

(def %audio-fx-panel ()
  (let ((items (seq-audio-effect-tree search-filter)))
    (v-stack :key "audio-fx-tab-panel" :width :fill :gap 0.5 :flex 1
      (%audio-fx-toolbar)
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (%empty-message "No audio effects found.")
          (scroll :key "audio-fx-tab-scroll" :width :fill :flex 1
            (tree
              :key "audio-fx-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :expand-all (not (= search-filter ""))
              :focusable true
              :drag-type "audio-effect"
              :on-select (lambda (item) (select-audio-effect item))
              :on-cursor-change (lambda (item) (select-audio-effect item))
              :on-activate (lambda (item) (activate-audio-effect item))
              :on-modified-activate (lambda (item) (activate-audio-effect item)))))))))

(def %midi-fx-panel ()
  (let ((items (seq-midi-effect-tree search-filter)))
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (if (= (len items) 0)
        (%empty-message "No MIDI effects found.")
        (scroll :key "midi-fx-tab-scroll" :width :fill :flex 1
          (tree
            :key "midi-fx-tab-tree"
            :width :fill
            :background-color :buffer-bg
            :items items
            :expand-all (not (= search-filter ""))
            :focusable true
            :drag-type "midi-effect"
            :on-select (lambda (item) (select-midi-effect item))
            :on-cursor-change (lambda (item) (select-midi-effect item))
            :on-activate (lambda (item) (activate-midi-effect item))
            :on-modified-activate (lambda (item) (activate-midi-effect item))))))))

(def %presets-tab-panel ()
  (v-stack :key "presets-tab-panel" :width :fill :gap 0.22 :padding 0.25 :flex 1
    (%instrument-header)
    (if (= SEQ.sidebar-kind "instrument")
      (let ((items (seq-preset-tree SEQ.sidebar-presets search-filter)))
        (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
          (if (= (len items) 0)
            (%empty-message "No presets found.")
            (scroll :key "presets-tab-scroll" :width :fill :flex 1
              (tree
                :key "presets-tab-tree"
                :width :fill
                :background-color :buffer-bg
                :items items
                :selected-label SEQ.sidebar-loaded-preset
                :expand-all false
                :focusable true
                :drag-type "instrument-preset"
                :on-select (lambda (item) (%load-preset (get item :label)))
                :on-activate (lambda (item) (%load-preset (get item :label))))))))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (%empty-message "Presets are available for instrument tracks.")))))

(def %scripts-toolbar ()
  (box :width :fill :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (button "New Script"
        :key "script-new-button"
        :variant :secondary
        :flex 1
        :height 1.3
        :font-size 10.5
        :on-click |x y r| (enter-new-script)
        :color :white))))

(def %scripts-tab-panel ()
  (let ((items (seq-script-tree search-filter)))
    (v-stack :key "scripts-tab-panel" :width :fill :gap 0.5 :flex 1
      (%scripts-toolbar)
      (box :width :fill :padding 0.25
        (label "Scripts"
          :font-size 10
          :color :gray
          :bg :transparent))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (%empty-message "No scripts found.")
          (scroll :key "scripts-tab-scroll" :width :fill :flex 1
            (tree
              :key "scripts-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :expand-all (not (= search-filter ""))
              :focusable true
              :on-select (lambda (item) (%select-script item))
              :on-cursor-change (lambda (item) (%select-script item))
              :on-activate (lambda (item) (%activate-script item))
              :on-modified-activate (lambda (item) (%activate-script item)))))))))

(def %projects-tab-panel ()
  (let ((items (seq-project-tree search-filter)))
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
          (%empty-message "No projects found.")
          (scroll :key "projects-tab-scroll" :width :fill :flex 1
            (tree
              :key "projects-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :selected-label SEQ.current-p0roject-name
              :expand-all false
              :focusable true
              :on-select (lambda (item) (%load-project (get item :label)))
              :on-activate (lambda (item) (%load-project (get item :label))))))))))

(def active-tab-panel ()
  (if (= sbrowser-tab "samples") (%samples-panel)
    (if (= sbrowser-tab "sounds") (%sounds-panel)
    (if (= sbrowser-tab "instruments") (%instruments-panel)
      (if (= sbrowser-tab "audio-fx") (%audio-fx-panel)
        (if (= sbrowser-tab "midi-fx") (%midi-fx-panel)
          (if (= sbrowser-tab "presets") (%presets-tab-panel)
            (if (= sbrowser-tab "scripts") (%scripts-tab-panel)
              (%projects-tab-panel)))))))))

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

(def %main-panel ()
  (v-stack :key "tabbed-browser" :width :fill :height :fill :gap 0.45 :flex 1
    (tabbed-content)))

(def %preset-search-bar ()
  (box :key "preset-search-bar" :width :fill :height 1.8 :padding 0.15
    (h-stack :width :fill :gap 0.5 :align :center
      (text-input
        :flex 1
        :value preset-filter
        :placeholder "Search presets..."
        :on-change (lambda (v) (set! preset-filter v))
        :height 1.5
        :font-size 12
        (mag-glass)))))

(def %presets-panel ()
  (v-stack :key "preset-list-panel" :width :fill :gap 0.22 :flex 1
    (%instrument-header)
    (%preset-search-bar)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (scroll :key "preset-list-scroll" :width :fill :flex 1
        (tree
          :key "preset-list-tree"
          :width :fill
          :background-color :buffer-bg
          :items (seq-preset-tree SEQ.sidebar-presets preset-filter)
          :selected-label SEQ.sidebar-loaded-preset
          :expand-all false
          :on-select (lambda (item) (%load-preset (get item :label)))
          :on-activate (lambda (item) (%load-preset (get item :label))))))))

(def %projects-panel ()
  (let ((items (seq-project-tree search-filter)))
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
            :selected-label SEQ.current-project-name
            :expand-all false
            :on-select (lambda (item) (%load-project (get item :label)))
            :on-activate (lambda (item) (%load-project (get item :label)))))))))

(def %project-save-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

;; ── Preset save sidebar ──

(def %preset-save-mode? ()
  (= %preset-save-mode "save-preset"))

(def enter-preset-save ()
  (set! %preset-name "")
  (set! %preset-save-mode "save-preset"))

(def %exit-preset-save ()
  (set! %preset-save-mode ""))

(def %preset-save-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.4
      (h-stack :width :fill :gap 0.5 :align :center
        (label "Save Preset"
          :font-size 12
          :color :white
          :bg :transparent)
        (box :bg :dark-gray :width 6 :height 1.5 :align :center
          :on-click |x y r| (%exit-preset-save)
          (label "cancel"
            :font-size 9
            :color :gray
            :bg :transparent)))
      (text-input
        :width :fill
        :value %preset-name
        :placeholder "preset name..."
        :on-change (lambda (v) (set! %preset-name v))
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
            (host-command "save-preset" (dict :name %preset-name :overwrite false))
            (%exit-preset-save))
        :color :white)
      ;; Overwrite button (only if a preset is currently loaded)
      (if (not (= SEQ.sidebar-loaded-preset ""))
        (button (str "Overwrite: " SEQ.sidebar-loaded-preset)
          :variant :secondary
          :width 16
          :height 1.2
          :font-size 10
          :on-click |x y r|
            (do
              (host-command "overwrite-preset" (dict))
              (%exit-preset-save))
          :color :white)
        (box)))))

(def %preset-save-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

;; ── Editor sidebar panels ──

(def %editor-macro-action? ()
  (or (= SEQ.editor-active-macro-action "save-to-library")
      (= SEQ.editor-active-macro-action "fork")))

(def %editor-macro-action-label ()
  (if (= SEQ.editor-active-macro-action "fork")
    "Fork Macro"
    "Save Macro to Library"))

;; Fork is offered exactly when the primary button means "overwrite the shared
;; definition" — i.e. edit-instrument / edit-effect. It has nothing to fork from
;; in the new-* draft modes, and hides behind the macro action like everything
;; else in that stack.
(def %editor-fork-available? ()
  (and (not (%editor-macro-action?))
       (or (= SEQ.editor-mode "edit-instrument")
           (= SEQ.editor-mode "edit-effect"))))

(def %editor-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.4
      (h-stack :width :fill :gap 0.5 :align :center
        (label
          (if (%editor-macro-action?) "Defmacro"
            (if (= SEQ.editor-mode "new-instrument") "New Instrument"
              (if (= SEQ.editor-mode "edit-instrument")
                (if (= SEQ.editor-surface "code") "Edit Instrument (code)" "Edit Instrument")
                (if (= SEQ.editor-mode "new-effect") "New Effect"
                  (if (= SEQ.editor-mode "edit-effect")
                    (if (= SEQ.editor-surface "code") "Edit Effect (code)" "Edit Effect")
                    "Editor")))))
          :font-size 12
          :color :white
          :bg :transparent))
      
      (if (%editor-macro-action?)
        (v-stack :width :fill :gap 0.35
          (label "Current macro"
            :font-size 9
            :color :gray
            :bg :transparent)
          (label SEQ.editor-active-macro-name
            :font-size 11
            :color :white
            :bg :transparent)
          (label "Action"
            :font-size 9
            :color :gray
            :bg :transparent)
          (label (%editor-macro-action-label)
            :font-size 11
            :color :white
            :bg :transparent))
        (if (= SEQ.editor-mode "new-instrument")
          (v-stack :width :fill :gap 0.35
            (label "Mode"
              :font-size 9
              :color :gray
              :bg :transparent)
            (h-stack :width :fill :gap 0.35
              (button "Instrument"
                :variant (if (= SEQ.editor-instrument-run-mode "instrument") :primary :secondary)
                :width 8.5
                :height 1.2
                :font-size 9
                :on-click |x y r|
                (host-command "set-draft-instrument-run-mode" (dict :run-mode "instrument"))
                :color :white)
              (button "Free Patch"
                :variant (if (= SEQ.editor-instrument-run-mode "free_patch") :primary :secondary)
                :width 8.5
                :height 1.2
                :font-size 9
                :on-click |x y r|
                (host-command "set-draft-instrument-run-mode" (dict :run-mode "free_patch"))
                :color :white))
            (label "Save as"
              :font-size 9
              :color :gray
              :bg :transparent)
            (text-input
              :width :fill
              :value sbrowser-editor-name
              :placeholder "instrument-name"
              :on-change (lambda (v) (set! sbrowser-editor-name v))
              :height 1.5
              :font-size 12))
          (if (= SEQ.editor-mode "new-effect")
            (v-stack :width :fill :gap 0.35
              (label "Draft patch"
                :font-size 9
                :color :gray
                :bg :transparent)
              (label (str "track " (+ SEQ.current-track 1))
                :font-size 11
                :color :white
                :bg :transparent)
              (label "Save as"
                :font-size 9
                :color :gray
                :bg :transparent)
              (text-input
                :width :fill
                :value sbrowser-editor-name
                :placeholder "effect-name"
                :on-change (lambda (v) (set! sbrowser-editor-name v))
                :height 1.5
                :font-size 12))
            ;; For edit modes, show the file name
            (label SEQ.editor-buffer-name
              :font-size 10
              :color :gray
              :bg :transparent))))
      ;; Status display
      (if SEQ.editor-canceling
        (editor-status-row "Canceling..." :gray)
        (if (= SEQ.editor-error "Preview compiling...")
          (editor-status-row SEQ.editor-error :gray)
          (if (not (= SEQ.editor-error ""))
            (label SEQ.editor-error
              :font-size 9
              :color :red
              :bg :transparent)
            (box))))
      ;; Eval button (code editor only): compile + hot-swap the buffer
      (if (= SEQ.editor-surface "code")
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
      (if (and (= SEQ.editor-surface "code")
          (or (= SEQ.editor-mode "edit-instrument") (= SEQ.editor-mode "edit-effect")))
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
      (if (and (= SEQ.editor-surface "patch")
          (or (= SEQ.editor-mode "edit-instrument") (= SEQ.editor-mode "edit-effect")))
        (button "Eject to code"
          :variant :secondary
          :width 13
          :height 1.2
          :font-size 10
          :on-click |x y r|
          (host-command "eject-editor-to-code" (dict))
          :color :white)
        (box))
      ;; Save button
      (if (%editor-busy?)
        (box :height 1.2)
        (h-stack :align :baseline
          (button
            (if (%editor-macro-action?)
              (%editor-macro-action-label)
              (if (= SEQ.editor-mode "new-instrument")
                "Finalize"
                (if (= SEQ.editor-mode "new-effect")
                  "Save & Add"
                  "Save")))
            :variant :primary
            :width (if (%editor-macro-action?) 14.5 10)
            :height 1.2
            :font-size 11
            :on-click |x y r|
            (if (%editor-macro-action?)
              (host-command "save-active-editor-macro" (dict))
              (if (= SEQ.editor-mode "new-instrument")
                (host-command "save-new-instrument" (dict :name sbrowser-editor-name))
                (if (= SEQ.editor-mode "edit-instrument")
                  (host-command "update-instrument" (dict :name SEQ.sidebar-instrument-name))
                  (if (= SEQ.editor-mode "new-effect")
                    (host-command "save-new-effect" (dict :name sbrowser-editor-name))
                    (host-command "update-effect" (dict))))))
            :color :white)
          ;; Fork sits next to the clobbering path on purpose: in edit modes the
          ;; primary button overwrites an instrument every project shares, and
          ;; the safe alternative should not require leaving the buffer.
          (if (%editor-fork-available?)
            (button "Fork"
              :variant :secondary
              :width 6
              :height 1.2
              :font-size 11
              :on-click |x y r| (host-command "fork-editor-session" (dict))
              :color :white)
            (box))
          (box :bg :dark-gray :width 6 :height 1.5 :align :center
            :on-click |x y r|
            (if SEQ.editor-canceling nil (host-command "cancel-editor" (dict)))
            (button "cancel"
              :font-size 9
              :height 1.2
              :color (if SEQ.editor-canceling :white :white)
              :background-color :gray)))            
        )
      )))

(def %editor-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

;; ── Build widgets ──

(def build-widgets ()
  (do
    (%sync-track-search)
    (if (%editor-mode?)
      (list
        (%editor-header)
        (%editor-panel))
      (if (%preset-save-mode?)
        (list
          (%preset-save-header)
          (%preset-save-panel))
        (if (project-save-mode?)
          (list
            (%project-save-header)
            (%project-save-panel))
          (if (%script-save-mode?)
            (list
              (%script-save-header)
              (%script-save-panel))
            (list
              (tabbed-content))))))))

;; ── Reactive rendering (like ui/main.lisp) ──

(def %root-widget ()
  (v-stack :width :fill :gap 0.4 :padding 0.15 (build-widgets)))

(def refresh-buffer ()
  (render-widget-to-buffer "*samples*" (%root-widget)))

(effect-buffer "*samples*"
  (%root-widget))

;; ── Entry point: just switch to the buffer ──

(def sample-browser-here ()
  (set! %source-buffer (current-buffer-name))
  (set! search-filter "")
  (set! %mode "audition")
  (set! sbrowser-tab (if (= SEQ.sidebar-kind "instrument") "presets" "samples"))
  (switch-to-buffer "*samples*"))

(bind-key "C-x s" "sample-browser-here")
