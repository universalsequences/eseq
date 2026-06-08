;; metal-seq-browser.lisp — Sample browser mode for Metal Sequencer
;; C-x s to open, type to filter, Enter to audition, +/= to add track, q to quit
;; Uses tree widget inside scroll container for hierarchical browsing.

;; ── State ──
(def sbrowser-filter (state ""))
(def sbrowser-source-buffer "")
(defstate sbrowser-mode "audition")
(defstate sbrowser-tab "samples")
(defstate sbrowser-project-name "")
(defstate sbrowser-last-track-index -1)
(defstate sbrowser-last-sidebar-sample "")
(defstate sbrowser-selected-sample "")
(defstate sbrowser-selected-tags (list))
(defstate sbrowser-auditioned-sample "")

;; Editor state for inline instrument/effect creation
(def sbrowser-editor-name (state ""))
;; Preset save state
(defstate sbrowser-preset-name "")
(defstate sbrowser-preset-save-mode "")  ;; "" or "save-preset"
(defstate sbrowser-preset-filter "")

(defwidget editor-spinner
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

(def sbrowser-editor-status-row (text color)
  (h-stack :width :fill :height 1.35 :gap 0.5 :align :center
    (editor-spinner :width 2.8 :height 1.15)
    (label text
      :font-size 9
      :color color
      :bg :transparent)))

(def sbrowser-editor-busy? ()
  (or SEQ.editor-canceling
    (= SEQ.editor-error "Preview compiling...")))

(def sbrowser-audition-mode? ()
  (= sbrowser-mode "audition"))

(def sbrowser-track-type-mode? ()
  (or (= sbrowser-mode "track-type")
    (and (= SEQ.num-tracks 0) (= sbrowser-mode "audition"))))

(def sbrowser-create-sampler-mode? ()
  (= sbrowser-mode "create-sampler"))

(def sbrowser-project-browser-mode? ()
  (= sbrowser-mode "project-browser"))

(def sbrowser-project-save-mode? ()
  (= sbrowser-mode "project-save"))

(def sbrowser-editor-mode? ()
  (not (= SEQ.editor-mode "")))

(def sbrowser-create-mode? ()
  (or (sbrowser-track-type-mode?) (sbrowser-create-sampler-mode?)))

(def sbrowser-mode-label ()
  (if (sbrowser-audition-mode?) "audition" "create"))

(def sbrowser-sync-track-search ()
  (let ((track-changed (not (= sbrowser-last-track-index SEQ.sidebar-track-index)))
        (sample-changed (not (= sbrowser-last-sidebar-sample SEQ.sidebar-selected-sample))))
  (if (or track-changed sample-changed)
    (do
      (set! sbrowser-last-track-index SEQ.sidebar-track-index)
      (set! sbrowser-last-sidebar-sample SEQ.sidebar-selected-sample)
      (if (and (sbrowser-audition-mode?) (= SEQ.sidebar-kind "sampler"))
        (do
          (set! sbrowser-selected-sample SEQ.sidebar-selected-sample)
          (if (and (or sample-changed track-changed) (= sbrowser-auditioned-sample SEQ.sidebar-selected-sample))
            (set! sbrowser-auditioned-sample "")
            (do
              (set! sbrowser-auditioned-sample "")
              (if (= sbrowser-tab "samples")
                (set! sbrowser-filter ""))
              (set! sbrowser-selected-tags
                (if (= SEQ.sidebar-selected-sample "")
                  (list)
                  (seq-sample-tags-for-path SEQ.sidebar-selected-sample)))))))))))

(def sbrowser-reset-to-audition ()
  (set! sbrowser-mode "audition")
  (set! sbrowser-filter "")
  (set! sbrowser-selected-tags (list)))

(def sbrowser-leave-create-mode ()
  (set! sbrowser-mode "audition")
  (set! sbrowser-filter "")
  (set! sbrowser-selected-tags (list)))

(def sbrowser-enter-create-track-mode ()
  (set! sbrowser-filter "")
  (set! sbrowser-mode "audition")
  (set! sbrowser-tab "instruments"))

(def sbrowser-toggle-create-track-mode ()
  (sbrowser-enter-create-track-mode))

(def sbrowser-enter-create-sampler-mode ()
  (set! sbrowser-filter "")
  (set! sbrowser-selected-tags (list))
  (set! sbrowser-mode "create-sampler")
  (set! sbrowser-tab "samples")
  (status "Create sampler track: choose a sample"))

(def sbrowser-open-project-browser ()
  (set! sbrowser-filter "")
  (set! sbrowser-mode "audition")
  (set! sbrowser-tab "projects"))

(def sbrowser-open-project-save ()
  (if (= SEQ.current-project-name "")
    (do
      (set! sbrowser-filter "")
      (set! sbrowser-project-name "")
      (set! sbrowser-mode "project-save"))
    (do
      (host-command "save-project" (dict :name SEQ.current-project-name))
      (status (str "Save project: " SEQ.current-project-name)))))

(def sbrowser-new-project ()
  (host-command "new-project" (dict))
  (set! sbrowser-filter "")
  (set! sbrowser-tab "projects")
  (status "New project"))

(def sbrowser-add-instrument-track (name)
  (host-command "add-track-instrument" (dict :name name))
  (set! sbrowser-tab "presets")
  (status (str "Add instrument track: " name)))

(def sbrowser-add-sampler-track ()
  (host-command "add-track-sampler" (dict))
  (set! sbrowser-tab "samples")
  (status "Add sampler track"))

(def sbrowser-add-modulator-track ()
  (host-command "add-track-modulator" (dict))
  (set! sbrowser-tab "instruments")
  (status "Add modulator track"))

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

(def sbrowser-audition (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sbrowser-auditioned-sample path)
        (host-command "audition-sample" (dict :path path))
        (status (str "Audition: " (get item :label))))
      (status (str (get item :label))))))

