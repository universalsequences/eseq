;; ui/markdown-view.lisp -- markdown as widgets, for any panel that shows
;; prose: the *manual* and the agent pane.
;;
;; Rust parses the text (`parse-manual-source` -> the AST of
;; docs/manual-spec.md §4) and this module renders its general blocks:
;; headings, paragraphs, lists and code blocks. Prose flows as per-word
;; labels in a `wrap` container (`manual-wrap-runs` groups the words that
;; must stay glued); code sits in tinted chips.
;;
;; Every renderer takes `opts`, a dict:
;;   :size      body font size (default 13)
;;   :on-link   (fn target) for a clicked link; without it links are plain
;;   :on-action (fn form) for a clicked action link; without it, plain
;;   :other     (fn block) for blocks this module does not draw (the
;;              manual's menus and images); without it they are skipped
;;
;; A library module: import it from the panels that use it.
(module eseq.markdown-view)

(export render-markdown render-blocks render-inlines external-link? open-external)

(def body-size (opts)
  (let ((size (get opts :size)))
    (if size size 13)))

(def code-chip (text opts)
  (box :background-color :button-ghost-bg :corner-radius 3 :padding 0.08
    (label text :font-size (body-size opts) :color :cyan :mono true :bg :transparent)))

;; A web address, as opposed to a manual node or other relative target.
(def external-link? (target)
  (or (string-starts-with? target "https://") (string-starts-with? target "http://")))

;; Open a web address in the browser.
(def open-external (target)
  (host-command "open-url" (dict :url target)))

;; A link: clickable when `handler` is given, else drawn like one.
(def link-label (text target handler color opts)
  (if handler
    (label text :font-size (body-size opts) :color color :hover-color :white :bg :transparent
      :underline true :on-click (lambda (event) (handler target)))
    (label text :font-size (body-size opts) :color color :bg :transparent :underline true)))

(def render-fragment (frag opts)
  (let ((kind (nth frag 0)) (text (nth frag 1)) (size (body-size opts)))
    (cond
      ((= kind 'code) (code-chip text opts))
      ((= kind 'link) (link-label text (nth frag 2) (get opts :on-link) :blue opts))
      ((= kind 'action-link) (link-label text (nth frag 2) (get opts :on-action) :accent opts))
      ((= kind 'b) (label text :font-size size :color :white :bg :transparent))
      ((= kind 'em) (label text :font-size size :color :cyan :bg :transparent))
      (else (label text :font-size size :color :fg :bg :transparent)))))

(def render-group (group opts)
  (h-stack :gap 0
    (each (range 0 (len group)) |i| (render-fragment (nth group i) opts))))

;; Prose: one widget per glued word group, flowing in a wrap container.
(def render-inlines (inlines opts)
  (let ((groups (manual-wrap-runs inlines)))
    (wrap :width :fill :gap 0.35 :row-gap 0.12
      (each (range 0 (len groups)) |i| (render-group (nth groups i) opts)))))

;; Headings scale with the body size: 13 gives the manual's 24 / 17 / 15.
(def heading-size (head opts)
  (let ((size (body-size opts)))
    (cond
      ((= head 'h1) (round (* size 1.85)))
      ((= head 'h2) (round (* size 1.3)))
      (else (round (* size 1.15))))))

(def render-heading (block opts)
  (label (nth block 1) :font-size (heading-size (nth block 0) opts) :bg :transparent
    :width :fill :wrap true :color :dim))

(def render-code-block (block opts)
  (let ((lines (string-split (nth block 2) "
")))
    (box :width :fill :background-color :button-ghost-bg :corner-radius 4 :padding 0.4
      (v-stack :width :fill :gap 0.05
        (each (range 0 (len lines)) |i|
          (label (nth lines i) :font-size (body-size opts) :color :cyan :mono true :bg :transparent))))))

;; The marker is the first group of the item's own wrap container: a flexed
;; sibling would be measured at unbounded width and report a one-line height,
;; so wrapped items would overlap the next item.
(def render-list-item (marker item opts)
  (let ((groups (manual-wrap-runs (rest item))))
    (wrap :width :fill :gap 0.35 :row-gap 0.12
      (label marker :width 2 :font-size (body-size opts) :color :dim :bg :transparent)
      (each (range 0 (len groups)) |i| (render-group (nth groups i) opts)))))

(def render-list (block opts)
  (let ((items (rest block)) (ordered (= (nth block 0) 'ol)))
    (v-stack :width :fill :gap 0.15 :padding-left 0.5
      (each (range 0 (len items)) |i|
        (render-list-item (if ordered (str (+ i 1) ".") "•") (nth items i) opts)))))

(def render-block (block opts)
  (let ((head (nth block 0)) (other (get opts :other)))
    (cond
      ((or (= head 'h1) (= head 'h2) (= head 'h3)) (render-heading block opts))
      ((= head 'p) (render-inlines (rest block) opts))
      ((= head 'code-block) (render-code-block block opts))
      ((or (= head 'ul) (= head 'ol)) (render-list block opts))
      (other (other block))
      (else (box :height 0)))))

;; The blocks of a parsed page (its `rest`), stacked.
(def render-blocks (blocks opts)
  (v-stack :width :fill :gap (* 0.065 (body-size opts))
    (each (range 0 (len blocks)) |i| (render-block (nth blocks i) opts))))

;; Markdown text, parsed and rendered.
(def render-markdown (text opts)
  (render-blocks (rest (parse-manual-source text)) opts))
