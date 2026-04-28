;; metal-seq-browser.lisp — Sample browser mode for Metal Sequencer
;; C-x s to open, type to filter, Enter to audition, +/= to add track, q to quit
;; Uses tree widget inside scroll container for hierarchical browsing.

;; ── State ──
(def sbrowser-filter (state ""))
(def sbrowser-source-buffer "")
(defstate sbrowser-mode "audition")
(defstate sbrowser-project-name "")

;; Editor state for inline instrument/effect creation
(def sbrowser-editor-name (state ""))
;; Preset save state
(defstate sbrowser-preset-name "")
(defstate sbrowser-preset-save-mode "")  ;; "" or "save-preset"

(def sbrowser-audition-mode? ()
  (= sbrowser-mode "audition"))

(def sbrowser-track-type-mode? ()
  (= sbrowser-mode "track-type"))

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

(def sbrowser-reset-to-audition ()
  (set! sbrowser-mode "audition")
  (set! sbrowser-filter ""))

(def sbrowser-enter-create-track-mode ()
  (set! sbrowser-filter "")
  (set! sbrowser-mode "track-type"))

(def sbrowser-toggle-create-track-mode ()
  (if (sbrowser-audition-mode?)
    (sbrowser-enter-create-track-mode)
    (sbrowser-reset-to-audition)))

(def sbrowser-enter-create-sampler-mode ()
  (set! sbrowser-filter "")
  (set! sbrowser-mode "create-sampler")
  (status "Create track: choose a sample"))

(def sbrowser-open-project-browser ()
  (if (sbrowser-project-browser-mode?)
    (sbrowser-reset-to-audition)
    (do
      (set! sbrowser-filter "")
      (set! sbrowser-mode "project-browser"))))

(def sbrowser-open-project-save ()
  (if (= SEQ.current-project-name "")
    (do
      (set! sbrowser-filter "")
      (set! sbrowser-project-name "")
      (set! sbrowser-mode "project-save"))
    (do
      (host-command "save-project" (dict :name SEQ.current-project-name))
      (status (str "Save project: " SEQ.current-project-name)))))

(def sbrowser-add-instrument-track (name)
  (host-command "add-track-instrument" (dict :name name))
  (sbrowser-reset-to-audition)
  (status (str "Add instrument track: " name)))

;; ── SDF widgets ──

(defwidget browser-panel-bg
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect (* width 1) (* height 1) 0.02)
              (material :color (rgba 0.16 0.16 0.17 1)))))

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
        (host-command "audition-sample" (dict :path path))
        (status (str "Audition: " (get item :label))))
      (status (str (get item :label))))))

(def sbrowser-add-track (item)
  (let ((path (get item :path)))
    (if path
      (do
        (host-command "add-track-sample" (dict :path path))
        (sbrowser-reset-to-audition)
        (status (str "Add track: " (get item :label))))
      (status "Select a sample file, not a folder"))))

(def sbrowser-select-item (item)
  (if (sbrowser-create-sampler-mode?)
    (sbrowser-add-track item)
    (sbrowser-audition item)))

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
  (box :padding 0.25
    (h-stack :gap 0.5 :align :center
      (text-input
        :value sbrowser-filter
        :placeholder "Search samples..."
        :on-change (lambda (v) (set! sbrowser-filter v))
        :width 30
        :height 1.5
        :font-size 12
        (mag-glass))
      (box :bg :dark-gray :width 8.2 :height 1.5 :align :center
        (label (sbrowser-mode-label)
          :font-size 9
          :color (if (sbrowser-audition-mode?) :gray :white)
          :bg :transparent)))))

(def sbrowser-instrument-header ()
  (box :padding 0.25
    (v-stack :gap 0.2
      (h-stack :gap 0.5 :align :center
        (label (if (= SEQ.sidebar-instrument-name "") "Instrument" SEQ.sidebar-instrument-name)
          :font-size 12
          :color :white
          :bg :transparent)
        (box :bg :dark-gray :width 8.2 :height 1.5 :align :center
          (label (sbrowser-mode-label)
            :font-size 9
            :color :gray
            :bg :transparent))
        ;; Edit button (only for custom instruments, not Sampler)
        (if (not (= SEQ.sidebar-instrument-name ""))
          (box :bg :dark-gray :width 5 :height 1.5 :align :center
            :on-click |x y r|
              (host-command "enter-edit-instrument"
                (dict :name SEQ.sidebar-instrument-name))
            (label "Edit"
              :font-size 9
              :color :white
              :bg :transparent))
          (box)))
      (label
        (str "Current preset: "
          (if (= SEQ.sidebar-loaded-preset "") "none" SEQ.sidebar-loaded-preset))
        :font-size 9
        :color :gray
        :bg :transparent))))

