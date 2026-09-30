;; The jaki kind's document and its tick (docs/jaki-kind-spec.md §5).
;;
;; A `jaki` instance's document is plain data (scene slots, so literals only):
;;
;;   figures   ((:dot :dot :dash) (4 :dot :dash))    one list per figure; a
;;                                                   leading count repeats it
;;   rows      ((dict :route 0 :mods (("trunc" 3) "right")) …)
;;   row-count 8                                      rows that show and play
;;
;; A row's :mods is the value of its sexp-slot (docs/sexp-slot-spec.md): a
;; list of items, each a word "left", a form ("trunc" 3) / ("every" 2 "rev"),
;; or a word cycle ("left" "left" "right"). Any number or word argument may be
;; a list, one per cycle: ("fast" (1 2)), ("every" 2 ("rev" "swap")). Rows
;; saved before the slot hold (dict :op "trunc" :args (3)) records; they
;; still read (`mod-item`).
;;
;; `body` turns it into ordinary jaki body data, the same shape a `(jak …)`
;; form hands `alez.jaki.core/run`:
;;
;;   (fig (. . -)) (fig (. -)) -> 0 (trunc 3) right -> 1 accent
;;
;; so every evaluator rule, hand model and memo is the language's own. This
;; module is what the scheduler imports for the tick (`:requires`); it holds
;; no widget code. The view lives in alez.jaki.kind.

(module alez.jaki.doc)

(import alez.jaki.core)

(export tick body row-at row-live? default-row mod-form mod-item row-mods
        row-schema figure-label figure-times figure-events with-times
        figure-options figure-option-labels zero-mods
        num-mods mod-arg-spec every-words split-targets
        figures-body modes mode-labels mode-of)

;; ── figures ─────────────────────────────────────────────────────────────────

(def event-sym (kw) (if (= kw :dash) '- '.))
(def event-text (kw) (if (= kw :dash) "-" "."))

(def join-words (words)
  (reduce (lambda (acc w) (if (= acc "") w (str acc " " w))) "" words))

;; A figure is its events, optionally led by a repeat count: (4 :dot :dash)
;; is four copies of `. -`. Figures saved before counts have none (1).
(def figure-times (figure)
  (if (and (not (empty? figure)) (number? (first figure))) (first figure) 1))
(def figure-events (figure)
  (if (and (not (empty? figure)) (number? (first figure))) (rest figure) figure))
(def with-times (figure n)
  (let ((events (figure-events figure)) (k (max 1 (round n))))
    (if (= k 1) events (cons k events))))

(def figure-label (figure) (join-words (map event-text (figure-events figure))))

;; Every figure of 1..5 units (`.` = 1, `-` = 2), shortest first, dots before
;; dashes: 19 of them. Built once; each option is (label events).
(def compositions (n)
  (if (= n 0)
    (list (list))
    (if (< n 0)
      (list)
      (append
        (map (lambda (c) (cons :dot c)) (compositions (- n 1)))
        (map (lambda (c) (cons :dash c)) (compositions (- n 2)))))))

(def figure-options
  (reduce (lambda (acc n)
            (append acc (map (lambda (f) (list (figure-label f) f)) (compositions n))))
          (list) (range 1 6)))

(def figure-option-labels (map (lambda (o) (nth o 0)) figure-options))

;; ── row modifiers (route words) ─────────────────────────────────────────────

;; Words that take nothing, in menu order.
(def zero-mods (list "left" "right" "accent" "rev" "stac" "ghost" "swap" "half"))
;; Words that take one number. Every one is resolved per cycle by jaki core,
;; so a list of numbers cycles: (fast (1 2)).
(def num-mods (list "trunc" "rot" "shift" "fast" "slow" "gate" "vel" "note"
                    "vel*" "vel+" "note+"
                    "basevel" "dotdecay" "dashdecay" "minvel" "maxvel"))
(def every-words zero-mods)
(def split-targets (list "first" "last" "all"))

;; (min max step decimals default) of a word's number argument, or nil.
(def mod-arg-spec (op)
  (match op
    "trunc" (list 1 32 1 0 3)
    "rot"   (list -16 16 1 0 1)
    "shift" (list -32 32 1 0 1)
    "fast"  (list 1 8 1 0 2)
    "slow"  (list 1 8 1 0 2)
    "gate"  (list 0 4 0.05 2 0.5)
    "vel"   (list 0 2 0.05 2 0.7)
    "note"  (list -48 48 1 0 12)
    "vel*"  (list 0 2 0.05 2 0.8)
    "vel+"  (list -1 1 0.05 2 -0.1)
    "note+" (list -48 48 1 0 7)
    "fig"   (list 1 16 1 0 1)
    "rep"   (list 1 16 1 0 1)
    "nth"   (list 1 16 1 0 2)
    "basevel"   (list 0 1 0.01 2 0.8)
    "dotdecay"  (list 0 1 0.01 2 0.85)
    "dashdecay" (list 0 1 0.01 2 0.9)
    "minvel"    (list 0 1 0.01 2 0.3)
    "maxvel"    (list 0 1 0.01 2 1.0)
    "every" (list 1 16 1 0 4)
    "every-fig" (list 1 16 1 0 2)
    _ nil))

