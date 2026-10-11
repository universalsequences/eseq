;; Edit this file or use eseq.menus/register-menu from user init/packages.
(module eseq.application-menus)
(import eseq.menus :as menus)
(import eseq.seq-core-state)
(import eseq.kinds :refer (tracks buses selection browser project transport))
(export application-menu-items install-default-menus)

(def commands-enabled? () (not (get (native-menu-context) :blocked)))
(def edit-commands-enabled? ()
  (let ((context (native-menu-context)))
    (or (not (get context :blocked)) (get context :text-input))))
(def has-tracks? () (not (empty? (tracks))))
(def pattern-commands-enabled? ()
  (let ((context (native-menu-context)))
    (and (not (get context :blocked)) (get context :ui-view)
         (not (get context :text-input)) (has-tracks?))))

(def track-pattern-enabled? ()
  (and (pattern-commands-enabled?) (< eseq.seq-core-state/selected-bus 0)))

(def application-menu-entry (id label shortcut action)
  (if (= shortcut "")
    (dict :id id :label label :on-select action)
    (dict :id id :label label :shortcut (dict :key shortcut :modifiers (list :primary)) :on-select action)))

;; The recent projects; reading the project's name re-lists them when
;; another project opens or saves.
(def recent-project-items ()
  project.name
  (map (lambda (name)
    (application-menu-entry (str "file-recent-" name) name ""
      (lambda () (host-command "menu-open-recent" (dict :name name)))))
    (seq-recent-projects)))

;; ── Commands ────────────────────────────────────────────────────────────
;; Each menu action is a palette command too (docs/command-palette-spec.md):
;; the menus below call these, so the menu and the palette run one thing.
;; Menus call them through lambdas, which look the name up on each click, so
;; an `override` or redefinition reaches the menu as it does the palette.
;; `:shortcut` mirrors the menu accelerator so the palette can show it.

(command-group "File" :icon :document)
(command-group "Create" :icon :plus)
(command-group "Pattern" :icon :clips)

;; The palette's home list (`:pinned`) is what you reach for while making
;; music: transport, tempo, views, adding tracks. The rest are found by
;; typing.

(command-group "Transport" :icon :sliders)
(command-group "View" :icon :clips)

(def playing? () transport.playing)
(def recording? () transport.recording)
(def metronome-on? () transport.metronome)
(def roll-mode-on? () transport.roll-mode)

(def toggle-playback () :doc "Start or stop the transport." :command "Play" :group "Transport"
  :current playing? :shortcut "SPC" :pinned true
  (set! transport.playing (not transport.playing)))
(def toggle-recording () :doc "Arm or disarm recording into the playing patterns." :command "Record"
  :group "Transport" :current recording? :pinned true
  (set! transport.recording (not transport.recording)))
(def toggle-metronome () :doc "Click on every beat while the transport runs." :command "Metronome"
  :group "Transport" :current metronome-on? :pinned true
  (set! transport.metronome (not transport.metronome)))
(def toggle-roll-mode () :doc "Hold steps to roll them at the roll rate." :command "Roll Mode"
  :group "Transport" :current roll-mode-on?
  (set! transport.roll-mode (not transport.roll-mode)))

(def tempo-choices ()
  (map (lambda (bpm) (dict :label (str bpm " BPM") :value bpm))
       (list 60 70 80 85 90 95 100 110 120 124 126 128 130 135 140 150 160 170 174 180)))
(def current-tempo () transport.bpm)
(def set-tempo (bpm) :doc "Set the project tempo. Previews as you move through the list."
  :command "Set Tempo" :group "Transport" :prompt "Tempo" :choices tempo-choices :current current-tempo
  :preview true :pinned true
  (set! transport.bpm bpm))

(def quantize-choices () eseq.kinds/launch-quantize-options)
(def current-launch-quantize () transport.launch-quantize)
(def set-launch-quantize (quantize) :doc "Where a launched scene or clip waits to start."
  :command "Launch Quantize" :group "Transport" :prompt "Quantize" :choices quantize-choices
  :current current-launch-quantize :pinned true
  (set! transport.launch-quantize quantize))
