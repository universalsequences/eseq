;; The jaki kind's document and its tick (docs/jaki-kind-spec.md §5).
;;
;; A `jaki` instance's document is plain data (scene slots, so literals only):
;;
;;   figures   ((:dot :dot :dash) (4 :dot :dash))    one list per figure; a
;;                                                   leading count repeats it
;;   rows      ((dict :route 0 :mods (("trunc" 3) "right")
;;                    :procs (("left" ("coin" 0.5) ("next" "rest")))
;;                    :seed 3) …)
;;   row-count 8                                      rows that show and play
;;   patterns  ((dict :figures … :rows … :row-count 4) …)
;;                                                   patterns after the first
;;
;; The first pattern is figures/rows/row-count (documents saved before
;; patterns hold only it); each further pattern is its own figure strip and
;; rows, played as one more jaki voice — its own cycle, beside the others.
;;
;; A row's :mods is the value of its sexp-slot (docs/sexp-slot-spec.md): a
;; list of items, each a word "left", a form ("trunc" 3) / ("every" 2 "rev"),
;; or a word cycle ("left" "left" "right"). Any number or word argument may be
;; a list, one per cycle: ("fast" (1 2)), ("every" 2 ("rev" "swap")). Rows
;; saved before the slot hold (dict :op "trunc" :args (3)) records; they
;; still read (`mod-item`).
;;
;; A row's :procs are its row process chains (docs/jaki-row-processes-spec.md
;; §9), each a list in the same encoding: the trigger (one selector item; a
;; multi-term trigger is ("and" …)) then stage and action items. They become
;; one (rule …) route word each, after the mods; :seed (optional) becomes
;; (seed n).
;;
;; `body` turns it into ordinary jaki body data, the same shape a `(jak …)`
;; form hands `alez.jaki.core/run`:
;;
;;   (fig (. . -)) (fig (. -)) -> 0 (trunc 3) right -> 1 accent
;;
;; With more than one pattern sounding, each is a voice line:
;;
;;   ((fig (. . -)) -> 0 left) ((fig (. -)) -> 2 left)
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
        figures-body modes mode-labels mode-of
        pattern-list pattern-sounds? patterns-body new-pattern tick-patterns
        row-procs proc-word new-chain chain-trigger-items chain-with-trigger
        proc-trigger-schema proc-body-schema)

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
    "dotdecay"  (list 0 1 0.01 2 0.5)
    "dashdecay" (list 0 1 0.01 2 0.5)
    "minvel"    (list 0 1 0.01 2 0)
    "maxvel"    (list 0 1 0.01 2 1.0)
    "every" (list 1 16 1 0 4)
    "every-fig" (list 1 16 1 0 2)
    ;; a plock value is in its parameter's own units: the editor takes its
    ;; rails from the named param (leaf-schema's dyn-num); this wide rail is
    ;; the fallback for an unknown name or an unrouted row
    "plock" (list -100000 100000 0.01 2 0)
    _ nil))

;; ── schema atoms shared by the row slot and the process chain slots ───────

(def pnum (lo hi step dec def)
  (list "num" :min lo :max hi :step step :decimals dec :default def))
(def pword (words) (list "fixed" (cons "word" words)))
(def pkw (k) (pword (list k)))

(def scale-modes
  (list ":major" ":minor" ":dorian" ":phrygian" ":lydian" ":mixolydian" ":locrian"
        ":harmonic-minor" ":melodic-minor" ":pentatonic" ":minor-pentatonic"
        ":blues" ":whole-tone" ":chromatic"))
(def note-roots (list "C" "Db" "D" "Eb" "E" "F" "Gb" "G" "Ab" "A" "Bb" "B"))

;; (scale :minor :root C) and (harmony :track 1 :amount 1): row words and
;; chain actions alike (docs/jaki-row-processes-spec.md §13). The track comes
;; from the host's `track` word source (0-based, the name beside it).
(def scale-form (list "form" "scale" (pword scale-modes) (pkw ":root") (pword note-roots)))
(def harmony-form
  (list "form" "harmony" (pkw ":track") (list "fixed" (list "dyn" "track"))
                         (pkw ":amount") (pnum 0 1 0.05 2 1)))

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
(def per-hit-mods (list "vel" "note" "vel*" "vel+" "note+" "gate" "plock"))
;; A seq's values may be seqs themselves — (seq :hit 2 (seq :cycle 5 9) 3).
;; A schema is plain data and cannot refer to itself, so the nesting is
;; spelled out seq-depth levels deep (a plain number list nests freely).
(def seq-depth 3)
(def leaf-schema (op)
  ;; a plock number scrubs in its sibling param's range (sexp-slot-spec §4.1)
  (if (= op "plock")
    (list "dyn-num" "param" (num-schema op))
    (num-schema op)))
