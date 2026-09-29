;; The jaki kind's document and its tick (docs/jaki-kind-spec.md §5).
;;
;; A `jaki` instance's document is plain data (scene slots, so literals only):
;;
;;   figures   ((:dot :dot :dash) (:dot :dash))      one list per figure
;;   rows      ((dict :route 0 :mods ((dict :op "trunc" :args (3)) …)) …)
;;   row-count 8                                      rows that show and play
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

(export tick body row-at row-live? default-row mod-form figure-label
        figure-options figure-option-labels zero-mods arg-mods mod-labels
        mod-arg-spec mod-choices mod-default-args every-words)

;; ── figures ─────────────────────────────────────────────────────────────────

(def event-sym (kw) (if (= kw :dash) '- '.))
(def event-text (kw) (if (= kw :dash) "-" "."))

(def join-words (words)
  (reduce (lambda (acc w) (if (= acc "") w (str acc " " w))) "" words))

(def figure-label (figure) (join-words (map event-text figure)))

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
(def zero-mods (list "left" "right" "accent" "rev" "stac" "ghost" "swap"))
;; Words that take a number and/or a choice, in menu order. `every n w`, `L w`
;; and `R w` wrap a zero-arg word; `split`/`merge` pick which dash / dot pair.
(def arg-mods (list "trunc" "rot" "shift" "fast" "slow" "gate" "vel" "note"
                    "basevel" "dotdecay" "dashdecay" "minvel" "maxvel"
                    "every" "L" "R" "split" "merge"))
(def mod-labels (append zero-mods arg-mods))
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
    "basevel"   (list 0 1 0.01 2 0.8)
    "dotdecay"  (list 0 1 0.01 2 0.85)
    "dashdecay" (list 0 1 0.01 2 0.9)
    "minvel"    (list 0 1 0.01 2 0.3)
    "maxvel"    (list 0 1 0.01 2 1.0)
    "every" (list 1 16 1 0 4)
    _ nil))

;; The options of a word's choice argument (after its number, if any), or nil.
(def mod-choices (op)
  (match op
    "every" every-words
    "L" every-words
    "R" every-words
    "split" split-targets
    "merge" split-targets
    _ nil))

;; A fresh modifier's args: its number default, then its first choice.
(def mod-default-args (op)
  (let ((spec (mod-arg-spec op)) (choices (mod-choices op)))
    (append (if spec (list (nth spec 4)) (list))
            (if choices (list (if (= op "every") "rev" (nth choices 0))) (list)))))

(def op-sym (op)
  (match op
    "left" 'left  "right" 'right  "accent" 'accent  "rev" 'rev
    "stac" 'stac  "ghost" 'ghost  "swap" 'swap
    "trunc" 'trunc  "rot" 'rot  "shift" 'shift  "fast" 'fast  "slow" 'slow
    "gate" 'gate  "vel" 'vel  "note" 'note  "every" 'every
    "basevel" 'basevel  "dotdecay" 'dotdecay  "dashdecay" 'dashdecay
    "minvel" 'minvel  "maxvel" 'maxvel
    "L" 'L  "R" 'R  "split" 'split  "merge" 'merge
    "first" 'first  "last" 'last  "all" 'all
    _ nil))

;; One modifier record -> its route word: `left`, `(trunc 3)`, `(every 4 rev)`,
;; `(split last)`, `(L stac)`. Choice args are words, numbers stay numbers.
(def mod-form (m)
  (let ((op (get m :op)) (args (get m :args)))
    (if (empty? args)
      (op-sym op)
      (cons (op-sym op)
            (map (lambda (a) (if (string? a) (op-sym a) a)) args)))))

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

;; The jaki body, or nil when nothing can sound (no figure, or no live row:
;; a body without routes would play track 0).
(def body (figures rows row-count)
  (let ((live (filter row-live?
                (map (lambda (i) (row-at rows i)) (range 0 row-count)))))
    (if (or (empty? figures) (empty? live))
      nil
      (append
        (map (lambda (f) (list 'fig (map event-sym f))) figures)
        (reduce (lambda (acc row) (append acc (row-segment row))) (list) live)))))

;; The kind's generator tick: `self` is the instance's document map.
(def tick (figures rows row-count)
  (do
    (alez.jaki.core/init :16)
    (let ((b (body figures rows row-count)))
      (if b (alez.jaki.core/run b) 0))))
