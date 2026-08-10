;; ui/patch-macros.lisp — Macro sidebar for the patch editor.
;; Renders to *patch-macros* buffer: defmacros defined in the current patch
;; ("In Patch", nested by call structure) plus the saved defmacro library
;; ("Library"; macros imported by the patch get the :sliders icon). Rows drag
;; into the patch editor as "dgen-macro" items; dropping one creates a node
;; calling that macro at the drop point. The blue selected row always mirrors
;; the macro view open in the patcher (SEQ.editor-open-macro); single-click an
;; "In Patch" row to open that macro's view. Library rows only open on
;; double-click, so click-dragging one into the patch does not navigate.

(def patch-macros-filter (state ""))

(def patch-macros-match? (m)
  (or (= patch-macros-filter "")
      (str-contains? (get m :name) patch-macros-filter)))

(def patch-macros-find (name ms)
  (nth (filter (lambda (m) (= (get m :name) name)) ms) 0))

(def patch-macros-lib-icon (m)
  (if (get m :used) :sliders :dial))

;; :click-opens marks rows that jump to their macro view on a single click.
;; Only rows under "In Patch" set it — "Library" rows are primarily drag
;; sources, and opening a view mid-drag-start feels like a misfire.
(def patch-macros-lib-leaf (m click-opens)
  (dict :label (get m :name)
        :name (get m :name)
        :kind "library-macro"
        :icon (patch-macros-lib-icon m)
        :click-opens click-opens
        :drop-target false))

;; Library macros can import other library macros; nest those too. Only
;; reachable from the "In Patch" call tree, so these rows open on click.
(def patch-macros-lib-item (m depth)
  (let ((kids (patch-macros-child-items (get m :calls) depth)))
    (if (= (len kids) 0)
      (patch-macros-lib-leaf m true)
      (dict :label (get m :name)
            :name (get m :name)
            :kind "library-macro"
            :icon (patch-macros-lib-icon m)
            :click-opens true
            :drop-target false
            :children kids))))

;; ── Nested "In Patch" items: children = macros this macro's body calls. ──
;; Depth-capped so a (malformed) cyclic call graph cannot recurse forever.

(def patch-macros-call-item (c depth)
  (let ((local (patch-macros-find c SEQ.editor-patch-macros)))
    (if (not (= local nil))
      (patch-macros-local-item local depth)
      (let ((lib (patch-macros-find c SEQ.editor-library-macros)))
        (if (not (= lib nil))
          (patch-macros-lib-item lib depth)
          nil)))))

(def patch-macros-child-items (calls depth)
  (if (> depth 4)
    (list)
    (filter (lambda (x) (not (= x nil)))
      (map (lambda (c) (patch-macros-call-item c (+ depth 1))) calls))))

(def patch-macros-local-item (m depth)
  (let ((kids (patch-macros-child-items (get m :calls) depth)))
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
(def patch-macros-called-by-some? (name)
  (< 0 (len (filter
              (lambda (m) (< 0 (len (filter (lambda (c) (= c name)) (get m :calls)))))
              SEQ.editor-patch-macros))))

(def patch-macros-header (label)
  (dict :label label :kind "header" :draggable false :drop-target false))

(def patch-macros-nested-patch-section ()
  (let ((roots (filter (lambda (m) (not (patch-macros-called-by-some? (get m :name))))
                       SEQ.editor-patch-macros)))
    (if (= (len roots) 0)
      (list)
      (append
        (list (patch-macros-header "In Patch"))
        (map (lambda (m) (patch-macros-local-item m 0)) roots)))))

(def patch-macros-lib-section ()
  (let ((visible (filter (lambda (m) (patch-macros-match? m)) SEQ.editor-library-macros)))
    (if (= (len visible) 0)
      (list)
      (append
        (list (patch-macros-header "Library"))
        (map (lambda (m) (patch-macros-lib-leaf m false)) visible)))))

;; Search active: flatten both sections to matching rows.
(def patch-macros-flat-patch-section ()
  (let ((visible (filter (lambda (m) (patch-macros-match? m)) SEQ.editor-patch-macros)))
    (if (= (len visible) 0)
      (list)
      (append
        (list (patch-macros-header "In Patch"))
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
    (append (patch-macros-nested-patch-section) (patch-macros-lib-section))
    (append (patch-macros-flat-patch-section) (patch-macros-lib-section))))

(def patch-macros-activate (item)
  (if (= (get item :kind) "header")
    nil
    (host-command "open-editor-macro-view" (dict :name (get item :name)))))

;; Single-click path: only "In Patch" rows navigate. Library rows stay inert
;; so a click-and-drag out of the sidebar never yanks the view away.
(def patch-macros-click (item)
  (if (= (get item :click-opens) true)
    (patch-macros-activate item)
    nil))

(def patch-macros-search-row ()
  (box :width :fill :padding 0.25
    (text-input
      :key "patch-macros-search"
      :width :fill
      :value patch-macros-filter
      :placeholder "Search macros..."
      :on-change (lambda (v) (set! patch-macros-filter v))
      :height 1.5
      :font-size 11)))

(def patch-macros-empty-message ()
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
      (patch-macros-search-row)
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (if (= (len items) 0)
          (patch-macros-empty-message)
          (scroll :key "patch-macros-scroll" :width :fill :flex 1
            (tree
              :key "patch-macros-tree"
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
              :on-select (lambda (item) (patch-macros-click item))
              :on-toggle (lambda (item) (patch-macros-click item))
              :on-activate (lambda (item) (patch-macros-activate item))
              :on-modified-activate (lambda (item) (patch-macros-activate item)))))))))