(def sbrowser-select-sample (item)
  (let ((path (get item :path)))
    (if path
      (set! sbrowser-selected-sample path)
      (status (str (get item :label))))))

(def sbrowser-activate-sample (item)
  (let ((path (get item :path)))
    (if path
      (if (or (sbrowser-create-sampler-mode?) (= SEQ.num-tracks 0))
        (sbrowser-add-track item)
        (if (= SEQ.sidebar-kind "sampler")
          (sbrowser-audition item)
          (status "Drop samples onto a sampler track or the new-track drop zone")))
      (status "Choose a sample file, not a folder"))))

(def sbrowser-sample-selected-path ()
  (if (= sbrowser-selected-sample "")
    SEQ.sidebar-selected-sample
    sbrowser-selected-sample))

(def sbrowser-add-track (item)
  (let ((path (get item :path)))
    (if path
      (do
        (set! sbrowser-auditioned-sample path)
        (host-command "add-track-sample" (dict :path path))
        (sbrowser-leave-create-mode)
        (status (str "Add track: " (get item :label))))
      (status "Select a sample file, not a folder"))))

(def sbrowser-select-item (item)
  (if (or (sbrowser-create-sampler-mode?) (= SEQ.num-tracks 0) (= SEQ.sidebar-kind "instrument"))
    (sbrowser-add-track item)
    (sbrowser-audition item)))

(def sbrowser-select-tab (name)
  (set! sbrowser-tab name)
  (set! sbrowser-filter "")
  (if (not (= name "samples"))
    (set! sbrowser-selected-tags (list))))

(def sbrowser-list-contains? (items value)
  (> (len (filter (lambda (item) (= item value)) items)) 0))

(def sbrowser-list-remove (items value)
  (filter (lambda (item) (not (= item value))) items))

(def sbrowser-toggle-tag (tag)
  (if (sbrowser-list-contains? sbrowser-selected-tags tag)
    (set! sbrowser-selected-tags (sbrowser-list-remove sbrowser-selected-tags tag))
    (set! sbrowser-selected-tags (append sbrowser-selected-tags (list tag)))))

(def sbrowser-clear-tags ()
  (set! sbrowser-selected-tags (list)))

(def sbrowser-set-search-filter (value)
  (if (and (= sbrowser-tab "samples") (not (= value sbrowser-filter)))
    (sbrowser-clear-tags))
  (set! sbrowser-filter value))

(def sbrowser-tag-chip (tag)
  (let ((name (get tag :name))
        (selected (get tag :selected)))
    (button name
      :variant :ghost
      :background-color (if selected :black "#26272b")
      :color (if selected :primary "#9ea1a8")
      :height 1.02
      :padding 0.32
      :font-size 9.0
      :on-click |x y r| (sbrowser-toggle-tag name))))

