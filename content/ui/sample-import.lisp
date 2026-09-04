;; ui/sample-import.lisp -- the drag-and-drop sample import modal.
;;
;; Dropping audio files or folders onto the window stages them in Rust
;; (src/ui/sample_import_ui.rs) and opens this panel over the main tile.
;; The panel is a view over that draft: every read goes through a
;; `seq-sample-import-*` native, every edit calls a mutating native and
;; then bumps `generation` so the body re-renders. Nothing about the staged
;; samples is duplicated into Lisp state -- only the text-input drafts, which
;; fire per keystroke and must not touch the draft until Enter.
;;
;; The left side is a `tree` of the dropped folders (only expanded rows
;; are laid out, so a thousand-file drop stays cheap); the right side shows
;; whatever tree node is selected. Tagging has two layers, each with its
;; own entry field:
;;   batch     -- tags every imported sample receives ("808" for a folder
;;                of 808s). The dropped folder names are offered as
;;                one-click suggestions.
;;   selection -- tags for the selected node: a folder tags its whole
;;                subtree (click "claps", type "clap"), a file tags just
;;                that sample and also exposes its title.
;; Every entry field autocompletes against the library's existing tags plus
;; whatever was typed during this import, and a typed tag that matches an
;; existing one ignoring case adopts the existing spelling. Free text still
;; works: Enter adds exactly what was typed.
;;
;; Mounted by both step-panel buffers (*sequencer* and *arrangement*), which
;; never share a tile; a modal only receives pointer input through the
;; active tile, so Rust activates the mount's tile before calling `open`.
(module eseq.sample-import)

(export open?
        generation
        open
        close
        panel
        ;; Read by the layout test.
        title-draft
        ;; Actions, exported so tests can drive the modal by name.
        select-node
        add-batch-tag
        add-selection-tag
        add-file-tag)

(defstate open? false)
;; Bumped after every draft mutation: the natives are plain calls and are
;; not reactive on their own.
(defstate generation 0)

;; Text-input drafts (per keystroke; committed on Enter or a chip click).
(defstate batch-draft "")
(defstate selection-draft "")
(defstate title-draft "")
;; Preview strip for the selected file, same shape as the sample browser's:
;; the decoded waveform map from `seq-sample-waveform` (false while the
;; selection is a folder or undecodable) and the headphone toggle. With the
;; headphone on, selecting a file plays it once. `preview-headphone-icon` is
;; the browser's defwidget; widget names do not module-qualify.
(defstate preview-path "")
(defstate preview-buffer false)
(defstate auto-preview false)
;; The title draft is reseeded from the staged title whenever the selected
;; node changes (`select-node`, `open`), never during render.
(def reset-drafts ()
  (set! batch-draft "")
  (set! selection-draft "")
  (set! title-draft ""))

(def seed-title-draft ()
  (let ((file (get (seq-sample-import-selection) :file)))
    (set! title-draft (if (= file nil) "" (get file :title)))))

(def open ()
  (reset-drafts)
  (seq-sample-import-select "")
  (seed-title-draft)
  (set! generation (+ generation 1))
  (set! open? true))

(def stop-preview ()
  (if SEQ.browser-preview-playing
    (host-command "stop-sample-preview" (dict))
    nil))

(def sync-preview ()
  (let ((file (get (seq-sample-import-selection) :file))
      (path (if (= file nil) "" (get file :path))))
    (if (= path preview-path) nil
      (do
        (set! preview-path path)
        (set! preview-buffer (if (= path "") false (seq-sample-waveform path)))
        (if (and auto-preview preview-buffer)
          (host-command "preview-sample" (dict :path path))
          (stop-preview))))))

(def toggle-auto-preview ()
  (if auto-preview
    (do
      (set! auto-preview false)
      (stop-preview))
    (do
      (set! auto-preview true)
      (if preview-buffer
        (host-command "preview-sample" (dict :path preview-path))
        nil))))

(def close ()
  (reset-drafts)
  (stop-preview)
  (set! preview-path "")
  (set! preview-buffer false)
  (set! open? false))

(def bump ()
  (set! generation (+ generation 1)))

(def commit ()
  (host-command "sample-import-commit" (dict)))

(def cancel ()
  (host-command "sample-import-cancel" (dict)))

;; ── Palette ──