(def sbrowser-create-header ()
  (box :padding 0.25
    (h-stack :gap 0.5 :align :center
      (label "Create track"
        :font-size 12
        :color :white
        :bg :transparent)
      (box :bg :blue :width 8.2 :height 1.5 :align :center
        (label (sbrowser-mode-label)
          :font-size 9
          :color :white
          :bg :transparent)))))

(def sbrowser-project-header ()
  (box :padding 0.25
    (v-stack :gap 0.2
      (h-stack :gap 0.5 :align :center
        (text-input
          :value sbrowser-filter
          :placeholder "Search projects..."
          :on-change (lambda (v) (set! sbrowser-filter v))
          :width 30
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
  (box :padding 0.25
    (v-stack :gap 0.5
      (text-input
        :value sbrowser-project-name
        :placeholder "Project name..."
        :on-change (lambda (v) (set! sbrowser-project-name v))
        :width 30
        :height 1.5
        :font-size 12
        (mag-glass))
      (box :background "browser-pill-btn-bg" :width 8.0 :height 1.2
        :on-click |x y r| (sbrowser-save-project)
        (box :width 8.0 :height 1.2
          (v-stack :align :center
            (label " Save "
              :font-size 11
              :color :white
              :bg :transparent)))))))

(def sbrowser-create-items ()
  (append
    (list (dict :label "Sampler" :kind "sampler")
          (dict :label "+ New Instrument" :kind "new-instrument"))
    (map (lambda (name)
      (dict :label name :kind "instrument"))
      (seq-saved-instruments))))

(def sbrowser-select-create-item (item)
  (if (= (get item :kind) "sampler")
    (sbrowser-enter-create-sampler-mode)
    (if (= (get item :kind) "new-instrument")
      (do
        (set! sbrowser-editor-name "")
        (host-command "enter-new-instrument-editor" (dict)))
      (sbrowser-add-instrument-track (get item :label)))))

(def sbrowser-create-picker ()
  (box :background "browser-panel-bg" :padding 0 :flex 1
    (scroll :flex 1
      (tree
        :items (sbrowser-create-items)
        :expand-all false
        :on-select (lambda (item) (sbrowser-select-create-item item))
        :on-activate (lambda (item) (sbrowser-select-create-item item))))))

(def sbrowser-presets-panel ()
  (box :background "browser-panel-bg" :padding 0 :flex 1
    (if (= (len SEQ.sidebar-presets) 0)
      (box :padding 1
        (label "No presets found for this instrument."
          :font-size 10
          :color :gray
          :bg :transparent))
      (scroll :flex 1
        (tree
          :items SEQ.sidebar-preset-tree
          :selected-label SEQ.sidebar-loaded-preset
          :expand-all false
          :on-select (lambda (item) (sbrowser-load-preset (get item :label)))
          :on-activate (lambda (item) (sbrowser-load-preset (get item :label))))))))

(def sbrowser-projects-panel ()
  (let ((items (seq-project-tree sbrowser-filter)))
    (box :background "browser-panel-bg" :padding 0 :flex 1
      (if (= (len items) 0)
        (box :padding 1
          (label "No projects found."
            :font-size 10
            :color :gray
            :bg :transparent))
        (scroll :flex 1
          (tree
            :items items
            :selected-label SEQ.current-project-name
            :expand-all false
            :on-select (lambda (item) (sbrowser-load-project (get item :label)))
            :on-activate (lambda (item) (sbrowser-load-project (get item :label)))))))))

(def sbrowser-project-save-panel ()
  (box :background "browser-panel-bg" :padding 0 :flex 1))

;; ── Preset save sidebar ──

(def sbrowser-preset-save-mode? ()
  (= sbrowser-preset-save-mode "save-preset"))

(def sbrowser-enter-preset-save ()
  (set! sbrowser-preset-name "")
  (set! sbrowser-preset-save-mode "save-preset"))

(def sbrowser-exit-preset-save ()
  (set! sbrowser-preset-save-mode ""))

(def sbrowser-preset-save-header ()
  (box :padding 0.25
    (v-stack :gap 0.4
      (h-stack :gap 0.5 :align :center
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
        :value sbrowser-preset-name
        :placeholder "preset name..."
        :on-change (lambda (v) (set! sbrowser-preset-name v))
        :width 30
        :height 1.5
        :font-size 12)
      ;; Save as New button
      (box :background "browser-pill-btn-bg" :width 10 :height 1.2
        :on-click |x y r|
          (do
            (host-command "save-preset" (dict :name sbrowser-preset-name :overwrite false))
            (sbrowser-exit-preset-save))
        (box :width 10 :height 1.2
          (v-stack :align :center
            (label " Save as New "
              :font-size 11
              :color :white
              :bg :transparent))))
      ;; Overwrite button (only if a preset is currently loaded)
      (if (not (= SEQ.sidebar-loaded-preset ""))
        (box :bg :dark-gray :width 16 :height 1.2 :align :center
          :on-click |x y r|
            (do
              (host-command "overwrite-preset" (dict))
              (sbrowser-exit-preset-save))
          (label (str " Overwrite: " SEQ.sidebar-loaded-preset " ")
            :font-size 10
            :color :white
            :bg :transparent))
        (box)))))

(def sbrowser-preset-save-panel ()
  (box :background "browser-panel-bg" :padding 0 :flex 1))

;; ── Editor sidebar panels ──

(def sbrowser-editor-header ()
  (box :padding 0.25
    (v-stack :gap 0.4
      (h-stack :gap 0.5 :align :center
        (label
          (if (= SEQ.editor-mode "new-instrument") "New Instrument"
            (if (= SEQ.editor-mode "edit-instrument") "Edit Instrument"
              (if (= SEQ.editor-mode "new-effect") "New Effect"
                (if (= SEQ.editor-mode "edit-effect") "Edit Effect"
                  "Editor"))))
          :font-size 12
          :color :white
          :bg :transparent)
        (box :bg :dark-gray :width 6 :height 1.5 :align :center
          :on-click |x y r| (host-command "cancel-editor" (dict))
          (label "cancel"
            :font-size 9
            :color :gray
            :bg :transparent)))
      ;; Name input (only for new-* modes)
      (if (or (= SEQ.editor-mode "new-instrument") (= SEQ.editor-mode "new-effect"))
        (text-input
          :value sbrowser-editor-name
          :placeholder (if (= SEQ.editor-mode "new-instrument") "instrument-name" "effect-name")
          :on-change (lambda (v) (set! sbrowser-editor-name v))
          :width 30
          :height 1.5
          :font-size 12)
        ;; For edit modes, show the file name
        (label SEQ.editor-buffer-name
          :font-size 10
          :color :gray
          :bg :transparent))
      ;; Error display
      (if (not (= SEQ.editor-error ""))
        (label SEQ.editor-error
          :font-size 9
          :color :red
          :bg :transparent)
        (box))
      ;; Save button
      (box :background "browser-pill-btn-bg" :width 10 :height 1.2
        :on-click |x y r|
          (if (= SEQ.editor-mode "new-instrument")
            (host-command "save-new-instrument" (dict :name sbrowser-editor-name))
            (if (= SEQ.editor-mode "edit-instrument")
              (host-command "update-instrument" (dict :name SEQ.sidebar-instrument-name))
              (if (= SEQ.editor-mode "new-effect")
                (host-command "save-new-effect" (dict :name sbrowser-editor-name))
                (host-command "update-effect" (dict)))))
        (box :width 10 :height 1.2
          (v-stack :align :center
            (label
              (if (or (= SEQ.editor-mode "new-instrument") (= SEQ.editor-mode "new-effect"))
                " Save & Add "
                " Save ")
              :font-size 11
              :color :white
              :bg :transparent)))))))

(def sbrowser-editor-panel ()
  (box :background "browser-panel-bg" :padding 0 :flex 1))

;; ── Build widgets ──

(def sbrowser-build-widgets ()
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
      (if (sbrowser-project-browser-mode?)
        (list
          (sbrowser-project-header)
          (sbrowser-projects-panel))
        (if (sbrowser-track-type-mode?)
          (list
            (sbrowser-create-header)
            (sbrowser-create-picker))
        (if (= SEQ.sidebar-kind "instrument")
          (list
            (sbrowser-instrument-header)
            (sbrowser-presets-panel))
          (let ((header (sbrowser-header)))
            (list
              header
              (box :background "browser-panel-bg" :padding 0 :flex 1
                (scroll :flex 1
                  (tree
                    :items (seq-filter-sample-tree sbrowser-filter)
                    :selected-path SEQ.sidebar-selected-sample
                    :expand-all (not (= sbrowser-filter ""))
                    :on-select (lambda (item) (sbrowser-select-item item))
                    :on-activate (lambda (item) (sbrowser-select-item item))))))))))))))

;; ── Reactive rendering (like metal-seq-grid.lisp) ──

(effect-buffer "*samples*"
  (v-stack :gap 0.5 :padding 1 (sbrowser-build-widgets)))

;; ── Entry point: just switch to the buffer ──

(def sample-browser-here ()
  (set! sbrowser-source-buffer (current-buffer-name))
  (set! sbrowser-filter "")
  (set! sbrowser-mode "audition")
  (switch-to-buffer "*samples*"))

(bind-key "C-x s" "sample-browser-here")
