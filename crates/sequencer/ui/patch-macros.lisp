;; ui/patch-macros.lisp — Macro sidebar for the patch editor.
;; Renders to *patch-macros* buffer: defmacros defined in the current patch
;; ("In Patch", nested by call structure) plus the saved defmacro library
;; ("Library"; macros imported by the patch get the :sliders icon). Rows drag
;; into the patch editor as "dgen-macro" items; dropping one creates a node
;; calling that macro at the drop point. The blue selected row always mirrors
;; the macro view open in the patcher (SEQ.editor-open-macro); single-click an
;; "In Patch" row to open that macro's view. Library rows only open on
;; double-click, so click-dragging one into the patch does not navigate.
;;
;; MODULE NOTE (spec §10, S3b): this file is a RENDER ROOT — it registers the
;; *patch-macros* effect-buffer at top level. `import` EVALUATES its target, so
;; NEVER import this module from a library file; that would drag a UI root into
;; every VM that loads the importer. Reach `patch-macros-items` bare through
;; the identity compat alias below instead.
;;
;; It needs no imports of its own: everything it touches outside the file is a
;; Rust native, a widget, or the SEQ reactive namespace (SEQ.editor-patch-macros
;; / SEQ.editor-library-macros / SEQ.editor-open-macro), none of which are
;; module-scoped. Only `patch-macros-items` and `patch-macros-filter` are
;; reachable from outside (Rust tests eval them by flat name); every other def
;; is `%`-private with the now-redundant file prefix stripped.
(module eseq.patch-macros)

;; The Rust sidebar tests call `(patch-macros-items)` by its flat spelling from
;; a headerless eval; it is a function, so an identity alias covers it safely.
(module-compat-alias patch-macros-items patch-macros-items)

;; The search box is a named state definition — `(def x (state …))` compiles
;; through the same path as `defstate`, so it lives in the `state_bindings`
;; keyspace, not the mutable-plain-def hazard (m) trap, and needs no
;; eseq.vanilla pin. A Rust test drives it with a headerless
;; `(set! patch-macros-filter …)`; that flat write follows this alias into the
;; qualified state binding and emits StoreState on the very same node.
(module-compat-alias patch-macros-filter patch-macros-filter)

(def patch-macros-filter (state ""))

(def %match? (m)
  (or (= patch-macros-filter "")
      (str-contains? (get m :name) patch-macros-filter)))

(def %find-macro (name ms)
  (nth (filter (lambda (m) (= (get m :name) name)) ms) 0))

(def %lib-icon (m)
  (if (get m :used) :sliders :dial))

;; :click-opens marks rows that jump to their macro view on a single click.
;; Only rows under "In Patch" set it — "Library" rows are primarily drag
;; sources, and opening a view mid-drag-start feels like a misfire.
(def %lib-leaf (m click-opens)
  (dict :label (get m :name)
        :name (get m :name)
        :kind "library-macro"
        :icon (%lib-icon m)
        :click-opens click-opens
        :drop-target false))

;; Library macros can import other library macros; nest those too. Only
;; reachable from the "In Patch" call tree, so these rows open on click.
(def %lib-item (m depth)
  (let ((kids (%child-items (get m :calls) depth)))
    (if (= (len kids) 0)
      (%lib-leaf m true)
      (dict :label (get m :name)
            :name (get m :name)
            :kind "library-macro"
            :icon (%lib-icon m)
            :click-opens true
            :drop-target false
            :children kids))))

;; ── Nested "In Patch" items: children = macros this macro's body calls. ──
;; Depth-capped so a (malformed) cyclic call graph cannot recurse forever.

(def %call-item (c depth)
  (let ((local (%find-macro c SEQ.editor-patch-macros)))
    (if (not (= local nil))
      (%local-item local depth)
      (let ((lib (%find-macro c SEQ.editor-library-macros)))
        (if (not (= lib nil))
          (%lib-item lib depth)
          nil)))))

(def %child-items (calls depth)
  (if (> depth 4)
    (list)
    (filter (lambda (x) (not (= x nil)))
      (map (lambda (c) (%call-item c (+ depth 1))) calls))))

