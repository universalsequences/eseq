;; ui/patch-macros.lisp — Macro sidebar for the patch editor.
;; Renders to *patch-macros* buffer: defmacros defined in the current patch
;; ("In Patch", nested by call structure) plus the saved defmacro library
;; ("Library"; macros imported by the patch get the :sliders icon), and file-
;; backed tensor assets from the draft, user, and factory tiers. Macro rows drag
;; as "dgen-macro"; asset rows override that with "dgen-asset". The blue
;; selected row always mirrors the macro view open in the patcher
;; (editor.open-macro); single-click an "In Patch" row to open that macro's
;; view. Library rows only open on double-click, so click-dragging one into
;; the patch does not navigate.
;;
;; The macros, assets and the selected asset are the host's `editor` kind
;; (kind-bindings spec §14.2i): `editor-macro`, `editor-asset` and
;; `asset-info` instances.
;;
;; MODULE NOTE (spec §10, S3b): this file is a RENDER ROOT — it registers the
;; *patch-macros* effect-buffer at top level. `import` EVALUATES its target, so
;; NEVER import this module from a library file; that would drag a UI root into
;; every VM that loads the importer. `patch-macros-items` and `macro-sidebar`
;; are exported for the Rust tests, which reach them by qualified name.
(module eseq.patch-macros)
(import eseq.kinds :refer (editor))
(import eseq.view-kit :refer (listed?))

(export macro-sidebar
        patch-macros-items)

;; The search box's text.
(def-kind macro-sidebar
  :key ()
  :state ((filter "")))

(def match? (name)
  (or (= macro-sidebar.filter "")
      (str-contains? name macro-sidebar.filter)))

(def find-macro (name ms)
  (first (filter (lambda (m) (= m.name name)) ms)))

(def lib-icon (m)
  (if m.used :sliders :dial))

;; :click-opens marks rows that jump to their macro view on a single click.
;; Only rows under "In Patch" set it — "Library" rows are primarily drag
;; sources, and opening a view mid-drag-start feels like a misfire.
(def macro-row (m kind icon click-opens kids)
  (let ((row (dict :label m.name
                   :name m.name
                   :kind kind
                   :icon icon
                   :click-opens click-opens
                   :drop-target false)))
    (if (empty? kids) row (merge row :children kids))))

;; Library macros can import other library macros; nest those too. Only
;; reachable from the "In Patch" call tree, so these rows open on click.
(def lib-item (m depth)
  (macro-row m "library-macro" (lib-icon m) true (child-items m.calls depth)))

;; ── Nested "In Patch" items: children = macros this macro's body calls. ──
;; Depth-capped so a (malformed) cyclic call graph cannot recurse forever.

(def call-item (c depth)
  (let ((local (find-macro c editor.patch-macros)))
    (if local
      (local-item local depth)
      (let ((lib (find-macro c editor.library-macros)))
        (when lib (lib-item lib depth))))))

(def child-items (calls depth)
  (if (> depth 4)
    (list)
    (filter (lambda (x) (not (= x nil)))
      (map (lambda (c) (call-item c (+ depth 1))) calls))))

(def local-item (m depth)
  (macro-row m "patch-macro" :dial true (child-items m.calls depth)))

;; Every name a local macro calls (built once per render).
(def called-names ()
  (reduce (lambda (all m) (append all m.calls)) (list) editor.patch-macros))

(def header-row (label)
  (dict :label label :kind "header" :draggable false :drop-target false))

;; A section: its header over its rows, or nothing without rows.
(def section (title rows)
  (if (empty? rows)
    (list)
    (append (list (header-row title)) rows)))

;; Macros not called by another local macro are roots; called ones appear
;; nested under each caller.
(def nested-patch-section ()
  (let ((called (called-names)))
    (section "In Patch"
      (map (lambda (m) (local-item m 0))
           (filter (lambda (m) (not (listed? m.name called))) editor.patch-macros)))))

;; Search active: flatten the patch's macros to the matching rows.
(def flat-patch-section ()
  (section "In Patch"
    (map (lambda (m) (macro-row m "patch-macro" :dial true (list)))
         (filter (lambda (m) (match? m.name)) editor.patch-macros))))

(def lib-section ()
  (section "Library"
    (map (lambda (m) (macro-row m "library-macro" (lib-icon m) false (list)))
         (filter (lambda (m) (match? m.name)) editor.library-macros))))

(def asset-row (a)
  (dict :label a.reference
        :name a.reference
        :kind "patcher-asset"
        :detail a.tier
        :tier a.tier
        :file a.reference
        :source-path a.source-path
        :drag-type "dgen-asset"
        :draggable true
        :drop-target false))

