;; ui/manual.lisp -- the *manual* buffer: an Emacs-Info-style reader over the
;; markdown nodes in docs/manual/ (see docs/manual-spec.md).
;;
;; Rust parses one node at a time (`parse-manual-page` -> the §4 AST) and
;; this file renders that AST to widgets: prose flows as per-word labels in a
;; `wrap` container (`manual-wrap-runs` groups the words that must stay
;; glued, e.g. a link's last word and the comma after it), links are
;; hover-coloured labels, code and key chords sit in tinted chips, and a menu
;; is one row per entry. Navigation state (current node, back history, the
;; parent each node was reached from) is plain Lisp state here.
;;
;; Render root: registers the *manual* buffer, its mode and keys at top
;; level. Import it only from main.lisp, never from a library module.
(module eseq.manual)

(export open-manual
        open-node
        manual-node
        manual-source-buffer
        manual-quit
        manual-back
        manual-up
        manual-next
        manual-prev
        manual-top
        manual-reload
        open-menu-entry
        follow-link
        run-action)

;; ── State ──

(defstate manual-node "")
(defstate manual-page (list 'page))
(defstate manual-source-buffer "")
;; Back history: node names, most recent first.
(defstate manual-history (list))
;; (child parent) pairs recording the menu each node was reached through, so
;; n/p/u follow the parent's menu order (spec §1).
(defstate manual-parents (list))
(defstate manual-generation 0)

(def root-node "index")

;; ── Loading ──

(def node-path (node)
  (seq-manual-path (str node ".md")))

(def missing-page (node)
  (parse-manual-source (str "# Missing page

There is no manual page named `" node "`.

Back to the [top page](index).")))

(def load-page (node)
  (let ((path (node-path node)))
    (if (file-exists? path)
      (parse-manual-page path)
      (missing-page node))))

(def page-title (page)
  (reduce |acc block|
    (if (and (= acc "") (= (nth block 0) 'h1)) (nth block 1) acc)
    "" (rest page)))

;; The menu entries of a page as a flat list of target names, in order.
(def page-menu-targets (page)
  (reduce |acc block|
    (if (= (nth block 0) 'menu)
      (append acc (map |entry| (nth entry 2) (rest block)))
      acc)
    (list) (rest page)))

(def assoc-lookup (pairs key)
  (reduce |acc pair| (if (= (nth pair 0) key) (nth pair 1) acc) "" pairs))

(def parent-of (node)
  (let ((known (assoc-lookup manual-parents node)))
    (if (not (= known ""))
      known
      (if (= node root-node)
        ""
        root-node))))

;; ── Navigation ──

(def show-node (node)
  (do
    (set! manual-page (load-page node))
    (set! manual-node node)
    (set! manual-generation (+ manual-generation 1))))

(def open-node (node)
  (do
    (if (not (= manual-node ""))
      (set! manual-history (cons manual-node manual-history))
      nil)
    (show-node node)))

;; A menu entry click: remember which page it was reached from.
(def open-menu-entry (node)
  (do
    (set! manual-parents (cons (list node manual-node) manual-parents))
    (open-node node)))

(def open-manual ()
  (do
    (if (not (= (current-buffer-name) "*manual*"))
      (set! manual-source-buffer (current-buffer-name))
      nil)
    (if (= manual-node "")
      (show-node root-node)
      nil)
    (switch-to-buffer "*manual*")))

(def manual-quit ()
  (if (not (= manual-source-buffer ""))
    (switch-to-buffer manual-source-buffer)
    (switch-to-buffer "*sequencer*")))

(def manual-back ()
  (if (empty? manual-history)
    (status "No earlier manual page")
    (let ((node (first manual-history)))
      (do
        (set! manual-history (rest manual-history))
        (show-node node)))))

(def manual-top ()
  (open-node root-node))

(def manual-up ()
  (let ((parent (parent-of manual-node)))
    (if (= parent "")
      (status "Already at the top of the manual")
      (open-node parent))))

(def sibling-index (targets node)
  (reduce |acc i| (if (= (nth targets i) node) i acc) -1 (range 0 (len targets))))

(def open-sibling (delta)
  (let ((parent (parent-of manual-node)))
    (if (= parent "")
      (status "This page has no siblings")
      (let ((targets (page-menu-targets (load-page parent))))
        (let ((i (sibling-index targets manual-node)))
          (let ((j (+ i delta)))
            (if (or (< i 0) (< j 0) (>= j (len targets)))
              (status (if (< delta 0) "First page in this chapter" "Last page in this chapter"))
              (do
                (set! manual-parents (cons (list (nth targets j) parent) manual-parents))
                (open-node (nth targets j))))))))))

(def manual-next () (open-sibling 1))
(def manual-prev () (open-sibling -1))

(def manual-reload ()
  (if (= manual-node "")
    (status "The manual is not open")
    (show-node manual-node)))

;; ── Links ──

(def external-link? (target)
  (or (string-starts-with? target "https://") (string-starts-with? target "http://")))

(def follow-link (target)
  (if (external-link? target)
    (host-command "open-url" (dict :url target))
    (open-node target)))

;; Action links carry a Lisp form without its outer parens (spec §3.1); it is
;; evaluated only here, on click.
(def run-action (form)
  (eval (str "(" form ")")))

;; ── Rendering ──

(def body-size 13)

(def code-chip (text)
  (box :background-color :button-ghost-bg :corner-radius 3 :padding 0.08
    (label text :font-size body-size :color :cyan :bg :transparent)))

(def render-fragment (frag)
  (let ((kind (nth frag 0)) (text (nth frag 1)))
    (if (= kind 'code)
      (code-chip text)
      (if (= kind 'link)
        (label text :font-size body-size :color :blue :hover-color :white :bg :transparent
          :underline true :on-click (lambda (event) (follow-link (nth frag 2))))
        (if (= kind 'action-link)
          (label text :font-size body-size :color :accent :hover-color :white :bg :transparent
            :underline true :on-click (lambda (event) (run-action (nth frag 2))))
          (if (= kind 'b)
            (label text :font-size body-size :color :white :bg :transparent)
            (if (= kind 'em)
              (label text :font-size body-size :color :cyan :bg :transparent)
              (label text :font-size body-size :color :fg :bg :transparent))))))))

(def render-group (group)
  (h-stack :gap 0
    (each (range 0 (len group)) |i| (render-fragment (nth group i)))))

;; Prose: one widget per glued word group, flowing in a wrap container.
(def render-inlines (inlines)
  (let ((groups (manual-wrap-runs inlines)))
    (wrap :width :fill :gap 0.35 :row-gap 0.12
      (each (range 0 (len groups)) |i| (render-group (nth groups i))))))

(def heading-size (head)
  (if (= head 'h1) 24 (if (= head 'h2) 17 15)))

(def render-heading (block)
  (label (nth block 1) :font-size (heading-size (nth block 0)) :color :white :bg :transparent
    :width :fill :wrap true :color :dim))

(def render-code-block (block)
  (let ((lines (string-split (nth block 2) "
")))
    (box :width :fill :background-color :button-ghost-bg :corner-radius 4 :padding 0.4
      (v-stack :width :fill :gap 0.05
        (each (range 0 (len lines)) |i|
          (label (nth lines i) :font-size body-size :color :cyan :bg :transparent))))))

;; The marker is the first group of the item's own wrap container: a flexed
;; sibling would be measured at unbounded width and report a one-line height,
;; so wrapped items would overlap the next item.
(def render-list-item (marker item)
  (let ((groups (manual-wrap-runs (rest item))))
    (wrap :width :fill :gap 0.35 :row-gap 0.12
      (label marker :width 2 :font-size body-size :color :dim :bg :transparent)
      (each (range 0 (len groups)) |i| (render-group (nth groups i))))))

(def render-list (block)
  (let ((items (rest block)) (ordered (= (nth block 0) 'ol)))
    (v-stack :width :fill :gap 0.15 :padding-left 0.5
      (each (range 0 (len items)) |i|
        (render-list-item (if ordered (str (+ i 1) ".") "•") (nth items i))))))

(def render-menu-entry (entry)
  (wrap :width :fill :gap 0.6 :row-gap 0.12
    (label "•" :font-size body-size :color :dim :bg :transparent)
    (label (nth entry 1) :font-size body-size :color :blue :hover-color :white :bg :transparent
      :underline true :on-click (lambda (event) (open-menu-entry (nth entry 2))))
    (label (nth entry 3) :font-size body-size :color :dim :bg :transparent)))

(def render-menu (block)
  (let ((entries (rest block)))
    (v-stack :width :fill :gap 0.15 :padding-left 0.5 :debug-name "manual-menu"
      (each (range 0 (len entries)) |i| (render-menu-entry (nth entries i))))))

(def render-image (block)
  (let ((alt (nth block 1))
        (info (manual-image-info (node-path manual-node) (nth block 2))))
    (v-stack :width :fill :gap 0.25 :debug-name "manual-figure"
      (if (get info :error)
        (render-inlines (list (list 'span (str "Image unavailable: " (get info :error)))))
        (image :src (get info :path) :width :fill
          :max-pixel-width (get info :width) :aspect (get info :aspect) :fit :contain))
      (if (= alt "") (box :height 0)
        (render-inlines (list (list 'em alt)))))))

(def render-block (block)
  (let ((head (nth block 0)))
    (if (or (= head 'h1) (= head 'h2) (= head 'h3))
      (render-heading block)
      (if (= head 'p)
        (render-inlines (rest block))
        (if (= head 'code-block)
          (render-code-block block)
          (if (or (= head 'ul) (= head 'ol))
            (render-list block)
            (if (= head 'menu)
              (render-menu block)
              (if (= head 'image)
                (render-image block)
                (box :height 0)))))))))

(def nav-button (text key handler)
  (h-stack :gap 0.2 :align :center
    (label text :font-size 11 :color :blue :hover-color :white :bg :transparent
      :on-click (lambda (event) (handler)))
    (label key :font-size 10 :color :dimmer :bg :transparent)))

(def render-nav-bar ()
  (h-stack :key "manual-navigation" :width :fill :gap 1.0 :align :center :debug-name "manual-nav"
    (label (str "eseq manual · " manual-node) :font-size 11 :color :dim :bg :transparent :flex 1)
    (nav-button "Top" "t" manual-top)
    (nav-button "Back" "l" manual-back)
    (nav-button "Up" "u" manual-up)
    (nav-button "Prev" "p" manual-prev)
    (nav-button "Next" "n" manual-next)
    (nav-button "Close" "q" manual-quit)))

(def render-page ()
  (let ((blocks (rest manual-page)))
    (v-stack :width :fill :gap 0.85 :debug-name "manual-page"
      (each (range 0 (len blocks)) |i| (render-block (nth blocks i))))))

(def root-widget ()
  (let ((generation manual-generation))
    (v-stack :width :fill :height :fill :gap 0.4 :padding 0.6
      (render-nav-bar)
      (scroll :key "manual-scroll" :width :fill :flex 1
        (box :width :fill :padding-right 1.0
          (render-page))))))

(effect-buffer "*manual*"
  (root-widget))

;; ── Mode and keys ──

(define-mode "eseq.manual/manual-mode" :read-only true)
(mode-bind-key "eseq.manual/manual-mode" "q" "manual-quit")
(mode-bind-key "eseq.manual/manual-mode" "l" "manual-back")
(mode-bind-key "eseq.manual/manual-mode" "u" "manual-up")
(mode-bind-key "eseq.manual/manual-mode" "n" "manual-next")
(mode-bind-key "eseq.manual/manual-mode" "p" "manual-prev")
(mode-bind-key "eseq.manual/manual-mode" "t" "manual-top")
(mode-bind-key "eseq.manual/manual-mode" "g" "manual-reload")
;; Set the mode after the buffer exists (effect-buffer creates it above).
(set-buffer-mode-for "*manual*" "eseq.manual/manual-mode")

;; `C-h` alone belongs to the sequencer's collapse-all-tracks; the chord
;; leaves it alone. File > Help reaches the same place.
(bind-key "C-h m" "eseq.manual/open-manual")