(def ready-color () (rgba 0.45 0.80 0.55 1.0))
(def dup-color () (rgba 0.95 0.70 0.30 1.0))
(def error-color () (rgba 0.95 0.40 0.40 1.0))
(def chip-bg () (rgba 1 1 1 0.08))
(def chip-border () (rgba 1 1 1 0.14))
(def section-bg () (rgba 1 1 1 0.035))
;; Preview strip ground: a touch lighter than the section, like the inputs.
(def strip-bg () (rgba 0.22 0.23 0.25 1.0))
(def row-bg () (rgba 1 1 1 0.02))
(def row-current-bg () (rgba 0.00 0.48 0.95 0.22))

;; ── Small pieces ──

;; A removable tag chip: the whole chip is the remove button.
(def tag-chip (key-prefix tag on-remove)
  (button (str tag "  x")
    :key (str key-prefix "-chip-" tag)
    :variant :ghost
    :font-size 10
    :height 1.05
    :padding 0.5
    :corner-radius 12
    :background-color (chip-bg)
    :border-color (chip-border)
    :color :fg
    :on-click |x y r| (on-remove tag)))

;; A suggestion chip: click adds the suggested (existing) tag.
(def suggestion-chip (key-prefix tag on-add)
  (button tag
    :key (str key-prefix "-suggest-" tag)
    :variant :ghost
    :font-size 10
    :height 1.05
    :padding 0.5
    :corner-radius 12
    :background-color (chip-bg)
    :border-color (chip-border)
    :color :fg
    :on-click |x y r| (on-add tag)))

(def chip-row (key-prefix tags on-remove)
  (if (> (len tags) 0)
    (wrap :key (str key-prefix "-chips") :width :fill :gap 0.2 :row-gap 0.15 :align :center
      (each tags |tag|
        (tag-chip key-prefix tag on-remove)))
    (label "none yet"
      :key (str key-prefix "-empty")
      :font-size 9 :color :dim :bg :transparent)))

(def suggestion-row (key-prefix draft exclude on-add)
  (let ((suggestions (seq-sample-import-tag-suggestions draft exclude)))
    (if (> (len suggestions) 0)
      (wrap :key (str key-prefix "-suggestions") :width :fill :gap 0.2 :row-gap 0.15 :align :center
        (each suggestions |tag|
          (suggestion-chip key-prefix tag on-add)))
      (box :width 0 :height 0 :bg :transparent))))

;; One tag entry field: `draft` is the current text, `set-draft` stores a
;; keystroke, `on-add` commits a tag (typed or suggested). Enter adds the
;; typed text; Escape clears the field (and never reaches the modal).
(def tag-entry (key-prefix draft set-draft exclude on-add placeholder)
  (v-stack :width :fill :gap 0.2
    (text-input
      :key (str key-prefix "-input")
      :width :fill
      :height 1.35
      :font-size 11
      :value draft
      :placeholder placeholder
      :on-change (lambda (v) (set-draft v))
      :on-submit (lambda () (on-add draft))
      :on-cancel (lambda () (set-draft "")))
    (suggestion-row key-prefix draft exclude on-add)))

(def section-title (key text)
  (label text
    :key key
    :font-size 11 :color :white :bg :transparent))

(def section-hint (key text)
  (label text
    :key key
    :font-size 8.5 :color :dim :bg :transparent))

;; ── Batch tags (every import) ──

(def add-batch-tag (tag)
  (if (seq-sample-import-add-batch-tag tag)
    (do (set! batch-draft "") (bump))
    nil))

(def remove-batch-tag (tag)
  (seq-sample-import-remove-batch-tag tag)
  (bump))

(def batch-section (summary)
  (let ((batch (get summary :batch-tags))
      (suggested (get summary :suggested-tags))
      (total (get summary :total)))
    (box :key "batch-section" :width :fill :padding 0.4 :corner-radius 8
      :background-color (section-bg)
      (v-stack :width :fill :gap 0.3
        (section-title "batch-title" (str "Tag all " total " samples"))
        (section-hint "batch-hint" "Every imported sample gets these.")
        (chip-row "batch" batch (lambda (tag) (remove-batch-tag tag)))
        (if (> (len suggested) 0)
          (wrap :key "batch-folder-suggestions" :width :fill :gap 0.2 :row-gap 0.15 :align :center
            (each suggested |tag|
              (suggestion-chip "batch-folder" tag (lambda (t) (add-batch-tag t)))))
          (box :width 0 :height 0 :bg :transparent))
        (tag-entry "batch" batch-draft
          (lambda (v) (set! batch-draft v))
          batch
          (lambda (tag) (add-batch-tag tag))
          "add a tag for all, Enter to apply")))))