(def seq-value-schema (op depth)
  (if (<= depth 0)
    (leaf-schema op)
    (list "or" (leaf-schema op)
          (list "form" "seq" (cons "word" seq-clocks)
                (list "rest" (seq-value-schema op (- depth 1)))))))

(def value-schema (op)
  (if (reduce (lambda (a x) (or a (= x op))) false per-hit-mods)
    (seq-value-schema op seq-depth)
    (leaf-schema op)))

(def num-forms (map (lambda (op) (list "form" op (value-schema op))) num-mods))


;; (plock NAME V) — a per-hit parameter lock (docs/jaki-plock-spec.md §5.3).
;; NAME is a string completed by the host's `param` source for the row's
;; destination track (the slot's :dyn-context); V takes seqs like note's.
(def plock-form (list "form" "plock" (list "fixed" (list "dyn" "param")) (value-schema "plock")))

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
                plock-form
                (list "form" "every" (num-schema "every") gated-word))))

(def row-schema
  (append
    (list "forms" (cons "word" zero-mods))
    num-forms
    (list quant-form plock-form scale-form harmony-form)
    (list (list "form" "on" sel-schema on-body)
          (list "form" "every" (num-schema "every") gated-word)
          (list "form" "every-fig" (num-schema "every-fig") gated-word)
          (list "form" "L" (cons "word" every-words))
          (list "form" "R" (cons "word" every-words))
          (list "form" "split" (list "fixed" (cons "word" split-targets)))
          (list "form" "merge" (list "fixed" (cons "word" split-targets))))))

;; ── row process chain schemas (docs/jaki-row-processes-spec.md §3-6, §13) ─
;; A chain line is two slots: the trigger's selector terms (ANDed), then the
;; stages and actions. Stage args are keyword-labelled and pre-filled; a
;; number arg also takes a nested source, (acc :by (coin :p 0.5) …).

(def proc-trigger-schema
  (cons "forms" (cons (cons "word" (cons "any" sel-words)) (rest (rest sel-schema)))))

;; a number, or (depth > 0) a nested source whose args nest one level less
(def pval (lo hi step dec def depth)
  (if (<= depth 0)
    (pnum lo hi step dec def)
    (cons "or" (cons (pnum lo hi step dec def) (source-forms (- depth 1))))))

(def source-forms (depth)
  (list (list "form" "acc" (pkw ":by") (pval -16 16 0.05 2 1 depth)
                           (pkw ":min") (pval -48 48 1 0 0 depth)
                           (pkw ":max") (pval -48 48 1 0 8 depth))
        (list "form" "coin" (pkw ":p") (pval 0 1 0.05 2 0.5 depth))
        (list "form" "rand" (pkw ":min") (pval -48 48 1 0 0 depth)
                            (pkw ":max") (pval -48 48 1 0 12 depth))
        (list "form" "count" (pkw ":n") (pval 1 64 1 0 4 depth))
        (list "form" "cyc" (list "rest" (pnum -48 48 1 0 0)))))

(def proc-then-schema
  (list "or" (pword (list "rest" "stac"))
        (list "form" "vel*" (num-schema "vel*"))
        (list "form" "vel+" (num-schema "vel+"))
        (list "form" "note+" (num-schema "note+"))
        (list "form" "note" (num-schema "note"))
        (list "form" "gate" (num-schema "gate"))))

;; only words that keep the figure's length (spec §6)
(def proc-next-schema
  (list "or" (pword (list "rest" "rev" "swap" "ghost" "stac" "half"))
        (list "form" "fast" (pnum 2 8 1 0 2))
        (list "form" "note+" (num-schema "note+"))
        (list "form" "vel*" (num-schema "vel*"))
        (list "form" "vel+" (num-schema "vel+"))
        (list "form" "gate" (num-schema "gate"))))