(def %local-item (m depth)
  (let ((kids (%child-items (get m :calls) depth)))
    (if (= (len kids) 0)
      (dict :label (get m :name)
            :name (get m :name)
            :kind "patch-macro"
            :icon :dial
            :click-opens true
            :drop-target false)
      (dict :label (get m :name)
            :name (get m :name)
            :kind "patch-macro"
            :icon :dial
            :click-opens true
            :drop-target false
            :children kids))))

;; Macros not called by another local macro are roots; called ones appear
;; nested under each caller.
(def %called-by-some? (name)
  (< 0 (len (filter
              (lambda (m) (< 0 (len (filter (lambda (c) (= c name)) (get m :calls)))))
              SEQ.editor-patch-macros))))

(def %header-row (label)
  (dict :label label :kind "header" :draggable false :drop-target false))

(def %nested-patch-section ()
  (let ((roots (filter (lambda (m) (not (%called-by-some? (get m :name))))
                       SEQ.editor-patch-macros)))
    (if (= (len roots) 0)
      (list)
      (append
        (list (%header-row "In Patch"))
        (map (lambda (m) (%local-item m 0)) roots)))))

(def %lib-section ()
  (let ((visible (filter (lambda (m) (%match? m)) SEQ.editor-library-macros)))
    (if (= (len visible) 0)
      (list)
      (append
        (list (%header-row "Library"))
        (map (lambda (m) (%lib-leaf m false)) visible)))))

;; Search active: flatten both sections to matching rows.
(def %flat-patch-section ()
  (let ((visible (filter (lambda (m) (%match? m)) SEQ.editor-patch-macros)))
    (if (= (len visible) 0)
      (list)
      (append
        (list (%header-row "In Patch"))
        (map (lambda (m)
               (dict :label (get m :name)
                     :name (get m :name)
                     :kind "patch-macro"
                     :icon :dial
                     :click-opens true
                     :drop-target false))
             visible)))))

(def patch-macros-items ()
  (if (= patch-macros-filter "")
    (append (%nested-patch-section) (%lib-section))
    (append (%flat-patch-section) (%lib-section))))

(def %activate (item)
  (if (= (get item :kind) "header")
    nil
    (host-command "open-editor-macro-view" (dict :name (get item :name)))))

;; Single-click path: only "In Patch" rows navigate. Library rows stay inert
;; so a click-and-drag out of the sidebar never yanks the view away.
(def %click (item)
  (if (= (get item :click-opens) true)
    (%activate item)
    nil))

;; Widget :key props auto-qualify against this module (hazard a), so the
;; hand-rolled "patch-macros-" prefix is redundant and dropped; no Rust
;; assertion looks these keys up.
(def %search-row ()
  (box :width :fill :padding 0.25
    (text-input
      :key "search"
      :width :fill
      :value patch-macros-filter
      :placeholder "Search macros..."
      :on-change (lambda (v) (set! patch-macros-filter v))
      :height 1.5
      :font-size 11)))

(def %empty-message ()
  (box :width :fill :padding 0.5 :align :center
    (label "No macros yet"
      :font-size 9.5
      :color :gray
      :bg :transparent)))

;; The buffer root must stay keyless: a keyed root is annotated as an
;; explicit subtree root and EmitTree then routes the update as a subtree
;; replacement, which is dropped when the buffer has no tree yet.
(effect-buffer "*patch-macros*"
  (let ((items (patch-macros-items)))
    (v-stack :width :fill :gap 0.4 :flex 1
      (%search-row)
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (%empty-message)
          (scroll :key "scroll" :width :fill :flex 1
            (tree
              :key "tree"
              :width :fill
              :background-color :buffer-bg
              :items items
              :expand-all true
              :focusable true
              :drag-type "dgen-macro"
              :selected-label (if (= SEQ.editor-open-macro "") nil SEQ.editor-open-macro)
              :selection-follows-external true
              :activate-parents true
              ;; Single click opens the macro view for "In Patch" rows: leaf
              ;; rows dispatch `select`, parent rows dispatch `toggle` (they
              ;; also expand or collapse). `activate` stays wired for
              ;; double-click / Enter, which works on Library rows too.
              :on-select (lambda (item) (%click item))
              :on-toggle (lambda (item) (%click item))
              :on-activate (lambda (item) (%activate item))
              :on-modified-activate (lambda (item) (%activate item)))))))))