;; ── the row's sexp-slot schema (docs/sexp-slot-spec.md §4) ─────────────────

(def num-schema (op)
  (let ((s (mod-arg-spec op)))
    (list "num" :min (nth s 0) :max (nth s 1) :step (nth s 2) :decimals (nth s 3)
          :default (nth s 4))))

;; A modifier row: a zero-arg word, a number word, `every n w` /
;; `every-fig n w` (w any zero-arg or number word — (every-fig 2 (fast 2)),
;; (every 4 (note (0 7))) — or a list of them, one per cycle), `L w` / `R w`,
;; and `split` / `merge` whose target is read as-is by jaki core, so it never
;; cycles.
;; (quant :8): snap every hit to the nearest step of a grid. The grid is
;; converted to units once per route, so it never cycles.
(def quant-grids (list ":8" ":16" ":32" ":4" ":2" ":1" ":8t" ":16t" ":32t" ":4t"))
(def quant-form (list "form" "quant" (list "fixed" (cons "word" quant-grids))))

;; Per-hit values also take a value sequence, (seq :clock v…) — next value
;; each hit / figure / cycle, or by position in the cycle (jaki-sequencer-spec
;; §7.2). The clock word leads; `+` inside adds a value. A plain list of
;; numbers still means one per cycle.
(def seq-clocks (list ":hit" ":fig" ":cycle" ":span"))
(def per-hit-mods (list "vel" "note" "vel*" "vel+" "note+" "gate"))
;; A seq's values may be seqs themselves — (seq :hit 2 (seq :cycle 5 9) 3).
;; A schema is plain data and cannot refer to itself, so the nesting is
;; spelled out seq-depth levels deep (a plain number list nests freely).
(def seq-depth 3)
(def seq-value-schema (op depth)
  (if (<= depth 0)
    (num-schema op)
    (list "or" (num-schema op)
          (list "form" "seq" (cons "word" seq-clocks)
                (list "rest" (seq-value-schema op (- depth 1)))))))

(def value-schema (op)
  (if (reduce (lambda (a x) (or a (= x op))) false per-hit-mods)
    (seq-value-schema op seq-depth)
    (num-schema op)))

(def num-forms (map (lambda (op) (list "form" op (value-schema op))) num-mods))

(def gated-word (append (list "or" (cons "word" every-words)) num-forms (list quant-form)))

;; (on SEL word) — the word applies only where the selector holds
;; (crates/sequencer/docs/jaki-sequencer-spec.md §7.1). A selector is a tag
;; word, a figure/cycle term, or two of those under and / or; schemas are
;; data, so the nesting is spelled out two levels deep.
(def sel-words (list "left" "right" "dot" "dash" "head" "tail" "accent"))
(def sel-num-form (lambda (h) (list "form" h (num-schema h))))
(def sel-leaf
  (append (list "or" (cons "word" sel-words)) (map sel-num-form (list "fig" "rep" "every"))))
(def sel-term
  (append sel-leaf
          (list (list "form" "not" sel-leaf)
                (list "form" "nth" (num-schema "nth")
                      (append (list "or" (list "word" "any"))
                              (map sel-num-form (list "fig" "rep")))))))
(def sel-schema
  (append sel-term
          (list (list "form" "and" sel-term sel-term)
                (list "form" "or" sel-term sel-term))))

;; the scoped word: vel+ first so a fresh (on …) reads (on left (vel+ -0.1));
;; `rest` drops the selection; (every n w) narrows it to every nth cycle
(def on-body
  (append (list "or" (list "form" "vel+" (num-schema "vel+"))
                (cons "word" (append zero-mods (list "rest"))))
          (filter (lambda (f) (not (= (nth f 1) "vel+"))) num-forms)
          (list quant-form
                (list "form" "every" (num-schema "every") gated-word))))

