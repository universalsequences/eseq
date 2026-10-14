;; ui/patch-inspector.lisp — The patch editor's inspector (*patch-inspector*).
;;
;; Shows the selected patcher boxes (docs/patcher-workspace-spec.md §6): the
;; operator's documentation with a line per inlet, and each attribute it
;; declares (plus any the box carries) as an editable field. The box text
;; itself is edited on the canvas. One box: its values. Several: the attributes of their
;; shared operator, a value shown when they agree, and a write sets every
;; box, one undo step. Edits rewrite box text through the patcher
;; (`patcher-set-node-texts`), so the inspector holds nothing the boxes do
;; not; a field commits on Enter.
;;
;; The selection is the host's `editor` kind (`selected-node-ids`,
;; `selected-node-texts`), mirrored from the patcher's render pass.
;;
;; MODULE NOTE (spec §10, S3b): this file is a RENDER ROOT — it registers the
;; *patch-inspector* effect-buffer at top level. NEVER import it from a
;; library file.
(module eseq.patch-inspector)
(import eseq.kinds :refer (editor))

(export patch-inspector
        set-attribute!)

;; The field being typed in (an attribute key) and its
;; draft, until Enter commits it. A selection change drops it.
(def-kind patch-inspector
  :key ()
  :state ((field "")
          (draft "")
          (selection "")))

(def selection-key ()
  (str editor.selected-node-ids))

(def sync-selection! ()
  (if (= patch-inspector.selection (selection-key))
    nil
    (do
      (set! patch-inspector.selection (selection-key))
      (set! patch-inspector.field "")
      (set! patch-inspector.draft ""))))

(def selected-count () (len editor.selected-node-ids))

(def all? (pred xs)
  (= (len (filter (lambda (x) (not (pred x))) xs)) 0))

(def parsed (i) (patcher-node-parse (nth editor.selected-node-texts i)))

;; Every selected box is a number literal.
(def constants? ()
  (all? (lambda (i) (let ((p (parsed i))) (and p (get p :constant))))
        (range 0 (selected-count))))

(def constant-badge (text)
  (box :padding 0.25 :corner-radius 6 :background-color :mixer-control-bg
    (label text :font-size 11 :color :fg :bg :transparent)))

;; The patch's own or library defmacro named `op`, or nil.
(def macro-of (op)
  (first (filter (lambda (m) (= m.name op))
                 (append editor.patch-macros editor.library-macros))))

(def tag (text)
  (box :padding 0.2 :corner-radius 6 :background-color :mixer-control-bg
    (label text :font-size 9.5 :color :dim :bg :transparent)))

(def op-of (i)
  (let ((p (parsed i)))
    (if p (get p :op) "")))

;; The one operator every selected box shares, or "".
(def shared-op ()
  (let ((op (op-of 0)))
    (if (all? (lambda (i) (= (op-of i) op)) (range 0 (selected-count))) op "")))

(def attr-value (i key)
  (let ((p (parsed i)))
    (if p
      (let ((hit (filter (lambda (kv) (= (nth kv 0) key)) (get p :attrs))))
        (if (> (len hit) 0) (nth (nth hit 0) 1) ""))
      "")))

;; The selected boxes' shared value for `key`, or nil when they differ.
(def shared-value (key)
  (let ((v (attr-value 0 key)))
    (if (all? (lambda (i) (= (attr-value i key) v)) (range 0 (selected-count))) v nil)))

