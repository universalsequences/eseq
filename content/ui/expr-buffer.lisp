;; ui/expr-buffer.lisp — the edit buffers of expr cards
;; (docs/expr-process-spec.md §2; bead eseq-waa9.11).
;;
;; An expr card's **edit** button opens an ordinary text buffer named for the
;; slot, `*expr node 3 · slot 2*`, preloaded with the card's stored body. The
;; buffer is plain eseqlisp text (highlighting, completion and paren matching
;; come from the editor); its mode only changes what saving means:
;;
;;   save-buffer, C-x C-s, C-c C-c, File > Save   commit the body
;;
;; A commit calls `graph-node-process-expr-set`. On success the buffer is
;; marked saved and a toast names any inlets that went away with their
;; cables. On a parse/compile error the card keeps its previous class, the
;; buffer stays modified, the error goes to the status line and a toast, and
;; the span the compiler reports is highlighted with the cursor on its line;
;; the card shows its error dot until the next good commit.
;;
;; Each open buffer remembers the card it edits by slot instance id, which
;; survives reordering; the name keeps the position it had when opened.
;; Reopening a card focuses its buffer. A commit after the card (or its
;; node or instance) was removed changes nothing and says so.
;;
;; Node bays only: track lane slots edit through the recorded history
;; commands, so the track twin is its own bead (eseq-waa9.18).
(module eseq.expr-buffer)

(export open-node-slot
        ensure-node-slot-buffer
        commit
        commit-source
        commit-error
        buffer-for-slot
        buffer-name-for
        expr-mode
        preset-list
        preset-labels
        preset-named
        add-preset
        add-node-preset
        open-promote
        close-promote
        promote-open?
        promote-name
        set-promote-name
        promote-name-error
        promote-update?
        commit-promote
        promote-node-slot
        edit-node-slot-as-expr
        promote-panel)

(def expr-mode "eseq.expr-buffer/expr-mode")