(def record-quantize-choices () eseq.kinds/record-quantize-options)
(def current-record-quantize () transport.record-quantize)
(def set-record-quantize (quantize) :doc "Snap recorded notes to this grid."
  :command "Record Quantize" :group "Transport" :prompt "Quantize" :choices record-quantize-choices
  :current current-record-quantize
  (set! transport.record-quantize quantize))

(def show-sequencer () :doc "Show the step sequencer." :command "Go to Sequencer" :group "View" :pinned true
  (eseq.seq-panels/seq-show-sequencer-main))
(def show-arrangement () :doc "Show the arrangement timeline." :command "Go to Arrangement" :group "View"
  :pinned true
  (eseq.seq-panels/seq-open-arrangement))
(def show-piano-roll () :doc "Show the piano roll for the selected track." :command "Go to Piano Roll"
  :group "View" :pinned true
  (eseq.seq-panels/seq-open-piano-roll-preferred))
(def browser-visible? () eseq.seq-core-state/samples-sidebar-visible)
(def mixer-visible? () eseq.seq-core-state/mixer-panel-visible)
(def track-fx-visible? () eseq.seq-core-state/lower-panel-visible)
(def patch-macros-visible? () eseq.seq-core-state/patch-macros-panel-visible)
(def toggle-browser () :doc "Show or hide the browser sidebar." :command "Show Browser" :group "View"
  :current browser-visible? :pinned true
  (eseq.seq-panels/seq-toggle-samples-sidebar))
(def toggle-mixer () :doc "Show or hide the mixer." :command "Show Mixer" :group "View"
  :current mixer-visible? :pinned true
  (eseq.seq-panels/seq-toggle-mixer-panel))
(def toggle-track-fx () :doc "Show or hide the selected track's effects." :command "Show Track FX" :group "View"
  :current track-fx-visible? :pinned true
  (eseq.seq-panels/seq-toggle-fx-panel))
(def toggle-patch-macros () :doc "Show or hide the patch macro knobs." :command "Show Patch Macros"
  :group "View" :current patch-macros-visible?
  (eseq.seq-panels/seq-toggle-patch-macros-panel))
(def collapse-all-tracks () :doc "Fold every track row to its header." :command "Collapse All Tracks"
  :group "View"
  (eseq.sequencer/collapse-all-tracks))

(def request-new-project () :doc "Start a new, empty project, asking to save first." :command "New Project" :pinned true :icon :project :shortcut "s-n"
  :group "File"
  (host-command "project-new-request" (dict)))
(def save-project () :doc "Save the project." :command "Save" :pinned true :icon :save :shortcut "s-s" :group "File"
  (eseq.transport/file-menu-save))
(def save-project-as () :doc "Save the project under a new name." :command "Save As…" :icon :save :shortcut "S-s-s" :group "File"
  (eseq.transport/file-menu-save-as))
(def open-project () :doc "Open a saved project." :command "Open Project…" :pinned true :icon :folder :shortcut "s-o" :group "File"
  (eseq.transport/file-menu-open-project))
(def import-samples () :doc "Copy sample files into the library." :command "Import Samples…" :icon :download :group "File"
  (host-command "menu-import-samples" (dict)))
(def import-package () :doc "Install a package from an .eseqpack file." :command "Import Package…" :icon :download :group "File"
  (host-command "menu-import-package" (dict)))
(def export-package () :doc "Bundle a package into an .eseqpack file." :command "Export Package…" :icon :upload :group "File"
  (host-command "menu-export-package" (dict)))
(def show-settings () :doc "Open the Settings window." :command "Settings…" :icon :sliders :shortcut "s-," :group "File"
  (host-command "settings-open" (dict)))
(def open-init () :doc "Open your init.lisp, loaded at every start." :command "Open init.lisp" :icon :document :group "File"
  (host-command "packages-open-init" (dict)))
