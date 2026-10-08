;; ui/processes-buffer.lisp — the *processes* dock: an inspector for the
;; process card selected in the open node editor, over that card's expr code
;; (docs/expr-process-spec.md §7; beads eseq-waa9.16, eseq-waa9.22).
;;
;; While a graph node's expanded editor is open in the main tile (a kind
;; instance's step tab whose `expanded-node` is a node), the dock takes the
;; right column: the inspector in place of *step*, the code in place of
;; *track* (eseq.seq-layout/step-and-track-panel-layout-spec). The browser
;; sidebar is never touched. Which buffer the main tile shows is
;; eseq.seq-step-tabs/step-panel-buffer, kept current by the tab strip's
;; :on-select, so the Seq tab gets *step*/*track* back and the instance tab
;; the dock, with its node, selection and code state (all kept per instance
;; or in defstates).
;;
;;   header     "<instance> · node N", and show code / hide code on an expr card
;;   inspector  the selected card: label, on/off, < > x, inlet pickers, out
;;              mapping, promote… / as expr. The node's kind draws it (see
;;              register-node-inspector); nothing selected shows a hint.
;;   code       an expr card's body in a real text-mode buffer — the same
;;              `*expr node N · slot M*` buffer eseq.expr-buffer commits — in
;;              its own tile under the inspector, splitting the dock by
;;              eseq.seq-layout/processes-code-ratio (0.5). Completion,
;;              C-c C-c and save work there unchanged.
;;
;; Adding processes is the bay's add-process dropdown; the dock has no
;; library of its own (it duplicated the dropdown, eseq-waa9.22).
;;
;; Inspector seam: this module is core UI and cannot import a package, so a
;; kind that hosts node editors registers its card renderer here, keyed by
;; its instance kind, from the event that expands a node (next to
;; eseq.sequencer/lane-patch-register-node):
;;
;;   (eseq.processes-buffer/register-node-inspector self.kind
;;     (lambda (graph node slot-id) (my-card-renderer graph node slot-id)))
;;
;; Register a lambda that calls the renderer by name, not the function
;; value: re-evaluating the kind's file then restyles the dock at once.
;;
;; The kind renders the same function inline in its bay when the dock is not
;; showing its node (`docks?` false: setting off, another layout), so the
;; two never drift.
(module eseq.processes-buffer)

(import eseq.kinds :refer (graph-of remove-process!))
(import eseq.seq-core-state)
(import eseq.seq-step-tabs)
(import eseq.expr-buffer)
;; *step*'s header pill + chip, shared so the two headers stay identical.
(import eseq.panel-header :as header)

(export processes-buffer-auto-split
        dock-target
        showing?
        docks?
        register-node-inspector
        inspected-slot-id
        code-visible
        set-code-visible
        toggle-code
        code-slot-id
        code-layout-buffer
        open-docked
        panel)

(defcustom processes-buffer-auto-split true
  :type :bool
  :doc "Show the *processes* inspector (and a selected expr card's code) in place of *step*/*track* while a node editor is open.")

;; ── Which node ──────────────────────────────────────────────────────────
;; The instance whose tab the main tile shows, or nil.
(def visible-instance ()
  (let ((buffer (eseq.seq-step-tabs/seq-visible-main-panel-buffer))
        (tab (reduce |acc t|
               (if (and (= acc nil)
                        (eseq.seq-step-tabs/seq-instance-step-tab? t)
                        (= (eseq.seq-step-tabs/seq-step-tab-buffer t) buffer))
                 t acc)
               nil eseq.seq-step-tabs/seq-registered-step-tabs)))
    (if tab (instance-ref (eseq.seq-step-tabs/seq-step-tab-instance-id tab)) nil)))

;; (graph node) of the node editor open in the main tile, or nil. A kind
;; that hosts node editors registers a node bay (lane-patch-register-node)
;; and keeps the open node in its `expanded-node` view field (-1 = none).
(def dock-target ()
  (let ((inst (visible-instance)))
    (if (and inst (eseq.sequencer/lane-patch-node-host? inst))
      (let ((n inst.expanded-node))
        (if (and (number? n) (>= n 0)) (list inst n) nil))
      nil)))

(def showing? ()
  (if (and processes-buffer-auto-split (not (= (dock-target) nil))) true false))