(def sbrowser-search-placeholder ()
  (if (= sbrowser-tab "samples") "Search samples..."
    (if (= sbrowser-tab "instruments") "Search instruments..."
      (if (= sbrowser-tab "audio-fx") "Search audio effects..."
        (if (= sbrowser-tab "midi-fx") "Search MIDI effects..."
          (if (= sbrowser-tab "presets") "Search presets..."
            "Search projects..."))))))

(def sbrowser-empty-message (message)
  (box :width :fill :height :fill :padding 1
    (label message
      :font-size 10
      :color :gray
      :bg :transparent)))

(def sbrowser-select-audio-effect (item)
  (let ((kind (get item :kind)) (label (get item :label)))
    (if (or (= kind "builtin-audio-effect") (= kind "custom-audio-effect"))
      (status (str label))
      (status "Open a section or choose an effect"))))

(def sbrowser-enter-new-effect-editor ()
  (set! sbrowser-editor-name "")
  (host-command "enter-new-effect-editor" (dict)))

(def sbrowser-activate-audio-effect (item)
  (let ((kind (get item :kind)) (name (get item :name)))
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
        (status "Open a section or choose an effect")))))

(def sbrowser-select-midi-effect (item)
  (let ((kind (get item :kind)) (label (get item :label)))
    (if (= kind "midi-effect")
      (status (str label))
      (status "Choose a MIDI effect"))))

(def sbrowser-activate-midi-effect (item)
  (let ((kind (get item :kind)) (name (get item :name)))
    (if (= kind "midi-effect")
      (do
        (host-command "add-midi-fx" (dict :name name))
        (status (str "Add MIDI FX: " name)))
      (status "Choose a MIDI effect"))))

(def sbrowser-load-preset (name)
  (host-command "load-instrument-preset" (dict :name name))
  (status (str "Load preset: " name)))

(def sbrowser-save-project ()
  (if (= (len sbrowser-project-name) 0)
    (status "Enter a project name")
    (do
      (host-command "save-project" (dict :name sbrowser-project-name))
      (sbrowser-reset-to-audition)
      (status (str "Save project: " sbrowser-project-name)))))

(def sbrowser-load-project (name)
  (host-command "load-project" (dict :name name))
  (sbrowser-reset-to-audition)
  (status (str "Open project: " name)))

(def sbrowser-select-project-for-save (item)
  (set! sbrowser-project-name (get item :label)))

;; ── Search bar widget ──

(def sbrowser-header ()
  (box :key "browser-header" :width :fill :height 2.0 :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (text-input
        :key "sbrowser-search-input"
        :width :fill
        :value sbrowser-filter
        :placeholder (sbrowser-search-placeholder)
        :on-change (lambda (v) (sbrowser-set-search-filter v))
        :height 1.5
        :font-size 12
        (mag-glass)))))

(def sbrowser-instrument-header ()
  (box :key "instrument-header" :width :fill :height 1.1 :padding 0.15
    (label (if (= SEQ.sidebar-instrument-display-name "") "Instrument" SEQ.sidebar-instrument-display-name)
      :font-size 12
      :color :white
      :bg :transparent)))

(def sbrowser-create-header ()
  (box :key "create-header" :width :fill :height 1.1 :padding 0.15
    (label "Create track"
      :font-size 12
      :color :white
      :bg :transparent)))

(def sbrowser-project-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.2
      (h-stack :width :fill :gap 0.5 :align :center
        (text-input
          :flex 1
          :value sbrowser-filter
          :placeholder "Search projects..."
          :on-change (lambda (v) (sbrowser-set-search-filter v))
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

(def sbrowser-project-save-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.5
      (text-input
        :width :fill
        :value sbrowser-project-name
        :placeholder "Project name..."
        :on-change (lambda (v) (set! sbrowser-project-name v))
        :height 1.5
        :font-size 12
        (mag-glass))
      (button "Save"
        :variant :primary
        :width 8.0
        :height 1.2
        :font-size 11
        :on-click |x y r| (sbrowser-save-project)
        :color :white))))

(def sbrowser-create-items ()
  (seq-saved-instrument-tree sbrowser-filter))

(def sbrowser-enter-new-instrument-editor ()
  (set! sbrowser-editor-name "")
  (host-command "enter-new-instrument-editor" (dict)))

(def sbrowser-select-create-item (item)
  (if (= (get item :kind) "sampler")
    (sbrowser-enter-create-sampler-mode)
    (if (= (get item :kind) "new-instrument")
      (sbrowser-enter-new-instrument-editor)
      (if (= (get item :kind) "instrument")
        (sbrowser-add-instrument-track (get item :name))
        (status "Open a folder or choose an instrument")))))

(def sbrowser-create-search-bar ()
  (box :key "create-search-bar" :width :fill :height 2.0 :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (text-input
        :flex 1
        :value sbrowser-filter
        :placeholder "Search instruments..."
        :on-change (lambda (v) (sbrowser-set-search-filter v))
        :height 1.5
        :font-size 12
        (mag-glass)))))