(def open-project-scratch () :doc "Open this project's scratch Lisp buffer." :command "Open Project Scratch" :icon :document :group "File"
  (host-command "packages-open-scratch" (dict)))
(def open-help () :doc "Open the manual." :command "Help" :group "File"
  (host-command "open-help" (dict)))
(def about-eseq () :doc "Show the version and credits." :command "About eseq" :group "File"
  (host-command "about-open" (dict)))
(def quit-eseq () :doc "Quit, asking to save unsaved changes." :command "Quit eseq" :group "File"
  (host-command "app-quit-request" (dict)))

(def add-bus () :doc "Add a mixer bus." :command "Add Bus" :icon :sliders :group "Create"
  (host-command "add-bus" (dict)))
(def add-midi-track () :doc "Add an empty MIDI track." :command "Add MIDI Track" :pinned true :icon :midi :group "Create"
  (host-command "add-track-empty" (dict)))
(def add-drum-rack () :doc "Add a drum rack track." :command "Add Drum Rack" :pinned true :icon :sampler :group "Create"
  (host-command "add-track-rack" (dict)))
(def add-layer-rack () :doc "Add a layer rack track." :command "Add Layer Rack" :group "Create"
  (host-command "add-track-layer-rack" (dict)))
(def add-scene () :doc "Add a scene cloned from the current one." :command "Add Scene" :group "Create"
  (host-command "clone-pattern" (dict)))
(def add-scene-bank () :doc "Add a scene bank." :command "Add Scene Bank" :group "Create"
  (eseq.transport/select-scene-bank "New bank"))

(def double-pattern-length () :doc "Double the track pattern's length, repeating its steps." :command "Double Pattern Length" :shortcut "s-+" :group "Pattern"
  (host-command "menu-pattern-double" (dict)))
(def half-pattern-length () :doc "Halve the track pattern's length." :command "Half Pattern Length" :shortcut "s--" :group "Pattern"
  (host-command "menu-pattern-half" (dict)))
(def clone-track-pattern () :doc "Copy the track pattern into a new pattern." :command "Clone Track Pattern" :group "Pattern"
  (host-command "menu-pattern-clone" (dict)))
;; Recover a freely played phrase without enabling recording or transport.
(def capture-midi () :doc "Turn what you just played into a pattern." :command "Capture MIDI…" :pinned true :icon :midi :group "Pattern"
  (host-command "retrospective-open" (dict)))
;; Print the last 30 seconds of master output into a crop-and-keep modal.
(def capture-resample () :doc "Record the last 30 seconds of output to a sample." :command "Resample Last 30 s…" :icon :waveform
  :group "Pattern"
  (host-command "resample-open" (dict)))
(def shift-steps-left () :doc "Rotate the track pattern one step left." :command "Shift Steps Left" :group "Pattern"
  (host-command "menu-pattern-transform" (dict :operation "left")))
(def shift-steps-right () :doc "Rotate the track pattern one step right." :command "Shift Steps Right" :group "Pattern"
  (host-command "menu-pattern-transform" (dict :operation "right")))
(def transpose-choices ()
  (list (dict :label "Up Semitone" :value 1) (dict :label "Down Semitone" :value -1)
        (dict :label "Up Octave" :value 12) (dict :label "Down Octave" :value -12)))
(def transpose-pattern (semitones) :doc "Transpose the track pattern's notes." :command "Transpose Pattern" :icon :note-arrow
  :group "Pattern" :prompt "Transpose" :choices transpose-choices
  (host-command "menu-pattern-transpose" (dict :semitones semitones)))
(def clear-track-pattern () :doc "Clear every step of the track pattern, after asking." :command "Clear Track Pattern…" :group "Pattern"
  (host-command "menu-pattern-clear-request" (dict)))