(def same-target? (graph node)
  (let ((target (dock-target)))
    (if (and target (= (nth target 0) graph) (= (nth target 1) node)) true false)))

;; Whether the dock is on screen for node `node` of `graph`: the kind then
;; leaves its inline inspector out of the bay, and an expr card's edit
;; button opens into the dock's code tile. The right column exists in the
;; regular (:lower-panel) layout only.
(def docks? (graph node)
  (if (and (showing?)
           (= eseq.seq-step-tabs/seq-layout-mode :lower-panel)
           (same-target? graph node))
    true false))

;; ── Inspector renderers ─────────────────────────────────────────────────
;; (list (list kind renderer) …), written from the expand event. A
;; `defstate`, not a plain `def`: re-evaluating this file (C-c C-c, eval
;; buffer, hot reload) keeps a defstate's value, where a `def` would reset
;; the registry to empty and blank the dock until the next node expand
;; (eseq-waa9.22). Being reactive, a registration also re-renders the panel.
;; A kind registers a trampoline that calls its renderer by name, so
;; re-evaluating the kind's file restyles the dock at once too.
(defstate node-inspectors (list))

(def register-node-inspector (kind renderer)
  (set! node-inspectors
    (append (list (list kind renderer))
            (filter (lambda (entry) (not (= (nth entry 0) kind))) node-inspectors))))

(def inspector-for (inst)
  (let ((kind inst.kind))
    (reduce |acc entry| (if (and (= acc nil) (= (nth entry 0) kind)) (nth entry 1) acc)
      nil node-inspectors)))