(def sbrowser-create-toolbar ()
  (box :width :fill :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (button "Sampler"
        :variant :secondary
        :icon :sampler
        :flex 1
        :height 1.3
        :font-size 10.5
        :on-click |x y r| (sbrowser-add-sampler-track)
        :color :white)
      (button "Mod"
        :variant :secondary
        :icon :waveform
        :flex 1
        :height 1.3
        :font-size 10.5
        :on-click |x y r| (sbrowser-add-modulator-track)
        :color :white))))

(def sbrowser-library-label ()
  (box :width :fill :padding 0.25
    (label "Library"
      :font-size 10
      :color :gray
      :bg :transparent)))

(def sbrowser-create-picker ()
  (v-stack :key "create-picker-panel" :width :fill :gap 0.5 :flex 1
    (sbrowser-create-search-bar)
    (sbrowser-create-toolbar)
    (sbrowser-library-label)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (scroll :key "create-picker-scroll" :width :fill :flex 1
        (tree
          :key "create-picker-tree"
          :width :fill
          :background-color :buffer-bg
          :items (sbrowser-create-items)
          :expand-all (not (= sbrowser-filter ""))
          :on-select (lambda (item) (sbrowser-select-create-item item))
          :on-activate (lambda (item) (sbrowser-select-create-item item)))))))

(def sbrowser-tab-button (name label)
  (button label
    :variant (if (= sbrowser-tab name) :primary :ghost)
    :width 8.5
    :height 1.25
    :font-size 8.8
    :on-click |x y r| (sbrowser-select-tab name)
    :color :white))

(def sbrowser-tabs ()
  (v-stack :key "browser-tabs" :width 9.0 :gap 0.25
    (sbrowser-tab-button "samples" "Samples")
    (sbrowser-tab-button "instruments" "Instr")
    (sbrowser-tab-button "audio-fx" "Audio FX")
    (sbrowser-tab-button "midi-fx" "MIDI FX")
    (sbrowser-tab-button "presets" "Presets")
    (sbrowser-tab-button "projects" "Projects")))

(def sbrowser-samples-panel ()
  (let ((browser (seq-sample-browser sbrowser-filter sbrowser-selected-tags)))
    (let ((tags (get browser :tags))
          (items (get browser :items)))
      (v-stack :key "samples-browser-panel" :width :fill :gap 0.35 :flex 1
        (box :key "sample-tag-filter" :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0.35
          (v-stack :width :fill :gap 0.35
            (if (> (len sbrowser-selected-tags) 0)
              (button "Clear"
                :variant :ghost
                :width :fill
                :height 1.15
                :font-size 9
                :on-click |x y r| (sbrowser-clear-tags)
                :color :white))
            (wrap :width :fill :gap 0.25 :row-gap 0.18 :align :center
              (each (range 0 (len tags)) |i|
                (sbrowser-tag-chip (nth tags i))))))
        (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
          (if (= (len items) 0)
            (sbrowser-empty-message
              (if (and (= sbrowser-filter "") (= (len sbrowser-selected-tags) 0))
                "Choose a tag or search samples."
                "No samples found."))
            (scroll :key "samples-tab-scroll" :width :fill :flex 1
              (tree
                :key "samples-tab-tree"
                :width :fill
                :focusable true
                :background-color :buffer-bg
                :items items
                :selected-path (sbrowser-sample-selected-path)
                :expand-all true
                :drag-type "sample"
                :on-select (lambda (item) (sbrowser-select-sample item))
                :on-cursor-change (lambda (item) (sbrowser-select-sample item))
                :on-activate (lambda (item) (sbrowser-activate-sample item))))))))))