;; ── (if COND THEN [ELSE]) ──
;; a value in a condition: a number or a $ context word; the left side of a
;; comparison may also be arithmetic on those, (= (mod $n 4) 3). Kept this
;; shallow on purpose: every slot carries its whole schema, and spelling
;; arithmetic into both sides and into and/or/not made this part alone 77 KB
;; (typed conditions may nest anything; only the editor's check is shallow).
(def ctx-words (list "$1" "$n" "$vel" "$cycle" "$fig" "$rep"))
(def cond-atom (list "or" (pnum -48 48 1 0 0) (cons "word" ctx-words)))
(def cond-left
  (append cond-atom
          (map (lambda (op) (list "form" op cond-atom cond-atom)) (list "mod" "+" "-" "*"))))
(def cond-ops (list "=" "!=" "<" ">" "<=" ">="))
(def cond-compare (map (lambda (op) (list "form" op cond-left cond-atom)) cond-ops))
(def cond-plain (cons "or" (map (lambda (op) (list "form" op cond-atom cond-atom)) cond-ops)))
(def cond-schema
  (append (cons "or" cond-compare)
          (list (list "form" "and" cond-plain cond-plain)
                (list "form" "or" cond-plain cond-plain)
                (list "form" "not" cond-plain))))

;; what an if does: event words on this hit, figure words on the next
;; figure; `+` adds another, (else w…) runs when the condition fails
(def value-or-x (op) (list "or" (num-schema op) (list "word" "$1")))
(def branch-one
  (list "or" (pword (list "rest" "stac" "half" "rev" "swap" "ghost"))
        (list "form" "note+" (value-or-x "note+"))
        (list "form" "vel*" (value-or-x "vel*"))
        (list "form" "vel+" (num-schema "vel+"))
        (list "form" "note" (num-schema "note"))
        (list "form" "gate" (num-schema "gate"))
        (list "form" "fast" (pnum 2 8 1 0 2))
        (list "form" "minvel" (num-schema "minvel"))
        (list "form" "maxvel" (num-schema "maxvel"))
        (list "form" "basevel" (num-schema "basevel"))
        (list "form" "dotdecay" (num-schema "dotdecay"))
        (list "form" "dashdecay" (num-schema "dashdecay"))
        scale-form))
;; (for N w…): figure words for the next N figures
(def for-form (list "form" "for" (pnum 1 16 1 0 2) (list "rest" branch-one)))
(def branch-schema
  (append branch-one (list for-form (list "form" "else" (list "rest" (append branch-one (list for-form)))))))
(def if-form (list "form" "if" cond-schema (list "rest" branch-schema)))

(def proc-body-schema
  (append
    (list "forms"
          ;; transforms on $1, held targets $1 drives, and figure words (on
          ;; the next figure)
          (list "word" "sin" "abs" "gate*" "dashdecay*" "dotdecay*" "basevel*"
                "half" "rev" "swap" "ghost"))
    ;; one level of nesting in the editor, (acc :by (coin :p 0.5)): every
    ;; level multiplies the schema, and each slot carries all of it (typed
    ;; code nests deeper; only the editor's check stops at one)
    (source-forms 1)
    (list (list "form" "remap" (pkw ":in-lo") (pval -48 48 0.1 2 -1 1) (pkw ":in-hi") (pval -48 48 0.1 2 1 1)
                               (pkw ":out-lo") (pval -48 48 0.1 2 0 1) (pkw ":out-hi") (pval -48 48 0.1 2 1 1))
          (list "form" "clamp" (pkw ":min") (pval -48 48 1 0 0 1) (pkw ":max") (pval -48 48 1 0 12 1))
          (list "form" "cmp" (pkw ":op") (pword (list ">=" ">" "<" "<=" "=")) (pkw ":to") (pval -48 48 0.5 1 1 1))
          (list "form" "quant" (pkw ":step") (pval 0 12 1 0 1 1))
          (list "form" "pow" (pkw ":e") (pval 0 8 0.5 1 2 1))
          scale-form
          harmony-form
          ;; (note+ $1) / (vel* $1) hold the stages' value on every hit;
          ;; with a number the word acts on the hit when $1 >= 0.5:
          ;; (coin :p 0.5) (note+ -12)
          (list "word" "rest" "stac")
          (list "form" "note+" (value-or-x "note+"))
          (list "form" "vel*" (value-or-x "vel*"))
          (list "form" "fast" (pnum 2 8 1 0 2))
          for-form
          if-form
          (list "form" "vel+" (num-schema "vel+"))
          (list "form" "note" (num-schema "note"))
          (list "form" "gate" (num-schema "gate"))
          (list "form" "then" (list "rest" proc-then-schema))
          (list "form" "next" (list "rest" proc-next-schema)))))

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
;; ("plock" "instrument:cutoff" v) keeps its NAME a string: it is a param
;; label, not a word (docs/jaki-plock-spec.md §4).
(def route-datum (x)
  (if (string? x)
    ;; "" is a dyn word nobody picked yet: `(harmony :track "")` reads 0
    (if (= x "") 0 (read-string x))
    (if (number? x)
      x
      (if (= (first x) "plock")
        (cons 'plock (cons (nth x 1) (map route-datum (rest (rest x)))))
        (map route-datum x)))))

(def mod-form (m) (route-datum (mod-item m)))

;; ── row processes (docs/jaki-row-processes-spec.md §9) ─────────────────────

(def row-procs (row) (let ((ps (get row :procs))) (if (= ps nil) (list) ps)))

;; ("chan" "name" 0) keeps its channel name a string, like plock's label
(def proc-datum (x)
  (if (and (not (string? x)) (not (number? x)) (not (empty? x)) (= (first x) "chan"))
    (cons 'chan (cons (nth x 1) (map route-datum (rest (rest x)))))
    (route-datum x)))

(def proc-word (chain) (cons 'rule (map proc-datum chain)))

;; a fresh rule: every hit, nothing yet (a rule without an action is
;; skipped until it has one)
(def new-chain (list "any"))

;; The trigger as the trigger slot's items: ("and" a b) shows as a b, so
;; `dash head` reads as two terms. An edit back: one item is the trigger, two
;; or more are ANDed, none is `any`.
(def chain-trigger-items (chain)
  (let ((t (first chain)))
    (if (and (not (string? t)) (not (number? t)) (not (empty? t)) (= (first t) "and"))
      (rest t)
      (list t))))

(def chain-with-trigger (chain items)
  (cons (if (empty? items) "any" (if (= (len items) 1) (first items) (cons "and" items)))
        (rest chain)))

;; ── rows ────────────────────────────────────────────────────────────────────

(def default-row (dict :route -1 :mods (list)))

(def row-at (rows i)
  (if (< i (len rows)) (nth rows i) default-row))

(def row-live? (row)
  (let ((route (get row :route)))
    (and (number? route) (>= route 0))))

(def row-segment (row)
  (cons '-> (cons (get row :route)
                  (append (map mod-form (get row :mods))
                          (map proc-word (row-procs row))
                          (let ((seed (get row :seed)))
                            (if (and (number? seed) (>= seed 0)) (list (list 'seed seed)) (list)))))))

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

;; ── patterns ────────────────────────────────────────────────────────────────

(def new-pattern (dict :figures (list) :rows (list) :row-count 4))

;; Every pattern, the first from the document's own fields.
(def pattern-list (figures rows row-count patterns)
  (cons (dict :figures figures :rows rows :row-count row-count)
        (if (= patterns nil) (list) patterns)))

(def pattern-body (pat)
  (body (get pat :figures) (get pat :rows) (get pat :row-count)))

(def pattern-sounds? (pat) (not (= (pattern-body pat) nil)))

;; The patterns that can sound, as one jaki body: one pattern keeps the
;; single-voice shape, several are voice lines, in pattern order — so the
;; routes (and their marks) number through the patterns in order.
(def patterns-body (pats)
  (let ((bodies (filter (lambda (b) (not (= b nil))) (map pattern-body pats))))
    (if (empty? bodies)
      nil
      (if (= (len bodies) 1) (first bodies) bodies))))

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

(def memo-body (figures rows row-count patterns)
  (let ((key (list figures rows row-count patterns)))
    (let ((hit (body-memo-find body-memo key)))
      (if (empty? hit)
        (let ((b (patterns-body (pattern-list figures rows row-count patterns))))
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
(def tick-patterns (figures rows row-count mode patterns)
  (do
    (alez.jaki.core/init :16)
    (let ((playing (alez.jaki.core/step-clock (mode-of mode))))
      (do
        (gen-mark (if playing (+ (alez.jaki.core/play-pos) 1) 0))
        (let ((b (memo-body figures rows row-count patterns)))
          (if (and b playing) (alez.jaki.core/play-routes b) 0))))))

;; one pattern: the tick before patterns
(def tick (figures rows row-count mode)
  (tick-patterns figures rows row-count mode (list)))
