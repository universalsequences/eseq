;; Edit this file or use eseq.menus/register-menu from user init/packages.
(module eseq.application-menus)
(import eseq.menus :as menus)
(import eseq.seq-core-state)
(export application-menu-items install-default-menus)

(def commands-enabled? () (not (get (native-menu-context) :blocked)))
(def edit-commands-enabled? ()
  (let ((context (native-menu-context)))
    (or (not (get context :blocked)) (get context :text-input))))
(def pattern-commands-enabled? ()
  (let ((context (native-menu-context)))
    (and (not (get context :blocked)) (get context :ui-view)
         (not (get context :text-input)) (> SEQ.num-tracks 0))))

(def track-pattern-enabled? ()
  (and (pattern-commands-enabled?) (< eseq.seq-core-state/selected-bus 0)))

(def application-menu-entry (id label shortcut action)
  (if (= shortcut "")
    (dict :id id :label label :on-select action)
    (dict :id id :label label :shortcut (dict :key shortcut :modifiers (list :primary)) :on-select action)))

(def application-menu-items (name)
  (if (= name "File")
    (list
      (application-menu-entry "file-menu-new-project" "New Project" "n"
        (lambda () (host-command "project-new-request" (dict))))
      nil
      (application-menu-entry "file-menu-save" "Save" "s" (lambda () (eseq.transport/file-menu-save)))
      (merge (application-menu-entry "file-menu-save-as" "Save As…" "s" (lambda () (eseq.transport/file-menu-save-as))) :shortcut (dict :key "s" :modifiers (list :primary :shift)))
      (application-menu-entry "file-menu-open-project" "Open Project…" "o" (lambda () (eseq.transport/file-menu-open-project)))
      (dict :id "file-recent" :label "Open Recent" :items-when
        (lambda ()
          (let ((project SEQ.current-project-name))
            (map (lambda (name)
              (application-menu-entry (str "file-recent-" name) name ""
                (lambda () (host-command "menu-open-recent" (dict :name name)))))
              (seq-recent-projects)))))
      nil
      (merge (application-menu-entry "file-menu-export" "Export Audio…" "e" (lambda () (eseq.export-song/export-song))) :shortcut (dict :key "e" :modifiers (list :primary :shift)))
      (application-menu-entry "file-menu-import" "Import Samples…" "" (lambda () (host-command "menu-import-samples" (dict))))
      (application-menu-entry "file-menu-import-package" "Import Package…" "" (lambda () (host-command "menu-import-package" (dict))))
      (application-menu-entry "file-menu-export-package" "Export Package…" "" (lambda () (host-command "menu-export-package" (dict))))
      (application-menu-entry "file-menu-settings" "Settings…" "," (lambda () (host-command "settings-open" (dict))))
      (application-menu-entry "file-menu-customize" "Customize…" "" (lambda () (host-command "customize-open" (dict))))
      nil
      (application-menu-entry "file-menu-help" "Help" "" (lambda () (host-command "open-help" (dict))))
      (application-menu-entry "file-menu-about" "About eseq" "" (lambda () (host-command "about-open" (dict)))))
    (if (= name "Create")
      (list
        (application-menu-entry "create-menu-instrument" "Create Instrument…" ""
          (lambda () (host-command "enter-new-instrument-editor" (dict))))
        (application-menu-entry "create-menu-effect" "Create Effect…" ""
          (lambda () (eseq.browser/enter-new-effect-editor)))
        nil
        (application-menu-entry "create-menu-bus" "Bus" ""
          (lambda () (host-command "add-bus" (dict))))
        (application-menu-entry "create-menu-midi" "MIDI Track" ""
          (lambda () (host-command "add-track-empty" (dict))))
        (application-menu-entry "create-menu-sampler" "Sampler Track" ""
          (lambda () (eseq.browser/add-sampler-track)))
        (application-menu-entry "create-menu-drum-rack" "Drum Rack" ""
          (lambda () (host-command "add-track-rack" (dict))))
        (application-menu-entry "create-menu-layer-rack" "Layer Rack" ""
          (lambda () (host-command "add-track-layer-rack" (dict))))
        nil
        (application-menu-entry "create-menu-scene" "Scene" ""
          (lambda () (host-command "clone-pattern" (dict))))
        (application-menu-entry "create-menu-scene-bank" "Scene Bank" ""
          (lambda () (eseq.transport/select-scene-bank "New bank"))))
      (if (= name "Pattern")
        (list
          (application-menu-entry "pattern-menu-double" "Double Pattern Length" "+"
            (lambda () (host-command "menu-pattern-double" (dict))))
          (application-menu-entry "pattern-menu-half" "Half Pattern Length" "-"
            (lambda () (host-command "menu-pattern-half" (dict))))
          nil
          (application-menu-entry "pattern-menu-clone" "Clone Track Pattern" ""
            (lambda () (host-command "menu-pattern-clone" (dict))))
          nil
          (application-menu-entry "pattern-menu-left" "Shift Steps Left" ""
            (lambda () (host-command "menu-pattern-transform" (dict :operation "left"))))
          (application-menu-entry "pattern-menu-right" "Shift Steps Right" ""
            (lambda () (host-command "menu-pattern-transform" (dict :operation "right"))))
          (dict :id "pattern-menu-transpose" :label "Transpose" :items
            (map (lambda (entry)
              (application-menu-entry (str "pattern-transpose-" (nth entry 0)) (nth entry 1) ""
                (lambda () (host-command "menu-pattern-transpose" (dict :semitones (nth entry 0))))))
              (list (list 1 "Up Semitone") (list -1 "Down Semitone") (list 12 "Up Octave") (list -12 "Down Octave"))))
          nil
          (application-menu-entry "pattern-menu-clear" "Clear Track Pattern…" ""
            (lambda () (host-command "menu-pattern-clear-request" (dict)))))
        (list)))))