(def sbrowser-instruments-panel ()
  (v-stack :key "instrument-tab-panel" :width :fill :gap 0.5 :flex 1
    (sbrowser-create-toolbar)
    (sbrowser-library-label)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (scroll :key "instruments-tab-scroll" :width :fill :flex 1
        (tree
          :key "instruments-tab-tree"
          :width :fill
          :background-color :buffer-bg
          :items (sbrowser-create-items)
          :expand-all (not (= sbrowser-filter ""))
          :on-select (lambda (item) (sbrowser-select-create-item item))
          :on-activate (lambda (item) (sbrowser-select-create-item item)))))))

(def sbrowser-audio-fx-toolbar ()
  (box :width :fill :padding 0.25
    (h-stack :width :fill :gap 0.5 :align :center
      (button "+ New Effect"
        :variant :secondary
        :flex 1
        :height 1.3
        :font-size 10.5
        :on-click |x y r| (sbrowser-enter-new-effect-editor)
        :color :white))))

(def sbrowser-audio-fx-panel ()
  (let ((items (seq-audio-effect-tree sbrowser-filter)))
    (v-stack :key "audio-fx-tab-panel" :width :fill :gap 0.5 :flex 1
      (sbrowser-audio-fx-toolbar)
      (sbrowser-library-label)
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (sbrowser-empty-message "No audio effects found.")
          (scroll :key "audio-fx-tab-scroll" :width :fill :flex 1
            (tree
              :key "audio-fx-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :expand-all (not (= sbrowser-filter ""))
              :focusable true
              :drag-type "audio-effect"
              :on-select (lambda (item) (sbrowser-select-audio-effect item))
              :on-cursor-change (lambda (item) (sbrowser-select-audio-effect item))
              :on-activate (lambda (item) (sbrowser-activate-audio-effect item)))))))))

(def sbrowser-midi-fx-panel ()
  (let ((items (seq-midi-effect-tree sbrowser-filter)))
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (if (= (len items) 0)
        (sbrowser-empty-message "No MIDI effects found.")
        (scroll :key "midi-fx-tab-scroll" :width :fill :flex 1
          (tree
            :key "midi-fx-tab-tree"
            :width :fill
            :background-color :buffer-bg
            :items items
            :expand-all (not (= sbrowser-filter ""))
            :focusable true
            :drag-type "midi-effect"
            :on-select (lambda (item) (sbrowser-select-midi-effect item))
            :on-cursor-change (lambda (item) (sbrowser-select-midi-effect item))
            :on-activate (lambda (item) (sbrowser-activate-midi-effect item))))))))

(def sbrowser-presets-tab-panel ()
  (v-stack :key "presets-tab-panel" :width :fill :gap 0.22 :padding 0.25 :flex 1
    (sbrowser-instrument-header)
    (if (= SEQ.sidebar-kind "instrument")
      (let ((items (seq-preset-tree SEQ.sidebar-presets sbrowser-filter)))
        (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
          (if (= (len items) 0)
            (sbrowser-empty-message "No presets found.")
            (scroll :key "presets-tab-scroll" :width :fill :flex 1
              (tree
                :key "presets-tab-tree"
                :width :fill
                :background-color :buffer-bg
                :items items
                :selected-label SEQ.sidebar-loaded-preset
                :expand-all false
                :on-select (lambda (item) (sbrowser-load-preset (get item :label)))
                :on-activate (lambda (item) (sbrowser-load-preset (get item :label))))))))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (sbrowser-empty-message "Presets are available for instrument tracks.")))))

(def sbrowser-projects-tab-panel ()
  (let ((items (seq-project-tree sbrowser-filter)))
    (v-stack :key "projects-tab-panel" :width :fill :gap 0.5 :flex 1
      (box :width :fill :padding 0.25
        (h-stack :width :fill :gap 0.5 :align :center
          (button "New Project"
            :key "project-new-button"
            :variant :secondary
            :flex 1
            :height 1.3
            :font-size 10.5
            :on-click |x y r| (sbrowser-new-project)
            :color :white)))
      (box :width :fill :padding 0.25
        (label "Projects"
          :font-size 10
          :color :gray
          :bg :transparent))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (sbrowser-empty-message "No projects found.")
          (scroll :key "projects-tab-scroll" :width :fill :flex 1
            (tree
              :key "projects-tab-tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :selected-label SEQ.current-project-name
              :expand-all false
              :on-select (lambda (item) (sbrowser-load-project (get item :label)))
              :on-activate (lambda (item) (sbrowser-load-project (get item :label))))))))))