(def row-schema
  (append
    (list "forms" (cons "word" zero-mods))
    num-forms
    (list quant-form)
    (list (list "form" "on" sel-schema on-body)
          (list "form" "every" (num-schema "every") gated-word)
          (list "form" "every-fig" (num-schema "every-fig") gated-word)
          (list "form" "L" (cons "word" every-words))
          (list "form" "R" (cons "word" every-words))
          (list "form" "split" (list "fixed" (cons "word" split-targets)))
          (list "form" "merge" (list "fixed" (cons "word" split-targets))))))

;; ── modifier items -> route words ──────────────────────────────────────────

;; A saved-before-the-slot record (dict :op "trunc" :args (3)) as a slot item:
;; "left", ("trunc" 3). Items already in the slot's shape pass through.
(def mod-item (m)
  (let ((op (get m :op)))
    (if (string? op)
      (let ((args (get m :args)))
        (if (empty? args) op (cons op args)))
      m)))

;; A row's modifiers as the slot's value.
(def row-mods (row) (map mod-item (get row :mods)))

;; Slot data -> route word data: a word string reads as its symbol (":16t"
;; as its keyword), numbers stay numbers, lists recurse. So "left" is
;; `left`, ("fast" (1 2)) is `(fast (1 2))`, ("every" 2 ("rev" "swap")) is
;; `(every 2 (rev swap))` and ("left" "left" "right") is a word cycle.
(def route-datum (x)
  (if (string? x)
    (read-string x)
    (if (number? x) x (map route-datum x))))

(def mod-form (m) (route-datum (mod-item m)))

;; ── rows ────────────────────────────────────────────────────────────────────

(def default-row (dict :route -1 :mods (list)))

(def row-at (rows i)
  (if (< i (len rows)) (nth rows i) default-row))

(def row-live? (row)
  (let ((route (get row :route)))
    (and (number? route) (>= route 0))))

(def row-segment (row)
  (cons '-> (cons (get row :route) (map mod-form (get row :mods)))))

;; ── document -> body ────────────────────────────────────────────────────────

;; The figure strip as jaki pattern data: (fig (. . -)) (fig (. -) (rep 4)) …
(def figures-body (figures)
  (map (lambda (f)
         (let ((events (list 'fig (map event-sym (figure-events f))))
               (n (figure-times f)))
           (if (> n 1) (append events (list (list 'rep n))) events)))
       figures))

;; The jaki body, or nil when nothing can sound (no figure, or no live row:
;; a body without routes would play track 0).
(def body (figures rows row-count)
  (let ((live (filter row-live?
                (map (lambda (i) (row-at rows i)) (range 0 row-count)))))
    (if (or (empty? figures) (empty? live))
      nil
      (append
        (figures-body figures)
        (reduce (lambda (acc row) (append acc (row-segment row))) (list) live)))))

;; `body` per document, so a steady document costs one deep `=` per tick
;; instead of a rebuild (read-string per word). A few entries: every jaki
;; instance's tick runs on this VM.
(def body-memo (list))

(def body-memo-find (m key)
  (if (empty? m)
    (list)
    (if (= (nth (first m) 0) key)
      (list (nth (first m) 1))
      (body-memo-find (rest m) key))))

(def memo-body (figures rows row-count)
  (let ((key (list figures rows row-count)))
    (let ((hit (body-memo-find body-memo key)))
      (if (empty? hit)
        (let ((b (body figures rows row-count)))
          (do (set! body-memo (cons (list key b)
                                    (if (< (len body-memo) 8)
                                      body-memo
                                      (map (lambda (i) (nth body-memo i)) (range 0 7)))))
              b))
        (first hit)))))

;; ── modes ───────────────────────────────────────────────────────────────────

(def modes (list :loop :retrig :continue :gate))
(def mode-labels (list "loop" "retrig" "continue" "gate"))

;; documents saved before modes have none: they loop
(def mode-of (m) (if (or (= m :retrig) (= m :continue) (= m :gate)) m :loop))

;; The kind's generator tick: `self` is the instance's document map. It
;; stamps tick + 1 for the panel's playhead (alez.jaki.kind, hit strip); 0 is
;; the host's "stopped".
;; `mode` is :loop, :retrig, :continue or :gate (docs/jaki-trig-modes-spec.md);
;; the mark follows the clock's position, and is 0 while the gate is shut.
(def tick (figures rows row-count mode)
  (do
    (alez.jaki.core/init :16)
    (let ((playing (alez.jaki.core/step-clock (mode-of mode))))
      (do
        (gen-mark (if playing (+ (alez.jaki.core/play-pos) 1) 0))
        (let ((b (memo-body figures rows row-count)))
          (if (and b playing) (alez.jaki.core/play-routes b) 0))))))