(def selected-editable-effect ()
  ;; Read the version so selecting a different header updates the native item.
  (let ((version SEQ.delete-target-version)
        (effects (append SEQ.effects
          (reduce (lambda (all chain) (append all chain)) (list) SEQ.bus-effects)))
        (matches (filter (lambda (fx)
          (and (not (get fx :builtin))
            (seq-delete-target? :fx-effect
              (if (get fx :bus-fx)
                (dict :chain "bus" :bus (get fx :bus-idx) :slot (get fx :slot-idx))
                (dict :chain "audio" :slot (get fx :slot-idx)))))) effects)))
    (if (> (len matches) 0) (nth matches 0) nil)))

(def edit-selected-effect ()
  (let ((fx (selected-editable-effect)))
    (if fx
      (do
        (eseq.effects.panel-frame/fx-clear-selected-effect)
        (host-command "enter-edit-effect"
          (if (get fx :bus-fx)
            (dict :name (get fx :name) :slot (get fx :slot-idx) :bus (get fx :bus-idx))
            (dict :name (get fx :name) :slot (get fx :slot-idx))))) nil)))

(def restore-default-layout ()
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
          :on-select (lambda () (host-command "about-open" (dict))))
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
          (and (commands-enabled?) (< eseq.seq-core-state/selected-bus 0) (> SEQ.num-tracks 0)
               (= (nth SEQ.track-instrument-types SEQ.current-track) "custom")
               (not (= SEQ.sidebar-instrument-name ""))))
        :on-select (lambda ()
          (host-command "enter-edit-instrument"
            (dict :name SEQ.sidebar-instrument-name))))))))
  (menus/register-menu (dict :id "Create" :label "Create" :enabled-when commands-enabled?
    :items (map (lambda (item)
      (if (and item (= (get item :id) "create-menu-effect"))
        (merge item :enabled-when (lambda () (> SEQ.num-tracks 0))) item))
      (application-menu-items "Create"))))
  (menus/register-menu (dict :id "Pattern" :label "Pattern" :enabled-when pattern-commands-enabled?
    :items (map (lambda (item)
      (if (and item (not (= (get item :id) "pattern-menu-double"))
                    (not (= (get item :id) "pattern-menu-half")))
        (merge item :enabled-when track-pattern-enabled?) item))
      (application-menu-items "Pattern")))))

(install-default-menus)
(menus/register-menu (dict :id "View" :label "View" :enabled-when commands-enabled?
  :items (list
    (application-menu-entry "view-sequencer" "Sequencer" "" (lambda () (eseq.seq-panels/seq-show-sequencer-main)))
    (application-menu-entry "view-arrangement" "Arrangement" "" (lambda () (eseq.seq-panels/seq-open-arrangement)))
    (application-menu-entry "view-piano-roll" "Piano Roll" "" (lambda () (eseq.seq-panels/seq-open-piano-roll-preferred)))
    nil
    (dict :id "view-browser" :label "Show Browser" :checked-when (lambda () eseq.seq-core-state/samples-sidebar-visible)
      :on-select (lambda () (eseq.seq-panels/seq-toggle-samples-sidebar)))
    (dict :id "view-mixer" :label "Show Mixer" :checked-when (lambda () eseq.seq-core-state/mixer-panel-visible)
      :on-select (lambda () (eseq.seq-panels/seq-toggle-mixer-panel)))
    (dict :id "view-fx" :label "Show Track FX" :checked-when (lambda () eseq.seq-core-state/lower-panel-visible)
      :on-select (lambda () (eseq.seq-panels/seq-toggle-fx-panel)))
    (dict :id "view-macros" :label "Show Patch Macros" :checked-when (lambda () eseq.seq-core-state/patch-macros-panel-visible)
      :on-select (lambda () (eseq.seq-panels/seq-toggle-patch-macros-panel)))
    nil
    (application-menu-entry "view-collapse" "Collapse All Tracks" "" (lambda () (eseq.sequencer/collapse-all-tracks)))
    (application-menu-entry "view-reset" "Restore Default Layout" "" restore-default-layout))))
(menus/register-menu (dict :id "Help" :label "Help" :enabled-when commands-enabled?
  :items (list
    (application-menu-entry "help-search" "Search Commands…" "" (lambda () (host-command "menu-search-commands" (dict))))
    (application-menu-entry "help-shortcuts" "Keyboard Shortcuts" "" (lambda () (host-command "menu-keyboard-shortcuts" (dict))))
    (application-menu-entry "help-manual" "Documentation" "" (lambda () (host-command "open-help" (dict)))))))