;; The attribute keys to show: the operator's declared ones, then any other
;; the first box carries.
(def attribute-keys (op)
  (let ((declared (patcher-operator-attributes op))
        (p (parsed 0))
        (carried (if p (map (lambda (kv) (nth kv 0)) (get p :attrs)) '())))
    (reduce (lambda (keys k) (if (all? (lambda (x) (not (= x k))) keys) (append keys (list k)) keys))
      declared carried)))

;; ── Writes ──

(def set-attribute! (key value)
  :doc "Set attribute `key` (such as @min) of every selected box to `value`; empty removes it."
  (do
    (set! patch-inspector.field "")
    (patcher-set-node-texts
      (map (lambda (i)
             (list (nth editor.selected-node-ids i)
                   (patcher-node-set-attr (nth editor.selected-node-texts i) key value)))
           (range 0 (selected-count))))))

;; ── Pieces ──

(def field-value (field current)
  (if (= patch-inspector.field field) patch-inspector.draft current))

(def field-input (field current placeholder commit)
  (text-input
    :key (str "patch-inspector-" field)
    :width :fill
    :value (field-value field current)
    :placeholder placeholder
    :on-change (lambda (v)
                 (do (set! patch-inspector.field field)
                     (set! patch-inspector.draft v)))
    :on-submit (lambda () (commit (field-value field current)))
    :height 1.4
    :font-size 11))

(def row (label-text input)
  (h-stack :width :fill :gap 0.6 :align :center
    ;; Wide enough for the manifest's longest name (@mod-resolved-symbol).
    (box :width 16
      (label label-text :font-size 10.5 :color :dim :bg :transparent))
    ;; Width from the row, not the value: a long box text would otherwise
    ;; widen every row past the tile.
    (box :flex 1 :width 1 input)))

(def attribute-row (key)
  (let ((v (shared-value key)))
    (row key
      (field-input key (if (= v nil) "" v) (if (= v nil) "mixed" "")
        (lambda (value) (set-attribute! key value))))))

(def section (title)
  (label title :font-size 10 :color :dim :bg :transparent))

;; An inlet or outlet: its index, name and kind, then what it does.
(def port-row (i port inlet)
  (v-stack :width :fill :gap 0.1
    (h-stack :width :fill :gap 0.5 :align :center
      (label (str (+ i 1)) :font-size 10.5 :color :dim :bg :transparent)
      (label (if (= (get port :name) "") "—" (get port :name))
        :font-size 11 :color :fg :bg :transparent)
      (if (= (get port :kind) "")
        (box :width 0 :height 0)
        (label (get port :kind) :font-size 9.5 :color :dim :bg :transparent))
      (if (or (not inlet) (get port :required))
        (box :width 0 :height 0)
        (label "optional" :font-size 9.5 :color :dim :bg :transparent)))
    (if (= (get port :summary) "")
      (box :width 0 :height 0)
      (label (get port :summary) :font-size 10 :color :dim :bg :transparent :wrap true :width :fill))))

(def ports-section (title ports inlet)
  (if (= (len ports) 0)
    (box :width 0 :height 0)
    (v-stack :width :fill :gap 0.35
      (section title)
      (each (range 0 (len ports)) |i| (port-row i (nth ports i) inlet)))))

;; The operator's manifest docs; a macro or unknown operator has none.
(def docs-section (op)
  (let ((docs (if (= op "") nil (patcher-operator-doc op))))
    (if docs
      (v-stack :width :fill :gap 0.6
        (if (= (get docs :summary) "")
          (box :width 0 :height 0)
          (label (get docs :summary) :font-size 10.5 :color :fg :bg :transparent :wrap true :width :fill))
        ;; A tensor shape (int[]) is typed in the box, not cabled.
        (ports-section "Arguments" (filter (lambda (p) (= (get p :kind) "int[]")) (get docs :inputs)) true)
        (ports-section "Inlets" (filter (lambda (p) (not (= (get p :kind) "int[]"))) (get docs :inputs)) true)
        (ports-section "Outlets" (get docs :outputs) false))
      (box :width 0 :height 0))))

(def hint (text)
  (box :width :fill :height :fill :padding 1 :h-align :center :v-align :center
    (label text :font-size 11 :color :dim :bg :transparent)))

;; A macro box: its name tagged macro, its summary and inlets (its params),
;; and Expand, which opens its view (the toolbar's back arrow returns).
(def macro-body (op m)
  (v-stack :width :fill :gap 0.6
    (h-stack :width :fill :gap 0.5 :align :center
      (label op :font-size 12 :color :fg :bg :transparent)
      (tag (if m.library "library macro" "macro"))
      (box :height 0 :flex 1)
      (button "Expand" :variant :secondary :height 1.4 :font-size 10.5
        :on-click |x y r| (host-command "open-editor-macro-view" (dict :name op))
        :color :fg))
    (if (= m.summary "")
      (box :width 0 :height 0)
      (label m.summary :font-size 10.5 :color :fg :bg :transparent :wrap true :width :fill))
    (ports-section "Inlets"
      (map (lambda (p) (dict :name p :kind "" :required true :summary "")) m.params) true)
    (ports-section "Outlets"
      (map (lambda (p) (dict :name p :kind "" :required true :summary "")) m.outputs) false)))

;; A number box: "constant" with its value (one box) or a count (several).
(def constant-body ()
  (h-stack :width :fill :gap 0.5 :align :center
    (label "constant" :font-size 12 :color :fg :bg :transparent)
    (if (= (selected-count) 1)
      (constant-badge (get (parsed 0) :op))
      (label (str (selected-count) " boxes") :font-size 10 :color :dim :bg :transparent))))

;; A compiled shape, `kind|dims|lanes` from the host: the kind as text, the
;; dims as a badge (`[8 1]`, or `scalar`), the lane count dimmed.
(def shape-view (encoded)
  (let ((parts (string-split encoded "|"))
        (part (lambda (i) (if (> (len parts) i) (nth parts i) ""))))
    (h-stack :gap 0.5 :align :center
      (label (part 0) :font-size 11.5 :color :fg :bg :transparent)
      (if (= (part 1) "")
        (box :width 0 :height 0)
        ;; Tensor dims take the tensor-cable colour, so the badge reads as
        ;; the cable it describes; `scalar` stays neutral.
        (let ((tensor (= (substring (part 1) 0 1) "[")))
          (box :padding 0.25 :corner-radius 12
            :background-color (if tensor :patcher-tensor-cable :mixer-control-bg)
            (label (part 1) :v-align :center :font-size 11 :color (if tensor :buffer-bg :fg) :bg :transparent))))
      (if (= (part 2) "")
        (box :width 0 :height 0)
        (label (part 2) :font-size 10.5 :color :dim :bg :transparent)))))

;; A selected cable: what flows through it (compiled kind, shape, lanes)
;; and where it runs (docs/patcher-tensor-cables-spec.md §6).
(def cable-selected? ()
  (> (len editor.selected-cable) 0))

(def cable-body ()
  (v-stack :width :fill :gap 0.6
    (label "Cable" :font-size 12 :color :fg :bg :transparent)
    (shape-view (nth editor.selected-cable 0))
    (v-stack :width :fill :gap 0.2
      (row "from" (label (nth editor.selected-cable 1) :font-size 11 :color :dim :bg :transparent))
      (row "to" (label (nth editor.selected-cable 2) :font-size 11 :color :dim :bg :transparent)))))

;; A node's compiled output: `Output  signal-tensor [8] · 8 lanes`.
(def output-row ()
  (if (= (selected-count) 1)
    (row "output"
      (shape-view (if (> (len editor.selected-node-shapes) 0) (nth editor.selected-node-shapes 0) "shape unknown||")))
    (box :width 0 :height 0)))

(def inspector-body ()
  (if (= (selected-count) 0)
    (if (cable-selected?) (cable-body) (hint "No nodes selected"))
    (if (constants?)
      (constant-body)
    (if (and (= (selected-count) 1) (macro-of (shared-op)))
      (macro-body (shared-op) (macro-of (shared-op)))
    (let ((op (shared-op)))
      (v-stack :width :fill :gap 0.35
        (h-stack :width :fill :gap 0.5 :align :center
          (label (if (= op "") "mixed selection" op) :font-size 12 :color :fg :bg :transparent)
          (if (> (selected-count) 1)
            (label (str (selected-count) " boxes") :font-size 10 :color :dim :bg :transparent)
            (box :width 0 :height 0)))
        (output-row)
        (docs-section op)
        (if (or (= op "") (= (len (attribute-keys op)) 0))
          (box :width 0 :height 0)
          (v-stack :width :fill :gap 0.35
            (section "Attributes")
            (each (attribute-keys op) |key| (attribute-row key))))))))))

(set-buffer-mode-for "*patch-inspector*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*patch-inspector*"
  (do
    (sync-selection!)
    (box :width :fill :height :fill :padding 0.6 :background-color :buffer-bg
      ;; The empty hint centers in the tile; a scroll has no height to center in.
      (if (and (= (selected-count) 0) (not (cable-selected?)))
        (hint "No nodes selected")
      (scroll :key "patch-inspector-scroll" :width :fill :flex 1
        ;; Clear of the tile's rounded edge, and of the scrollbar on the right.
        (h-stack :width :fill :gap 0 :padding-top 0.6 :padding-bottom 0.6
          (box :width 0.8 :height 1)
          (box :flex 1 :width 1 (inspector-body))
          (box :width 1.6 :height 1)))))))