;; One entry per edit buffer this session opened, newest first:
;; (dict :name :graph :node :slot-id).
(defstate expr-buffers '())

;; Slots whose last commit failed, as (dict :graph :node :slot-id :error):
;; the card's error dot for a body that never reached the scheduler. A good
;; commit removes the slot's entry.
(defstate expr-commit-errors '())

(def same-slot? (entry graph node slot-id)
  (and (= (get entry :graph) graph)
       (= (get entry :node) node)
       (= (get entry :slot-id) slot-id)))

(def entry-for-slot (entries graph node slot-id)
  (reduce |acc entry| (if (and (= acc nil) (same-slot? entry graph node slot-id)) entry acc)
    nil entries))

(def entry-for-name (name)
  (reduce |acc entry| (if (and (= acc nil) (= (get entry :name) name)) entry acc)
    nil expr-buffers))

(def buffer-open? (name)
  (reduce |acc open| (or acc (= open name)) false (buffer-list)))

;; The spec's name for node `node`'s slot at 0-based `slot-index` (slots
;; count from 1 as the bay reads, the node as its editor labels it).
(def buffer-name-for (node slot-index)
  (str "*expr node " node " · slot " (+ slot-index 1) "*"))

;; A name no other buffer holds: two instances' node 3 · slot 2 get `<2>`.
(def fresh-name (base)
  (if (buffer-open? base)
    (reduce |acc k| (if (and (= acc nil) (not (buffer-open? (str base " <" k ">"))))
                      (str base " <" k ">")
                      acc)
      nil (range 2 100))
    base))

;; The open edit buffer of this slot, or nil.
(def buffer-for-slot (graph node slot-id)
  (let ((entry (entry-for-slot expr-buffers graph node slot-id)))
    (if (and entry (buffer-open? (get entry :name))) (get entry :name) nil)))

;; The last failed commit's message for this slot, or nil.
(def commit-error (graph node slot-id)
  (let ((entry (entry-for-slot expr-commit-errors graph node slot-id)))
    (if entry (get entry :error) nil)))

(def record-commit-error (graph node slot-id message)
  (set! expr-commit-errors
    (let ((others (filter (lambda (entry) (not (same-slot? entry graph node slot-id)))
                          expr-commit-errors)))
      (if (= message nil)
        others
        (append (list (dict :graph graph :node node :slot-id slot-id :error message)) others)))))

;; Open (or focus) the edit buffer of `slot-id` on node `node` of `graph`.
;; `slot-index` is the card's current 0-based position, used for the name.
;; While the *processes* dock is showing this node, the buffer opens in the
;; dock's code tile instead (docs/expr-process-spec.md §7).
(def open-node-slot (graph node slot-id slot-index)
  (if (eseq.processes-buffer/docks? graph node)
    (eseq.processes-buffer/open-docked graph node slot-id slot-index)
    (open-node-slot-here graph node slot-id slot-index)))

(def remember-slot-buffer (name graph node slot-id)
  (set! expr-buffers
    (append
      (list (dict :name name :graph graph :node node :slot-id slot-id))
      (filter (lambda (entry) (not (same-slot? entry graph node slot-id))) expr-buffers))))

;; The slot's edit buffer name, creating the buffer WITHOUT switching to it
;; when it is not open (the dock shows it in its own tile). Idempotent inside
;; one event: a buffer queued for creation is not in (buffer-list) until the
;; editor drains, so a known slot re-queues the same text under the same
;; name rather than minting a `<2>`.
(def ensure-node-slot-buffer (graph node slot-id slot-index)
  (let ((entry (entry-for-slot expr-buffers graph node slot-id)))
    (if (and entry (buffer-open? (get entry :name)))
      (get entry :name)
      (let ((name (if entry (get entry :name) (fresh-name (buffer-name-for node slot-index))))
            (source (graph-node-process-expr-source graph node slot-id)))
        (do
          (if entry nil (remember-slot-buffer name graph node slot-id))
          (set-buffer-text-for name (if source source ""))
          (set-buffer-mode-for name expr-mode)
          (mode-add-completions expr-mode (expr-context-completions))
          name)))))

(def open-node-slot-here (graph node slot-id slot-index)
  (let ((existing (buffer-for-slot graph node slot-id)))
    (if existing
      (do (switch-to-buffer existing) existing)
      (let ((name (fresh-name (buffer-name-for node slot-index)))
            (source (graph-node-process-expr-source graph node slot-id)))
        (do
          (remember-slot-buffer name graph node slot-id)
          (create-buffer name)
          (set-buffer-mode expr-mode)
          ;; Completion offers the names only a card body knows: the $
          ;; context variables and the direct writes (spec §4, §6), with
          ;; their one-line docs. Registered here, not at load, because the
          ;; host native arrives with the graph natives.
          (mode-add-completions expr-mode (expr-context-completions))
          (set-buffer-text (if source source ""))
          (status "expr: C-c C-c or save to commit")
          name)))))

(def join-names (names)
  (reduce |acc name| (if (= acc "") name (str acc ", " name)) "" names))

;; Commit `source` to the expr card `slot-id` with the edit buffer's
;; bookkeeping (error dot, toasts, status), buffer or not. Returns the
;; native's result map, or nil when the card is gone.
(def commit-source (graph node slot-id source)
  (if (not (graph-node-process-slot? graph node slot-id))
    (do
      (status "expr: the card this buffer edits is gone; nothing committed")
      (toast "expr card is gone: nothing committed" :kind :error)
      nil)
    (let ((result (graph-node-process-expr-set graph node slot-id source)))
      (do
        (if (get result :ok)
          (let ((removed (get result :removed))
                (inlets (get result :inlets)))
            (do
              (record-commit-error graph node slot-id nil)
              (if (> (len removed) 0)
                (toast (str "expr committed · removed inlets: " (join-names removed)))
                nil)
              (status (str "expr: committed, inlets: "
                           (if (> (len inlets) 0) (join-names inlets) "none")))))
          (let ((message (get result :error)))
            (do
              (record-commit-error graph node slot-id message)
              (status (str "expr: " message " (card keeps its previous body)"))
              (toast (str "expr: " message) :kind :error))))
        result))))

;; Highlight the span a failed commit reports and put the cursor on its line.
;; `where` is (line column end-line end-column), 0-based, columns in chars; a
;; span running past its first line is highlighted to that line's end.
(def show-error-span (where)
  (if (= where nil)
    (set-buffer-styles '())
    (let ((line (nth where 0))
          (column (nth where 1))
          (end-line (nth where 2))
          (end-column (nth where 3)))
      (do
        (set-buffer-styles
          (list
            (if (and (= end-line line) (> end-column column))
              (dict :line line :start column :end end-column :fg :toast-error :bold true)
              (dict :line line :start column :fg :toast-error :bold true))))
        (goto-line (+ line 1))))))

;; The mode's :on-save handler: commit the active edit buffer's text to its
;; card. True marks the buffer saved; false keeps it modified.
(def commit ()
  (let ((name (current-buffer-name))
        (entry (entry-for-name (current-buffer-name))))
    (if (= entry nil)
      (do (status (str "expr: " name " is not attached to a card")) false)
      (let ((result (commit-source (get entry :graph) (get entry :node) (get entry :slot-id)
                                   (current-buffer-text))))
        (if (= result nil)
          false
          (if (get result :ok)
            (do (set-buffer-styles '()) true)
            (do (show-error-span (get result :where)) false)))))))

(define-mode "eseq.expr-buffer/expr-mode" :on-save "commit")
(mode-bind-key "eseq.expr-buffer/expr-mode" "C-c C-c" "save-buffer")
(mode-bind-key "eseq.expr-buffer/expr-mode" "C-x C-s" "save-buffer")

;; ── Presets (docs/expr-process-spec.md §6.1; bead eseq-waa9.15) ──────────
;;
;; Rows of the node bay's add-process menu that are just expr cards with a
;; body filled in. Picking one adds an `expr` card, commits the body and
;; sets its inlets' starting values, so every preset stays an ordinary,
;; editable expr card. One table; `add-preset` appends to it (packages,
;; init.lisp). Each entry: (dict :label :source :inlets ((name value) …)).
;; `in` is the input inlet a cable lands on. Labels must not repeat a
;; class label of the menu (the pick arrives as the label).
(defstate expr-presets
  (list
    (dict :label "× k" :source "(* in k)" :inlets (list (list "k" 1)))
    (dict :label "+ k" :source "(+ in k)" :inlets (list (list "k" 1)))
    (dict :label "sin" :source "(sin in)" :inlets (list))
    (dict :label "quant" :source "(quant in step)" :inlets (list (list "step" 1)))
    (dict :label "fold" :source "(fold in lo hi)" :inlets (list (list "lo" 0) (list "hi" 1)))
    (dict :label "wrap" :source "(wrap in lo hi)" :inlets (list (list "lo" 0) (list "hi" 1)))
    ;; Not "scale": that row is the node's musical-scale class (neural-scale).
    (dict :label "scale 0..1 → lo..hi" :source "(scale in 0 1 lo hi)" :inlets (list (list "lo" 0) (list "hi" 1)))
    ;; A bouncing ball: k shrinks by decay each fire since the last reset.
    (dict :label "bounce" :source "(* k (pow decay $n))"
          :inlets (list (list "k" 1) (list "decay" 0.8)))
    ;; The spec §9 16-bit Galois LFSR driving the propagation delay; a reset
    ;; restarts it from 0xACE1. taps 46080 = 0xB400.
    (dict :label "lfsr"
          :source "(state s 0xACE1)
(set! s (bit-xor (shr s 1)
                 (if (= (bit-and s 1) 1) taps 0)))
(delay! (* grain (bit-and s 7)))"
          :inlets (list (list "taps" 46080) (list "grain" 1)))))

(def preset-list () expr-presets)

(def preset-labels () (map (lambda (preset) (get preset :label)) expr-presets))

;; The preset with this label, or nil.
(def preset-named (label)
  (reduce |acc preset| (if (and (= acc nil) (= (get preset :label) label)) preset acc)
    nil expr-presets))

;; Add (or replace, by label) a preset. `inlets` is a list of (name value).
(def add-preset (label source inlets)
  (set! expr-presets
    (append
      (filter (lambda (preset) (not (= (get preset :label) label))) expr-presets)
      (list (dict :label label :source source :inlets inlets)))))

;; Add `preset` to node `node` of `graph` as an expr card: commit its body
;; (with the edit buffer's bookkeeping) and set its inlets' starting values.
;; Returns the new slot id, or nil when the add failed.
(def add-node-preset (graph node preset)
  (let ((id (graph-node-process-add graph node "expr")))
    (if (= id nil)
      nil
      (let ((result (commit-source graph node id (get preset :source))))
        (do
          (if (and result (get result :ok))
            (reduce |acc inlet|
                (do (graph-node-process-inlet graph node id (nth inlet 0) (nth inlet 1)) acc)
              nil (get preset :inlets))
            nil)
          id)))))

;; ── Promote to My processes / edit as expr (docs/expr-process-spec.md §8;
;;    bead eseq-waa9.17) ─────────────────────────────────────────────────
;;
;; Promote names an expr card and writes it into the user's own package
;; (<user lisp root>/packages/user.processes/src/<name>.lisp) as a
;; `def-process <name> :in (…) :expr "<body>"` — the body verbatim, compiled
;; by the same pipeline as the card, so the class behaves exactly like it.
;; The module is loaded at once and the card is rebound in place (same slot,
;; values and cables kept by name, runtime state carried): it now shows the
;; class name and is no longer an expr card. The class joins the node add
;; menu and loads on every start. "as expr" reverses it: a card whose class came from an expr card
;; becomes an expr card holding that body.
;;
;; The name modal is mounted by the node editor and by the *processes* dock
;; (a modal only takes pointer input through its own tile); `promote-origin`
;; says which mount shows it: the inspector card passes its home.

;; (dict :graph :node :slot-id), or nil while closed.
(defstate promote-target nil)
(defstate promote-origin "node")
(defstate promote-name "")
;; The last failed promote's message (a write or load problem the live name
;; check cannot see).
(defstate promote-failure nil)

(def promote-open? () (if promote-target true false))

;; Cards turned back into expr cards by "as expr" from a My processes class:
;; (dict :graph :node :slot-id :name) each, newest first. Promote prefills
;; that name, so as expr -> tweak -> promote updates the class in place.
;; Session memory only: after a restart typing the name does the same.
(defstate promote-origins (list))

;; Matched on node and slot id only: callers name the graph differently (an
;; instance ref from the inspector, the dock's recorded source).
(def promote-origin-entry? (entry graph node slot-id)
  (and (= (get entry :node) node) (= (get entry :slot-id) slot-id)))

(def promote-origin-name (graph node slot-id)
  (let ((hits (filter (lambda (e) (promote-origin-entry? e graph node slot-id)) promote-origins)))
    (if (empty? hits) "" (get (first hits) :name))))

(def remember-promote-origin (graph node slot-id name)
  (set! promote-origins
    (cons (dict :graph graph :node node :slot-id slot-id :name name)
          (filter (lambda (e) (not (promote-origin-entry? e graph node slot-id))) promote-origins))))

;; Open the name modal for expr card `slot-id`. `origin` is the mount that
;; shows it: "node" (the node editor) or "dock" (*processes*).
(def open-promote (graph node slot-id origin)
  (do
    (set! promote-name (promote-origin-name graph node slot-id))
    (set! promote-failure nil)
    (set! promote-origin (or origin "node"))
    (set! promote-target (dict :graph graph :node node :slot-id slot-id))))

(def close-promote ()
  (do (set! promote-target nil) (set! promote-failure nil)))

(def set-promote-name (text)
  (do (set! promote-name text) (set! promote-failure nil)))

;; Why the typed name cannot be used, or nil (empty says nothing yet).
(def promote-name-error ()
  (if (or (= promote-target nil) (= (string-trim promote-name) ""))
    nil
    (let ((check (graph-node-process-promote-check (get promote-target :graph) (string-trim promote-name))))
      (if (get check :ok) nil (get check :error)))))

;; Whether the typed name is the user's own My processes class, so the
;; promote replaces it (every card using it picks up the new body).
(def promote-update? ()
  (if (or (= promote-target nil) (= (string-trim promote-name) ""))
    false
    (let ((check (graph-node-process-promote-check (get promote-target :graph) (string-trim promote-name))))
      (if (and (get check :ok) (get check :update)) true false))))

;; Promote expr card `slot-id` of node `node` as `name`: write the module,
;; load it, rebind the slot. `replace` confirms updating the user's own My
;; processes class of that name (the module is rewritten and reloaded, so
;; every card using the class picks it up). Returns {:ok :error :class …}.
(def promote-node-slot (graph node slot-id name replace)
  (let ((written (graph-node-process-promote graph node slot-id name replace)))
    (if (not (get written :ok))
      written
      (let ((loaded (load (get written :path))))
        (if (= loaded (get written :class))
          (let ((rebind (graph-node-process-rebind-class graph node slot-id (get written :class))))
            (do
              (record-commit-error graph node slot-id nil)
              rebind))
          (dict :ok false :class (get written :class)
                :error (str "wrote " (get written :path) " but it did not load: " loaded)))))))

(def commit-promote ()
  (let ((target promote-target)
        (name (string-trim promote-name)))
    (if (or (= target nil) (= name "") (promote-name-error))
      nil
      (let ((updating (promote-update?)))
        (let ((result (promote-node-slot (get target :graph) (get target :node) (get target :slot-id) name updating)))
          (if (get result :ok)
            (do
              (close-promote)
              (toast (str (if updating "updated My processes: " "promoted to My processes: ") name))
              (status (str "expr: " (if updating "updated " "promoted as ") name
                           " (packages/user.processes/src/" name ".lisp)")))
            (do
              (set! promote-failure (get result :error))
              (toast (str "promote: " (get result :error)) :kind :error))))))))

;; Turn a card whose class was promoted from an expr card back into an expr
;; card holding that body (values and cables kept). Returns the native's map.
(def edit-node-slot-as-expr (graph node slot-id)
  (let ((result (graph-node-process-edit-as-expr graph node slot-id)))
    (do
      (if (get result :ok)
        (do
          (if (get result :origin) (remember-promote-origin graph node slot-id (get result :origin)) nil)
          (record-commit-error graph node slot-id nil)
          (status "expr: the card is an expr card again; edit its body and C-c C-c to commit"))
        (toast (str "as expr: " (get result :error)) :kind :error))
      result)))

(def promote-body ()
  (let ((problem (or promote-failure (promote-name-error))))
   (let ((updating (if problem false (promote-update?))))
    (v-stack :width :fill :height :fill :gap 0.5
      (label "Promote expr card" :key "expr-promote-title" :font-size 16 :color :foreground :bg :transparent)
      (label "Name" :font-size 11 :color :dim :bg :transparent)
      (text-input :key "expr-promote-name" :width :fill :height 1.3 :font-size 13
        :value promote-name
        :placeholder "lowercase-name"
        :auto-focus true
        :on-change (lambda (v) (set-promote-name v))
        :on-submit (lambda () (commit-promote))
        :on-cancel (lambda () (close-promote)))
      (label (if problem problem
               (if updating
                 (str "Updates your My processes `" (string-trim promote-name)
                      "`: its file is replaced and every card using it picks up this body.")
                 (str "Saved to My processes as packages/user.processes/src/"
                      (if (= (string-trim promote-name) "") "<name>" (string-trim promote-name)) ".lisp")))
        :key "expr-promote-hint" :width :fill :font-size 10 :wrap true
        :color (if problem :toast-error (if updating :process-lane-accent :dim)) :bg :transparent)
      (label "The card becomes that process: values and cables stay, and it is added to every node's add menu."
        :key "expr-promote-note" :width :fill :font-size 10 :wrap true :color :dim :bg :transparent)
      (box :flex 1 :bg :transparent)
      (h-stack :width :fill :gap 0.5
        (box :flex 1 :bg :transparent)
        (button "Cancel" :key "expr-promote-cancel" :on-click |x y r| (close-promote))
        (button (if updating "Update" "Promote") :key "expr-promote-submit" :variant :primary
          :disabled (or (= (string-trim promote-name) "") (if problem true false))
          :on-click |x y r| (commit-promote)))))))

;; The name modal, for the mount `origin` ("node" or "dock").
(def promote-panel (origin)
  (modal :key (str "expr-promote-modal-" origin)
    :is-open (and (promote-open?) (= promote-origin origin))
    :on-close (lambda () (close-promote))
    :title "Promote to My processes" :width-px 520 :height-px 460
    (if (and (promote-open?) (= promote-origin origin)) (promote-body) (box :height 0 :width 0))))