(def application-menu-items (name)
  (if (= name "File")
    (list
      (application-menu-entry "file-menu-new-project" "New Project" "n" (lambda () (request-new-project)))
      nil
      (application-menu-entry "file-menu-save" "Save" "s" (lambda () (save-project)))
      (merge (application-menu-entry "file-menu-save-as" "Save As…" "s" (lambda () (save-project-as))) :shortcut (dict :key "s" :modifiers (list :primary :shift)))
      (application-menu-entry "file-menu-open-project" "Open Project…" "o" (lambda () (open-project)))
      (dict :id "file-recent" :label "Open Recent" :items-when recent-project-items)
      nil
      (merge (application-menu-entry "file-menu-export" "Export Audio…" "e" (lambda () (eseq.export-song/export-song))) :shortcut (dict :key "e" :modifiers (list :primary :shift)))
      (application-menu-entry "file-menu-import" "Import Samples…" "" (lambda () (import-samples)))
      (application-menu-entry "file-menu-import-package" "Import Package…" "" (lambda () (import-package)))
      (application-menu-entry "file-menu-export-package" "Export Package…" "" (lambda () (export-package)))
      (application-menu-entry "file-menu-settings" "Settings…" "," (lambda () (show-settings)))
      (application-menu-entry "file-menu-customize" "Customize…" "" (lambda () (eseq.customize/customize)))
      (application-menu-entry "file-menu-open-init" "Open init.lisp" "" (lambda () (open-init)))
      (application-menu-entry "file-menu-open-scratch" "Open Project Scratch" "" (lambda () (open-project-scratch)))
      nil
      (application-menu-entry "file-menu-help" "Help" "" (lambda () (open-help)))
      (application-menu-entry "file-menu-about" "About eseq" "" (lambda () (about-eseq)))
      nil
      ;; No shortcut: Cmd+Q lives on the application menu's Quit item. Both
      ;; go through the unsaved-changes prompt.
      (application-menu-entry "file-menu-quit" "Quit eseq" "" (lambda () (quit-eseq))))
    (if (= name "Create")
      (list
        (application-menu-entry "create-menu-instrument" "Create Instrument…" ""
          (lambda () (eseq.browser/enter-new-instrument-editor)))
        (application-menu-entry "create-menu-effect" "Create Effect…" ""
          (lambda () (eseq.browser/enter-new-effect-editor)))
        nil
        (application-menu-entry "create-menu-bus" "Bus" "" (lambda () (add-bus)))
        (application-menu-entry "create-menu-midi" "MIDI Track" "" (lambda () (add-midi-track)))
        (application-menu-entry "create-menu-sampler" "Sampler Track" ""
          (lambda () (eseq.browser/add-sampler-track)))
        (application-menu-entry "create-menu-drum-rack" "Drum Rack" "" (lambda () (add-drum-rack)))
        (application-menu-entry "create-menu-layer-rack" "Layer Rack" "" (lambda () (add-layer-rack)))
        nil
        (application-menu-entry "create-menu-scene" "Scene" "" (lambda () (add-scene)))
        (application-menu-entry "create-menu-scene-bank" "Scene Bank" "" (lambda () (add-scene-bank))))
      (if (= name "Pattern")
        (list
          (application-menu-entry "pattern-menu-double" "Double Pattern Length" "+" (lambda () (double-pattern-length)))
          (application-menu-entry "pattern-menu-half" "Half Pattern Length" "-" (lambda () (half-pattern-length)))
          nil
          (application-menu-entry "pattern-menu-clone" "Clone Track Pattern" "" (lambda () (clone-track-pattern)))
          (application-menu-entry "pattern-menu-capture-midi" "Capture MIDI…" "" (lambda () (capture-midi)))
          (application-menu-entry "pattern-menu-capture-resample" "Resample Last 30 s…" "" (lambda () (capture-resample)))
          nil
          (application-menu-entry "pattern-menu-left" "Shift Steps Left" "" (lambda () (shift-steps-left)))
          (application-menu-entry "pattern-menu-right" "Shift Steps Right" "" (lambda () (shift-steps-right)))
          (dict :id "pattern-menu-transpose" :label "Transpose" :items
            (map (lambda (choice)
              (application-menu-entry (str "pattern-transpose-" (get choice :value)) (get choice :label) ""
                (lambda () (transpose-pattern (get choice :value)))))
              (transpose-choices)))
          nil
          (application-menu-entry "pattern-menu-clear" "Clear Track Pattern…" "" (lambda () (clear-track-pattern))))
        (list)))))