;; ── Selection (the tree node on the left) ──

(def select-node (node)
  (seq-sample-import-select node)
  (set! selection-draft "")
  (seed-title-draft)
  (sync-preview)
  (bump))

(def add-selection-tag (tag)
  (if (seq-sample-import-add-selection-tag tag)
    (do (set! selection-draft "") (bump))
    nil))

(def remove-selection-tag (tag)
  (seq-sample-import-remove-selection-tag tag)
  (bump))

;; Single-file edits go by staged index (the file pane's own entry).
(def add-file-tag (index tag)
  (if (seq-sample-import-add-tag index tag)
    (do (set! selection-draft "") (bump))
    nil))

(def remove-file-tag (index tag)
  (seq-sample-import-remove-tag index tag)
  (bump))

(def commit-title (index)
  (seq-sample-import-set-title index title-draft)
  (bump))

(def status-text (entry)
  (let ((status (get entry :status)))
    (if (= status "ready") "ready"
      (if (= status "duplicate") "already in library"
        (str "failed: " (get entry :error))))))

(def status-color (entry)
  (let ((status (get entry :status)))
    (if (= status "ready") (ready-color)
      (if (= status "duplicate") (dup-color) (error-color)))))

(def selection-title (selection)
  (let ((kind (get selection :kind))
      (count (get selection :count)))
    (if (= kind "file")
      (get selection :label)
      (str (if (= kind "all") "Everything" (get selection :label))
        " · " count " sample" (if (= count 1) "" "s")))))

;; Folder / everything: shared tags + an entry that tags the whole subtree,
;; then a capped list of the samples inside.
(def folder-pane (selection)
  (let ((tags (get selection :tags))
      (count (get selection :count))
      (kind (get selection :kind)))
    (v-stack :width :fill :gap 0.3
      (section-hint "selection-hint"
        (if (= kind "all")
          (str "Pick a folder or file on the left, or tag all " count " samples here.")
          (str "Tags below apply to all " count " sample" (if (= count 1) "" "s") " in this folder.")))
      (chip-row "selection" tags (lambda (tag) (remove-selection-tag tag)))
      (tag-entry "selection" selection-draft
        (lambda (v) (set! selection-draft v))
        tags
        (lambda (tag) (add-selection-tag tag))
        (if (= kind "all") "tag every sample, Enter to apply" "tag this folder, Enter to apply")))))

;; The browser's preview strip (ui/browser.lisp `sample-preview-strip`):
;; headphone toggle + waveform with the shared preview playhead.
(def preview-strip ()
  (if preview-buffer
    ;; The browser strip sits on :buffer-bg (near black); inside the modal
    ;; that reads as a hole, so the strip takes the input-field gray instead.
    (box :key "preview-strip" :width :fill :height 1.5
      :background-color (strip-bg) :corner-radius 8 :padding 0.03
      (h-stack :width :fill :gap 0.35 :align :baseline
        (box :key "preview-headphone" :width 2.3 :height 2.2 :align :center
          :on-click |x y r| (toggle-auto-preview)
          (preview-headphone-icon :active (if auto-preview 1 0)))
        (box :width 0 :flex 1 :height 2.3
          (subtree :key (str "preview-wave-" preview-path)
            (waveform
              :height 2
              :header-height 0
              :bg (strip-bg)
              :waveform-color :dim
              :grid-major-color :transparent
              :grid-minor-color :transparent
              :inactive-waveform-color '(rgba 0.25 0.25 0.25 1)
              :view-start 0
              :view-duration (get preview-buffer :duration)
              :selection-start 0
              :selection-end (get preview-buffer :duration)
              :playhead-time (bind-seq "browser-preview-playhead")
              :buffer preview-buffer)))))
    (label "no preview for this file"
      :key "preview-empty"
      :font-size 8.5 :color :dim :bg :transparent)))

;; Single file: title, preview, and its own tags.
(def file-pane (entry)
  (let ((index (get entry :index))
      (tags (get entry :tags)))
    (v-stack :width :fill :gap 0.3
      (h-stack :width :fill :gap 0.3 :align :center
        (text-input
          :key "title-input"
          :flex 1
          :height 1.35
          :font-size 11
          :value title-draft
          :placeholder "title"
          :on-change (lambda (v) (set! title-draft v))
          :on-submit (lambda () (commit-title index)))
        (button "ok"
          :key "title-ok"
          :font-size 10 :height 1.35 :padding 0.6
          :on-click |x y r| (commit-title index)))
      (preview-strip)
      (chip-row "file" tags (lambda (tag) (remove-file-tag index tag)))
      (tag-entry "file" selection-draft
        (lambda (v) (set! selection-draft v))
        tags
        (lambda (tag) (add-file-tag index tag))
        "add a tag to this sample"))))

(def selection-section (selection)
  (box :key "selection-section" :width :fill :padding 0.4 :corner-radius 8
    :background-color (section-bg)
    (v-stack :width :fill :gap 0.3
      (h-stack :width :fill :gap 0.3 :align :center
        (section-title "selection-title" (selection-title selection))
        (box :flex 1 :bg :transparent)
        (if (= (get selection :kind) "file")
          (label (status-text (get selection :file))
            :key "file-status"
            :font-size 9 :color (status-color (get selection :file)) :bg :transparent)
          (box :width 0 :height 0 :bg :transparent)))
      (if (= (get selection :kind) "file")
        (file-pane (get selection :file))
        (folder-pane selection)))))

;; ── Tree ──

(def import-tree ()
  (scroll :key "tree-scroll" :width :fill :flex 1
    (tree :key "import-tree"
      :width :fill
      :row-bg-alt (rgba 1 1 1 0.03)
      :items (seq-sample-import-tree)
      :font-size 11
      :focusable true
      :selected-path (get (seq-sample-import-selection) :node)
      :selection-follows-external true
      ;; Folder rows fire on-toggle on click (expand/collapse); file rows
      ;; fire on-select. Both select the node.
      :on-toggle (lambda (item) (select-node (get item :path)))
      :on-select (lambda (item) (select-node (get item :path))))))

;; ── Header ──

(def header (summary)
  (let ((ready (get summary :ready))
      (dups (get summary :duplicates))
      (failed (get summary :failed))
      (total (get summary :total)))
    (h-stack :key "header" :width :fill :gap 0.5 :align :center
      (label "Import samples"
        :key "import-title"
        :font-size 15 :color :white :bg :transparent)
      (label (str total " file" (if (= total 1) "" "s"))
        :key "import-count"
        :font-size 10 :color :dim :bg :transparent)
      (label (str ready " ready")
        :key "import-ready"
        :font-size 10 :color (ready-color) :bg :transparent)
      (if (> dups 0)
        (label (str dups " already in library")
          :key "import-dups"
          :font-size 10 :color (dup-color) :bg :transparent)
        (box :width 0 :height 0 :bg :transparent))
      (if (> failed 0)
        (label (str failed " failed")
          :key "import-failed"
          :font-size 10 :color (error-color) :bg :transparent)
        (box :width 0 :height 0 :bg :transparent))
      (box :flex 1 :bg :transparent)
      (button "Cancel"
        :key "cancel"
        :variant :ghost
        :font-size 11 :height 1.5 :padding 0.8
        :border-color (chip-border)
        :on-click |x y r| (cancel))
      (button (str "Import " ready)
        :key "import"
        :font-size 11 :height 1.5 :padding 0.9
        :on-click |x y r| (commit)))))

;; ── Body ──

(def body ()
  ;; Reading the generation is what re-renders after every draft edit.
  (let ((epoch generation)
      (summary (seq-sample-import-summary)))
    (if (= summary nil)
      (label "Nothing staged for import."
        :key "empty"
        :font-size 11 :color :dim :bg :transparent)
      (let ((selection (seq-sample-import-selection)))
        (v-stack :width :fill :height :fill :gap 0.45
          (header summary)
          ;; :stretch hands both columns the row's full height (an h-stack
          ;; otherwise sizes children to their measured height, and a scroll
          ;; measures to zero); each column's scroll then flexes inside it.
          (h-stack :width :fill :flex 1 :gap 0.6 :align :stretch
            (v-stack :width 34 :gap 0.3
              (section-hint "tree-hint" "Click a folder to tag everything in it, a file to edit just that one.")
              (import-tree))
            (scroll :key "side-scroll" :flex 1
              (v-stack :width :fill :gap 0.45
                (batch-section summary)
                (selection-section selection)))))))))

(def panel ()
  (modal :is-open open?
         :on-close (lambda () (cancel))
         :width-px 1240 :height-px 860
    (box :debug-name "sample-import-panel"
      :width :fill :height :fill :bg :transparent :padding 0.6
      ;; The natives are only consulted while open: closed, the modal has
      ;; zero footprint and the body is never built.
      (if open? (body) (box :width 0 :height 0 :bg :transparent)))))