(def sbrowser-active-tab-panel ()
  (if (= sbrowser-tab "samples") (sbrowser-samples-panel)
    (if (= sbrowser-tab "instruments") (sbrowser-instruments-panel)
      (if (= sbrowser-tab "audio-fx") (sbrowser-audio-fx-panel)
        (if (= sbrowser-tab "midi-fx") (sbrowser-midi-fx-panel)
          (if (= sbrowser-tab "presets") (sbrowser-presets-tab-panel)
            (sbrowser-projects-tab-panel)))))))

(def sbrowser-tabbed-content ()
  (h-stack :key "browser-tabbed-content" :width :fill :gap 0.5 :flex 1 :align :stretch
    (sbrowser-tabs)
    (box :key "browser-active-tab-panel" :width 0 :flex 1 :padding 0
      (sbrowser-active-tab-panel))))

(def sbrowser-main-panel ()
  (v-stack :key "tabbed-browser" :width :fill :height :fill :gap 0.45 :flex 1
    (sbrowser-header)
    (box :key "browser-content" :width :fill :height :fill :padding 0 :flex 1
      (sbrowser-instruments-panel))))

(def sbrowser-preset-search-bar ()
  (box :key "preset-search-bar" :width :fill :height 1.8 :padding 0.15
    (h-stack :width :fill :gap 0.5 :align :center
      (text-input
        :flex 1
        :value sbrowser-preset-filter
        :placeholder "Search presets..."
        :on-change (lambda (v) (set! sbrowser-preset-filter v))
        :height 1.5
        :font-size 12
        (mag-glass)))))

(def sbrowser-presets-panel ()
  (v-stack :key "preset-list-panel" :width :fill :gap 0.22 :flex 1
    (sbrowser-instrument-header)
    (sbrowser-preset-search-bar)
    (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
      (scroll :key "preset-list-scroll" :width :fill :flex 1
        (tree
          :key "preset-list-tree"
          :width :fill
          :background-color :buffer-bg
          :items (seq-preset-tree SEQ.sidebar-presets sbrowser-preset-filter)
          :selected-label SEQ.sidebar-loaded-preset
          :expand-all false
          :on-select (lambda (item) (sbrowser-load-preset (get item :label)))
          :on-activate (lambda (item) (sbrowser-load-preset (get item :label))))))))

(def sbrowser-projects-panel ()
  (let ((items (seq-project-tree sbrowser-filter)))
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
            :on-select (lambda (item) (sbrowser-load-project (get item :label)))
            :on-activate (lambda (item) (sbrowser-load-project (get item :label)))))))))

(def sbrowser-project-save-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

;; ── Preset save sidebar ──

(def sbrowser-preset-save-mode? ()
  (= sbrowser-preset-save-mode "save-preset"))

(def sbrowser-enter-preset-save ()
  (set! sbrowser-preset-name "")
  (set! sbrowser-preset-save-mode "save-preset"))

(def sbrowser-exit-preset-save ()
  (set! sbrowser-preset-save-mode ""))

(def sbrowser-preset-save-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.4
      (h-stack :width :fill :gap 0.5 :align :center
        (label "Save Preset"
          :font-size 12
          :color :white
          :bg :transparent)
        (box :bg :dark-gray :width 6 :height 1.5 :align :center
          :on-click |x y r| (sbrowser-exit-preset-save)
          (label "cancel"
            :font-size 9
            :color :gray
            :bg :transparent)))
      (text-input
        :width :fill
        :value sbrowser-preset-name
        :placeholder "preset name..."
        :on-change (lambda (v) (set! sbrowser-preset-name v))
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
            (host-command "save-preset" (dict :name sbrowser-preset-name :overwrite false))
            (sbrowser-exit-preset-save))
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
              (sbrowser-exit-preset-save))
          :color :white)
        (box)))))