;; The current track's audio effects and every bus's.
(def audio-effects ()
  (let ((t selection.track)
        (track-effects (if t (filter (lambda (d) (= d.role "effect")) t.devices) (list))))
    (reduce (lambda (all b) (append all b.devices)) track-effects (buses))))

;; The effect armed for deletion (its header selected) when the effect
;; editor opens it (no built-in), or nil.
(def selected-editable-effect ()
  (first (filter (lambda (d) (and d.delete-target (not d.builtin))) (audio-effects))))

(def edit-selected-effect ()
  (let ((d (selected-editable-effect)))
    (when d
      (eseq.effects.panel-frame/fx-clear-selected-effect)
      (host-command "enter-edit-effect"
        (if d.bus
          (dict :name d.name :slot d.slot :bus d.bus.index)
          (dict :name d.name :slot d.slot))))))

;; Whether the current track plays a custom (editable) instrument. Menu
;; predicates answer true or false (a menu's :enabled is a boolean).
(def custom-instrument? ()
  (let ((t selection.track))
    (and (not (= t nil)) (= t.instrument-type "custom"))))

(def restore-default-layout ()
  :doc "Show the browser and mixer and reset panel sizes."
  :command "Restore Default Layout"
  :group "View"
  (set! eseq.seq-core-state/samples-sidebar-visible true)
  (set! eseq.seq-core-state/mixer-panel-visible true)
  (set! eseq.seq-core-state/lower-panel-visible true)
  (set! eseq.seq-core-state/patch-macros-panel-visible true)
  (set! eseq.seq-step-tabs/piano-roll-placement :bottom)
  (eseq.seq-panels/seq-show-sequencer-main)
  (eseq.seq-layout/apply-fx-layout))

(def install-default-menus ()
  (menus/register-menu
    (dict :id "application" :label "eseq" :native-only true
      :items (list
        (dict :id "app-about" :label "About eseq"
          :on-select (lambda () (about-eseq)))
        nil
        (dict :id "app-services" :label "Services" :role :services)
        nil
        (dict :id "app-hide" :label "Hide eseq" :role :hide)
        (dict :id "app-hide-others" :label "Hide Others" :role :hide-others)
        (dict :id "app-show-all" :label "Show All" :role :show-all)
        nil
        (dict :id "app-quit" :label "Quit eseq" :role :quit
          :shortcut (dict :key "q" :modifiers (list :primary))))))
  (menus/register-menu (dict :id "File" :label "File" :enabled-when commands-enabled?
    :items (application-menu-items "File")))
  (menus/register-menu (dict :id "Edit" :label "Edit"
    :items (append
      (map (lambda (entry)
        (merge
          (application-menu-entry (str "edit-menu-" (nth entry 0)) (nth entry 1) (nth entry 2)
            (lambda () (host-command "menu-edit" (dict :action (nth entry 0)))))
          :enabled-when edit-commands-enabled?))
        (list (list "cut" "Cut" "x") (list "copy" "Copy" "c")
              (list "paste" "Paste" "v") (list "delete" "Delete" "")
              (list "select-all" "Select All" "a")))
      (list nil
      (dict :id "edit-menu-effect" :label "Edit Selected Effect…"
        :enabled-when (lambda () (and (commands-enabled?) (not (= (selected-editable-effect) nil))))
        :on-select (lambda () (edit-selected-effect)))
      (dict :id "edit-menu-instrument" :label "Edit Instrument…"
        :enabled-when (lambda ()
          (and (commands-enabled?) (< eseq.seq-core-state/selected-bus 0)
               (custom-instrument?)
               (not (= browser.instrument ""))))
        :on-select (lambda ()
          (host-command "enter-edit-instrument"
            (dict :name browser.instrument))))
      nil
      ;; Pick up instrument/effect files written outside the app, e.g. by a
      ;; coding agent (eseq-63j4.4). New folders and ui.lisp edits are picked
      ;; up by the file watcher; DSP running on a track needs these.
      (dict :id "edit-menu-reload-instrument" :label "Reload Instrument From Disk"
        :enabled-when (lambda ()
          (and (commands-enabled?) (< eseq.seq-core-state/selected-bus 0)
               (custom-instrument?)))
        :on-select (lambda ()
          (let ((t selection.track))
            (when t
              (host-command "reload-instrument-from-disk" (dict :track t.index))))))
      (dict :id "edit-menu-reload-effect" :label "Reload Selected Effect From Disk"
        :enabled-when (lambda ()
          (and (commands-enabled?)
               (let ((d (selected-editable-effect)))
                 (and (not (= d nil)) (= d.bus nil)))))
        :on-select (lambda ()
          (let ((d (selected-editable-effect)))
            (when d
              (host-command "reload-effect-from-disk" (dict :slot d.slot))))))
      (dict :id "edit-menu-rescan-library" :label "Rescan Instruments & Effects"
        :enabled-when commands-enabled?
        :on-select (lambda () (host-command "reload-content-library" (dict))))))))
  (menus/register-menu (dict :id "Create" :label "Create" :enabled-when commands-enabled?
    :items (map (lambda (item)
      (if (and item (= (get item :id) "create-menu-effect"))
        (merge item :enabled-when has-tracks?) item))
      (application-menu-items "Create"))))
  (menus/register-menu (dict :id "Pattern" :label "Pattern" :enabled-when pattern-commands-enabled?
    :items (map (lambda (item)
      (if (and item (not (= (get item :id) "pattern-menu-double"))
                    (not (= (get item :id) "pattern-menu-half"))
                    (not (= (get item :id) "pattern-menu-capture-midi"))
                    (not (= (get item :id) "pattern-menu-capture-resample")))
        (merge item :enabled-when track-pattern-enabled?) item))
      (application-menu-items "Pattern")))))

(install-default-menus)
(menus/register-menu (dict :id "View" :label "View" :enabled-when commands-enabled?
  :items (list
    (application-menu-entry "view-sequencer" "Sequencer" "" (lambda () (show-sequencer)))
    (application-menu-entry "view-arrangement" "Arrangement" "" (lambda () (show-arrangement)))
    (application-menu-entry "view-piano-roll" "Piano Roll" "" (lambda () (show-piano-roll)))
    nil
    (dict :id "view-browser" :label "Show Browser" :checked-when (lambda () (browser-visible?))
      :on-select (lambda () (toggle-browser)))
    (dict :id "view-mixer" :label "Show Mixer" :checked-when (lambda () (mixer-visible?))
      :on-select (lambda () (toggle-mixer)))
    (dict :id "view-fx" :label "Show Track FX" :checked-when (lambda () (track-fx-visible?))
      :on-select (lambda () (toggle-track-fx)))
    (dict :id "view-macros" :label "Show Patch Macros" :checked-when (lambda () (patch-macros-visible?))
      :on-select (lambda () (toggle-patch-macros)))
    nil
    (application-menu-entry "view-collapse" "Collapse All Tracks" "" (lambda () (collapse-all-tracks)))
    (application-menu-entry "view-reset" "Restore Default Layout" "" (lambda () (restore-default-layout))))))
(menus/register-menu (dict :id "Help" :label "Help" :enabled-when commands-enabled?
  :items (list
    (application-menu-entry "help-search" "Search Commands…" "" (lambda () (host-command "menu-search-commands" (dict))))
    (application-menu-entry "help-shortcuts" "Keyboard Shortcuts" "" (lambda () (host-command "menu-keyboard-shortcuts" (dict))))
    (application-menu-entry "help-manual" "Documentation" "" (lambda () (open-help)))
    nil
    ;; Where a coding agent writes new instruments and effects (eseq-63j4).
    (application-menu-entry "help-authoring" "Open Authoring Folder in Terminal" "" (lambda () (host-command "menu-open-authoring-folder" (dict)))))))