;; ── The inspected card ──────────────────────────────────────────────────
;; The process of node `node` of instance `graph` whose proc-id is
;; `slot-id`, or nil (gone, or not listed yet: the kinds list a process
;; added through a native at the host's next sync).
(def chain-slot (graph node slot-id)
  (let ((g (graph-of graph))
        (n (when g (nth g.nodes node))))
    (when n (eseq.sequencer/process-of n slot-id))))

;; The card selected in node `node`'s bay while it is still in the chain,
;; or nil.
(def inspected-slot-id (graph node)
  (let ((selected (eseq.sequencer/lane-patch-node-selected-id)))
    (if (and (number? selected) (>= selected 0) (chain-slot graph node selected))
      selected
      nil)))

;; ── The code tile ───────────────────────────────────────────────────────
;; Show code / hide code, remembered for the session. On by default: an
;; expr card is its code, so selecting one shows it.
(defstate code-visible true)

(def set-code-visible (on) (set! code-visible (if on true false)))
(def toggle-code () (set-code-visible (not code-visible)))

;; The expr card whose code the dock shows: the inspected card when it is
;; an expr card and code is shown, else nil.
(def code-slot-id ()
  (let ((target (if (showing?) (dock-target) nil)))
    (if (and target code-visible)
      (let ((id (inspected-slot-id (nth target 0) (nth target 1))))
        (let ((p (when id (chain-slot (nth target 0) (nth target 1) id))))
          (if (and p p.expr) id nil)))
      nil)))

;; For the layout spec: the code tile's buffer, created (without switching)
;; when it is not open, so the layout never names a missing buffer.
(def code-layout-buffer ()
  (let ((id (code-slot-id)))
    (if id
      (let ((target (dock-target)))
        (let ((p (chain-slot (nth target 0) (nth target 1) id)))
          (eseq.expr-buffer/ensure-node-slot-buffer (nth target 0) (nth target 1) id
            (if p p.index 0))))
      nil)))

;; Re-lay the right column when the dock appears, goes, or its code tile
;; changes: a node expanded or collapsed, the main tab switched (its
;; :on-select moves step-panel-buffer), a card selected, code shown or
;; hidden. `showing-seen` is a plain global so the write does not re-trigger
;; this observer; the first run (at load, before a layout exists) only
;; records.
(def showing-seen nil)

(def layout-key ()
  (if (showing?)
    (let ((target (dock-target)))
      (let ((inst (nth target 0)))
        (str "dock|" inst.id "|" (nth target 1) "|" (or (code-slot-id) ""))))
    "plain"))

(def relayout ()
  (eseq.seq-layout/refresh-current-layout))

(observe
  (let ((now (layout-key)))
    (do
      (if (and showing-seen
               (not (= showing-seen now))
               (= eseq.seq-step-tabs/seq-layout-mode :lower-panel))
        (relayout)
        nil)
      (set! showing-seen now))))

;; The edit button while docked (eseq.expr-buffer/open-node-slot; the bay
;; selected the card first): show the code tile and focus it. The layout is
;; applied here, before the focus, so both land in one editor drain.
(def open-docked (graph node slot-id slot-index)
  (let ((name (eseq.expr-buffer/ensure-node-slot-buffer graph node slot-id slot-index)))
    (do
      (set! code-visible true)
      (set! showing-seen (layout-key))
      (relayout)
      (select-window-for name)
      (status "expr: C-c C-c or save to commit")
      name)))

;; ── Panel ───────────────────────────────────────────────────────────────
;; *step*'s header: the instance in an accent chip, then "node N" and the
;; selected card's label as muted text.
(def node-caption (target slot-id)
  (let ((inst (nth target 0))
        (node (nth target 1)))
    (let ((name (or inst.label ""))
          (slot (if slot-id (chain-slot inst node slot-id) nil)))
      (header/pill
        (header/chip "processes-caption" name (max 4.55 (+ 1.2 (* 0.6 (len name))))
          :process-lane-accent false :process-lane-accent)
        (header/note "processes-caption-note"
          (str "node " node (if slot (str " · " slot.name) "")))))))

(def code-toggle (graph node slot-id)
  (let ((slot (if slot-id (chain-slot graph node slot-id) nil)))
    (if (and slot slot.expr)
      (button (if code-visible "hide code" "show code")
        :key "processes-code-toggle"
        :width 5.6 :height 1.1 :padding 0.05 :font-size 8
        :background-color (if code-visible :process-lane-accent :transparent)
        :border-color :process-lane-accent
        :color (if code-visible :black :process-lane-accent)
        :on-click (lambda (event) (toggle-code)))
      nil)))

(def hint (text)
  (label text :key "processes-hint" :width :fill :height 1.2 :font-size 9
    :color :dim :bg :transparent))

(def inspector (inst node slot-id)
  (let ((renderer (inspector-for inst)))
    (if (= slot-id nil)
      (hint "no process is selected yet.")
      (if renderer
        (renderer inst node slot-id)
        (hint "this sequencer has no process inspector")))))

(def panel ()
  (let ((target (if (showing?) (dock-target) nil)))
    (if (= target nil)
      (box :key "processes-empty" :width :fill :flex 1 :bg :transparent)
      (let ((inst (nth target 0))
            (node (nth target 1)))
        (let ((slot-id (inspected-slot-id inst node)))
          (v-stack :key "processes-buffer" :debug-name "processes-buffer"
            :width :fill :flex 1 :gap 0.2
            (h-stack :width :fill :gap 0.5 :align :center
              (node-caption target slot-id)
              (box :flex 1 :height 1.0 :bg :transparent)
              (code-toggle inst node slot-id))
            ;; Promote's name modal for the inspector's promote… (zero
            ;; footprint closed; a modal takes pointer input through its tile).
            (subtree :key "processes-promote-modal"
              (eseq.expr-buffer/promote-panel "dock"))
            (box :width :fill :flex 1 :padding 0 :bg :transparent
              (scroll :key "processes-inspector-scroll" :width :fill :height :fill
                (v-stack :key "processes-inspector" :width :fill :gap 0.3
                  (inspector inst node slot-id))))
            (delete-row inst node slot-id)))))))

;; The inspected card's delete, pinned to the inspector's bottom right.
(def delete-row (graph node slot-id)
  (let ((p (when slot-id (chain-slot graph node slot-id))))
    (if p
      (h-stack :key "processes-delete-row" :width :fill :align :center
        (box :flex 1 :height 0.5 :bg :transparent)
        (button "Delete"
          :key "processes-delete"
          :width 6.0 :height 1.1 :padding 0.05 :font-size 8
          :background-color :red :border-color :red :color :black
          :on-click (lambda (event) (remove-process! p))))
      nil)))

(def root-widget ()
  (v-stack :width :fill :height :fill :gap 0 :padding 1.0
    (panel)))

;; Widget-only buffer: take the shared sequencer keymap, like *samples*.
(set-buffer-mode-for "*processes*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*processes*"
  (root-widget))