(def sbrowser-preset-save-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

;; ── Editor sidebar panels ──

(def sbrowser-editor-macro-action? ()
  (or (= SEQ.editor-active-macro-action "save-to-library")
      (= SEQ.editor-active-macro-action "fork")))

(def sbrowser-editor-macro-action-label ()
  (if (= SEQ.editor-active-macro-action "fork")
    "Fork Macro"
    "Save Macro to Library"))

(def sbrowser-editor-header ()
  (box :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.4
      (h-stack :width :fill :gap 0.5 :align :center
        (label
          (if (sbrowser-editor-macro-action?) "Defmacro"
            (if (= SEQ.editor-mode "new-instrument") "New Instrument"
              (if (= SEQ.editor-mode "edit-instrument") "Edit Instrument"
                (if (= SEQ.editor-mode "new-effect") "New Effect"
                  (if (= SEQ.editor-mode "edit-effect") "Edit Effect"
                    "Editor")))))
          :font-size 12
          :color :white
          :bg :transparent)
        (box :bg :dark-gray :width 6 :height 1.5 :align :center
          :on-click |x y r|
            (if SEQ.editor-canceling nil (host-command "cancel-editor" (dict)))
          (label "cancel"
            :font-size 9
            :color (if SEQ.editor-canceling :dim :gray)
            :bg :transparent)))
      (if (sbrowser-editor-macro-action?)
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
          (label (sbrowser-editor-macro-action-label)
            :font-size 11
            :color :white
            :bg :transparent))
        (if (= SEQ.editor-mode "new-instrument")
        (v-stack :width :fill :gap 0.35
          (label "Draft patch"
            :font-size 9
            :color :gray
            :bg :transparent)
          (label (str "track " (+ SEQ.current-track 1))
            :font-size 11
            :color :white
            :bg :transparent)
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
        (sbrowser-editor-status-row "Canceling..." :gray)
        (if (= SEQ.editor-error "Preview compiling...")
          (sbrowser-editor-status-row SEQ.editor-error :gray)
          (if (not (= SEQ.editor-error ""))
            (label SEQ.editor-error
              :font-size 9
              :color :red
              :bg :transparent)
            (box))))
      ;; Save button
      (if (sbrowser-editor-busy?)
        (box :height 1.2)
        (button
          (if (sbrowser-editor-macro-action?)
            (sbrowser-editor-macro-action-label)
            (if (= SEQ.editor-mode "new-instrument")
            "Finalize"
            (if (= SEQ.editor-mode "new-effect")
              "Save & Add"
            "Save")))
          :variant :primary
          :width (if (sbrowser-editor-macro-action?) 13.5 10)
          :height 1.2
          :font-size 11
          :on-click |x y r|
            (if (sbrowser-editor-macro-action?)
              (host-command "save-active-editor-macro" (dict))
              (if (= SEQ.editor-mode "new-instrument")
              (host-command "save-new-instrument" (dict :name sbrowser-editor-name))
              (if (= SEQ.editor-mode "edit-instrument")
                (host-command "update-instrument" (dict :name SEQ.sidebar-instrument-name))
                (if (= SEQ.editor-mode "new-effect")
                  (host-command "save-new-effect" (dict :name sbrowser-editor-name))
                  (host-command "update-effect" (dict))))))
          :color :white)))))

(def sbrowser-editor-panel ()
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1))

;; ── Build widgets ──

(def sbrowser-build-widgets ()
  (do
    (sbrowser-sync-track-search)
    (if (sbrowser-editor-mode?)
      (list
        (sbrowser-editor-header)
        (sbrowser-editor-panel))
      (if (sbrowser-preset-save-mode?)
        (list
          (sbrowser-preset-save-header)
          (sbrowser-preset-save-panel))
        (if (sbrowser-project-save-mode?)
          (list
            (sbrowser-project-save-header)
            (sbrowser-project-save-panel))
          (list
            (sbrowser-header)
            (sbrowser-tabbed-content)))))))

;; ── Reactive rendering (like metal-seq-grid.lisp) ──

(def sbrowser-root-widget ()
  (v-stack :width :fill :gap 0.4 :padding 0.15 (sbrowser-build-widgets)))

(def sbrowser-refresh-buffer ()
  (render-widget-to-buffer "*samples*" (sbrowser-root-widget)))

(effect-buffer "*samples*"
  (sbrowser-root-widget))

;; ── Entry point: just switch to the buffer ──

(def sample-browser-here ()
  (set! sbrowser-source-buffer (current-buffer-name))
  (set! sbrowser-filter "")
  (set! sbrowser-mode "audition")
  (set! sbrowser-tab (if (= SEQ.sidebar-kind "instrument") "presets" "samples"))
  (switch-to-buffer "*samples*"))

(bind-key "C-x s" "sample-browser-here")