(def asset-section ()
  (section "Assets"
    (map asset-row (filter (lambda (a) (match? a.reference)) editor.assets))))

(def patch-macros-items ()
  (append
    (if (= macro-sidebar.filter "") (nested-patch-section) (flat-patch-section))
    (lib-section)
    (asset-section)))

(def activate (item)
  (if (or (= (get item :kind) "patch-macro")
          (= (get item :kind) "library-macro"))
    (host-command "open-editor-macro-view" (dict :name (get item :name)))
    nil))

;; Single-click path: only "In Patch" rows navigate. Library rows stay inert
;; so a click-and-drag out of the sidebar never yanks the view away.
(def click (item)
  (if (= (get item :click-opens) true)
    (activate item)
    nil))

;; Widget :key props auto-qualify against this module (hazard a), so the
;; hand-rolled "patch-macros-" prefix is redundant and dropped; no Rust
;; assertion looks these keys up.
(def search-row ()
  (box :width :fill :padding 0.25
    (text-input
      :key "search"
      :width :fill
      :value macro-sidebar.filter
      :placeholder "Search macros and assets..."
      :on-change (lambda (v) (set! macro-sidebar.filter v))
      :height 1.5
      :font-size 11)))

(def empty-message ()
  (box :width :fill :padding 0.5 :align :center
    (label "No macros yet"
      :font-size 9.5
      :color :gray
      :bg :transparent)))

;; ── Asset inspector ─────────────────────────────────────────────────────
;; When exactly one file-backed tensor node is selected in the patcher, the
;; host shows its reference + asset metadata as editor.selected-asset (nil
;; otherwise); a compact panel below the tree shows the high-level shape so
;; the index math (set * waves-per-set + wave) is patchable at a glance. An
;; undeclared label reads empty (a count 0).

(def join-names (names)
  (reduce |acc n| (if (= acc "") n (str acc ", " n)) "" names))

(def dims-text (shape)
  (reduce |acc d| (if (= acc "") (str d) (str acc " x " d)) "" shape))

(def inspector-row (text color size)
  (label text :font-size size :color color :bg :transparent :width :fill))

(def asset-inspector (a)
  (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0.45
    (v-stack :width :fill :gap 0.18
      (inspector-row a.reference :white 9.5)
      (unless (empty? a.shape)
        (inspector-row
          (if (= a.tensor-kind "")
            (dims-text a.shape)
            (str a.tensor-kind "  " (dims-text a.shape)))
          :gray 9))
      (unless (= a.waves-per-set 0)
        (inspector-row
          (str a.set-count " sets x " a.waves-per-set " waves/set")
          :gray 9))
      (unless (empty? a.sets)
        (inspector-row (join-names a.sets) :dim 8.5))
      ;; Per-wave names matter most when there is no set structure to
      ;; summarize (single-set assets like basic-shapes).
      (when (and (not (empty? a.wave-names)) (empty? a.sets))
        (inspector-row (join-names a.wave-names) :dim 8.5))
      (unless (= a.source "")
        (inspector-row a.source :dim 8)))))

;; The buffer root must stay keyless: a keyed root is annotated as an
;; explicit subtree root and EmitTree then routes the update as a subtree
;; replacement, which is dropped when the buffer has no tree yet.
;; Widget-only buffer: take the shared sequencer keymap (was an implicit host default).
(set-buffer-mode-for "*patch-macros*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*patch-macros*"
  (let ((items (patch-macros-items))
        (selected-asset editor.selected-asset))
    (v-stack :width :fill :gap 0.4 :flex 1
      (search-row)
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (empty? items)
          (empty-message)
          (scroll :key "scroll" :width :fill :flex 1
            (tree
              :key "tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :expand-all true
              :focusable true
              :drag-type "dgen-macro"
              :selected-label (if (= editor.open-macro "") nil editor.open-macro)
              :selection-follows-external true
              :activate-parents true
              ;; Single click opens the macro view for "In Patch" rows: leaf
              ;; rows dispatch `select`, parent rows dispatch `toggle` (they
              ;; also expand or collapse). `activate` stays wired for
              ;; double-click / Enter, which works on Library rows too.
              :on-select (lambda (item) (click item))
              :on-toggle (lambda (item) (click item))
              :on-activate (lambda (item) (activate item))
              :on-modified-activate (lambda (item) (activate item))))))
      (when selected-asset
        (asset-inspector selected-asset)))))
