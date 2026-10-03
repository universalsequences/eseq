;; Jaki sequencer library — pure-Lisp evaluator core, pattern surface, and
;; generator wiring (docs/jaki-sequencer-spec.md §11 phases 1-3, bead eseq-5k5).
;;
;; Patterns use the variadic `alez.jaki.core/pat` macro:
;;
;;   (alez.jaki.core/pat . . - (every 2 swap) (* (cyc 1 2)))
;;   (alez.jaki.core/pat (fig (. . -) (* 2)) (fig (. -) (/ 3)))
;;
;; The tick body that builds patterns is authored on the UI VM but runs on the
;; scheduler VM: def-sequencer expands `pat` on the authoring side before it
;; auto-quotes and serializes the residue. The scheduler only runs the resulting
;; `from-list` call and never needs the macro layer.
;;
;; Offsets and gates are exact rationals — normalized (numerator denominator)
;; 2-lists — so tuplet window membership never needs an epsilon (spec §8.3).
;; One unit = one tick of the generator's :resolution grid.
;;
;; Evaluated event fields (dicts): :off rational, :sym :dot|:dash, :hit 1|2,
;; :hand :left|:right, :vel number, :accent bool, :fig index, :gate rational,
;; :afig / :arep authored figure and repetition (0-based; spec §7.1), and the
;; optional note adjustments :nadd / :nset that `emit` applies.
;; `alez.jaki.core/eval-at` returns (dict :events ... :len rational :end-hand hand
;; :end-st velocity-state); velocity state is (dict :cur :pwd :streak).

(module alez.jaki.core)

;; declared harmony: `-> chords` rows, (deg n), (chord Q), (voice)
(import alez.jaki.chords)

(export pat from-list xform rev rot trunc every every-fig stac ghost swap
        shift filter for-hand fast slow on preview
        eval-at eval-cycle cycle-length locate cycle-index
        default-state mk-state
        init emit emit* reset
        run run-in step-clock play-routes play-pos
        prepare hash-u preview-procs)

;; ── exact rationals: normalized (num den) 2-lists, den > 0 ──────────────────

(def gcd* (a b) (if (= b 0) a (gcd* b (mod a b))))

;; exact integer floor division (float floor corrected at the boundary)
(def idiv (a b)
  (let ((q (floor (/ a b))))
    (if (> (* q b) a)
        (- q 1)
        (if (<= (* (+ q 1) b) a) (+ q 1) q))))

(def imod (a n) (mod (+ (mod a n) n) n))

(def rat (n d)
  (let ((s (if (< d 0) -1 1)))
    (let ((n2 (* n s)) (d2 (* d s)))
      (let ((g (gcd* (abs n2) d2)))
        (if (= g 0) (list 0 1) (list (/ n2 g) (/ d2 g)))))))

(def r-int (n) (list n 1))
(def r-num (r) (nth r 0))
(def r-den (r) (nth r 1))
(def r+ (a b)
  (rat (+ (* (r-num a) (r-den b)) (* (r-num b) (r-den a)))
       (* (r-den a) (r-den b))))
(def r- (a b) (r+ a (list (* -1 (r-num b)) (r-den b))))
(def r* (a b) (rat (* (r-num a) (r-num b)) (* (r-den a) (r-den b))))
(def r-div (a b) (rat (* (r-num a) (r-den b)) (* (r-den a) (r-num b))))
(def r< (a b) (< (* (r-num a) (r-den b)) (* (r-num b) (r-den a))))
(def r<= (a b) (<= (* (r-num a) (r-den b)) (* (r-num b) (r-den a))))
(def r-min (a b) (if (r< a b) a b))
(def r->f (r) (/ (r-num r) (r-den r)))
(def r-ceil (r)
  (let ((q (idiv (r-num r) (r-den r))))
    (if (= (* q (r-den r)) (r-num r)) q (+ q 1))))
(def iceil-div (a b) (let ((q (idiv a b))) (if (= (* q b) a) q (+ q 1))))
(def lcm* (a b) (let ((g (gcd* a b))) (if (= g 0) 1 (/ (* a b) g))))

;; floor-mod for rationals, m > 0
(def r-mod (a m)
  (let ((q (idiv (* (r-num a) (r-den m)) (* (r-den a) (r-num m)))))
    (r- a (r* (r-int q) m))))

;; ── small list helpers (recursion is the loop construct here) ───────────────

(def take* (n l)
  (if (or (<= n 0) (empty? l)) (list) (cons (first l) (take* (- n 1) (rest l)))))
(def drop* (n l) (if (or (<= n 0) (empty? l)) l (drop* (- n 1) (rest l))))
(def last* (l) (if (empty? (rest l)) (first l) (last* (rest l))))
(def repeat* (x n) (if (<= n 0) (list) (cons x (repeat* x (- n 1)))))
(def keep (f l)
  (reduce (lambda (acc x) (if (f x) (append acc (list x)) acc)) (list) l))
(def member? (x l) (reduce (lambda (acc i) (or acc (= i x))) false l))
(def sum* (l) (reduce (lambda (a b) (+ a b)) 0 l))

;; stable sort by rational :off — a natural merge sort. Lists are vectors:
;; `rest` and `cons` copy, so the old recursive insertion sort cost ~n³
;; element copies and n² interpreted compares, worst on the usual input (an
;; already sorted cycle) — 25+ ms per sort of a 128-hit (fast 8) cycle on
;; the scheduler thread (eseq-8cim). Here a sorted list costs one O(n) scan,
;; and k ascending runs cost O(n log k) compares. Ties keep input order.
(def ev< (a b) (r< (get a :off) (get b :off)))

;; (lo hi) bounds of the maximal non-decreasing runs, in order
(def srt-runs (l i n lo acc)
  (if (>= i n)
      (reverse (cons (list lo n) acc))
      (if (ev< (nth l i) (nth l (- i 1)))
          (srt-runs l (+ i 1) n i (cons (list lo i) acc))
          (srt-runs l (+ i 1) n lo acc))))

(def srt-slice (l b) (map (lambda (i) (nth l i)) (range (nth b 0) (nth b 1))))

;; stable merge of sorted a and b: on equal :off, a's events go first
(def srt-merge (a i na b j nb acc)
  (if (>= i na)
      (append (reverse acc) (map (lambda (k) (nth b k)) (range j nb)))
      (if (>= j nb)
          (append (reverse acc) (map (lambda (k) (nth a k)) (range i na)))
          (if (ev< (nth b j) (nth a i))
              (srt-merge a i na b (+ j 1) nb (cons (nth b j) acc))
              (srt-merge a (+ i 1) na b j nb (cons (nth a i) acc))))))

(def srt-pairs (runs i n acc)
  (if (>= i n)
      (reverse acc)
      (if (= (+ i 1) n)
          (reverse (cons (nth runs i) acc))
          (let ((a (nth runs i)) (b (nth runs (+ i 1))))
            (srt-pairs runs (+ i 2) n
                       (cons (srt-merge a 0 (len a) b 0 (len b) (list)) acc))))))

(def srt-all (runs)
  (if (<= (len runs) 1) (first runs) (srt-all (srt-pairs runs 0 (len runs) (list)))))

(def sort-evs (l)
  (let ((n (len l)))
    (if (<= n 1)
        l
        (let ((bounds (srt-runs l 1 n 0 (list))))
          (if (= (len bounds) 1)
              l
              (srt-all (map (lambda (b) (srt-slice l b)) bounds)))))))

;; ── pattern parsing: quoted body data → figure records ──────────────────────

;; head of a raw item: the item itself for symbols, the first element for lists
(def raw-head (x) (let ((h (nth x 0))) (if (= h nil) x h)))
(def raw-args (x) (let ((h (nth x 0))) (if (= h nil) (list) (rest x))))

(def head-kw (s)
  (match s
    'rev :rev  'rot :rot  'trunc :trunc  'every :every
    'stac :stac  'ghost :ghost  'swap :swap
    'split :split  'merge :merge
    'basevel :basevel  'dotdecay :dotdecay  'dashdecay :dashdecay
    'minvel :minvel  'maxvel :maxvel
    'L :L  'R :R
    'half :half
    'id :id
    _ nil))

(def norm-target (t) (match t 'first :first 'last :last 'all :all _ t))

;; normalize a transform (symbol or list) to a keyword-headed list; unknown → nil
;; ── word alternation: (right right right left) = one word per cycle ────────
;; A LIST in word position whose head is a zero-arg word (or itself a list)
;; can never be a parameterized word, so it reads as a per-cycle alternation
;; over its elements — the modifier twin of implicit cyc. (cyc w1 w2 …) is
;; the explicit spelling. `id` is the no-op member: (stac stac id).

(def zero-word? (w)
  (member? w '(left right accent rev stac ghost swap half id none rest)))

(def alt-word? (w)
  (if (= (nth w 0) nil)
      false
      (let ((h (raw-head w)))
        (or (= h 'cyc)
            (or (not (= (nth h 0) nil))
                (and (zero-word? h) (> (len w) 1)))))))

(def alt-members (w) (if (= (raw-head w) 'cyc) (rest w) w))

(def alt-pick (ws cycle) (nth ws (imod cycle (max 1 (len ws)))))

(def norm-xf (f)
  (if (alt-word? f)
      (let ((xs (map norm-xf (alt-members f))))
        (if (member? nil xs) nil (cons :alt xs)))
      (let ((h (head-kw (raw-head f))) (args (raw-args f)))
        (match h
          :every (list :every (nth args 0) (norm-xf (nth args 1)))
          :L (list :L (norm-xf (nth args 0)))
          :R (list :R (norm-xf (nth args 0)))
          :split (list :split (norm-target (nth args 0)))
          :merge (list :merge (norm-target (nth args 0)))
          _ (if (= h nil) nil (cons h args))))))

(def event-sym? (x) (or (= x '.) (= x '-)))
(def event-kw (x) (if (= x '-) :dash :dot))

(def add-fig-events (acc evs)
  (merge acc :events (append (get acc :events) evs)))

(def parse-fig (items)
  (reduce
    (lambda (acc it)
      (let ((h (raw-head it)))
        (if (event-sym? it)
            (add-fig-events acc (list (event-kw it)))
            (if (event-sym? h)
                ;; a (. . -) group — the fig form's event list
                (add-fig-events acc (map event-kw (keep event-sym? it)))
                (if (or (= h '*) (= h '/) (= h '%))
                    (merge acc :tm (list (match h '* :fast '/ :slow _ :fit)
                                         (nth (raw-args it) 0)))
                    (if (= h 'align)
                        (merge acc :align (list (nth it 1) (= (nth it 2) :pad)))
                    (if (= h 'rep)
                        (merge acc :rep (max 1 (round-int (nth it 1))))
                        (let ((x (norm-xf it)))
                          (if (= x nil)
                              acc
                              (merge acc :xf (append (get acc :xf) (list x))))))))))))
    (dict :events (list) :xf (list) :tm nil :align nil :rep 1)
    items))

(def fig-form? (x) (= (raw-head x) 'fig))

;; :id keys the full-evaluation memo; :len-id keys the cycle-length memos and
;; only changes with the FIGURES (xform/retime), never with post ops — every
;; route derived from one pattern by filters/shift/quant/gate shares one
;; length table instead of rebuilding pat-lens per route.
;; (fig (. -) (rep 4)) is four consecutive copies of the figure — four
;; figures in every respect (hands and velocity thread through them, each has
;; its own index for (every-fig …) and :figure filters) — not (* 4), which
;; squeezes the figure into its own length.
;; Each copy is stamped with its authored figure :afig and repetition :arep
;; (0-based) for the (fig n) / (rep n) selectors of `on` (spec §7.1).
(def expand-reps (figs)
  (reduce (lambda (acc a)
            (let ((f (nth figs a)))
              (append acc (map (lambda (k) (merge f :afig a :arep k))
                               (range 0 (get f :rep))))))
          (list) (range 0 (len figs))))

(def from-list (body)
  (dict :id (source body)
        :len-id (source body)
        :figs (expand-reps
                (if (fig-form? (first body))
                    (map (lambda (f) (parse-fig (rest f))) body)
                    (list (parse-fig body))))
        :post (list)))

(defmacro pat (&rest body)
  `(alez.jaki.core/from-list '(,@body)))

;; ── whole-pattern transform functions (spec §4.6) ───────────────────────────

;; append a transform (quoted data, e.g. '(every 2 rev)) to every figure
(def xform (p t)
  (merge p
    :figs (map (lambda (f) (merge f :xf (append (get f :xf) (list (norm-xf t)))))
               (get p :figs))
    :id (str (get p :id) "|xf:" (source t))
    :len-id (str (get p :len-id) "|xf:" (source t))))

;; retime every figure: (fast p 2), (slow p (cyc 1 2)) — n is raw per-cycle
;; argument data, same as a figure's own (* n)/(/ n) time-mod
(def retime (p mode n)
  (merge p
    :figs (map (lambda (f) (merge f :tm (list mode n))) (get p :figs))
    :id (str (get p :id) "|tm:" (source (list mode n)))
    :len-id (str (get p :len-id) "|tm:" (source (list mode n)))))
;; A figure's :tm is nil, (mode n), or conditional — (:when kind n index on
;; off): `on` (a (mode n)) while the gate is open, else `off` (any :tm). kind
;; :every gates on the cycle, :fig on the figure's index. Written by
;; (every n (fast m)) / (every-fig n (slow m)).
(def when-active? (tm cycle)
  (match (nth tm 1)
    :every (every-active? (round-int (resolve-arg (nth tm 2) cycle)) cycle)
    :on (sel-ok? (nth tm 2) (nth tm 3) cycle)
    _ (fig-index-active? (nth tm 2) (nth tm 3) cycle)))

(def tm-at (tm cycle)
  (if (and (not (= tm nil)) (= (first tm) :when))
      (if (when-active? tm cycle) (nth tm 4) (tm-at (nth tm 5) cycle))
      tm))

(def when-retime (p kind n mode m)
  (let ((figs (get p :figs)) (tag (str "|when:" (source (list kind n mode m)))))
    (merge p
      :figs (map (lambda (i)
                   (let ((f (nth figs i)))
                     (merge f :tm (list :when kind n i (list mode m) (get f :tm)))))
                 (range 0 (len figs)))
      :id (str (get p :id) tag)
      :len-id (str (get p :len-id) tag))))

(def fast (p n) (retime p :fast n))
(def slow (p n) (retime p :slow n))

;; per-figure gate: t applies to figure i of the cycle when (i + 1) mod n = 0
;; — (every-fig 2 rev) reverses figures 1, 3, 5 …, the same convention as
;; (every n …) over cycles. n is per-cycle argument data. Each figure's xf
;; carries its own index: (:fig-every n xf index).
(def every-fig (p n t)
  (let ((x (norm-xf t)) (figs (get p :figs)))
    (if (= x nil)
        p
        (merge p
          :figs (map (lambda (i)
                       (let ((f (nth figs i)))
                         (merge f :xf (append (get f :xf) (list (list :fig-every n x i))))))
                     (range 0 (len figs)))
          :id (str (get p :id) "|fe:" (source n) ":" (source t))
          :len-id (str (get p :len-id) "|fe:" (source n) ":" (source t))))))

(def fig-every-active? (f cycle)
  (let ((n (round-int (resolve-arg (nth f 1) cycle))))
    (and (> n 0) (= 0 (imod (+ (nth f 3) 1) n)))))

;; ── selectors: (on SEL word…) scopes words (spec §7.1) ─────────────────────
;; A selector is normalized to a keyword-headed list and tested against a
;; context dict: an evaluated event, or a figure context (dict :fig :afig
;; :arep) for figure-level scoping. Event terms (hand, symbol, hit, accent)
;; hold for any figure context — a figure-level test only rules on figure
;; terms.

(def sel-atom (w)
  (match w
    'left (list :hand :left)  'right (list :hand :right)
    'dot (list :sym :dot)  'dash (list :sym :dash)
    'head (list :hit 1)  'tail (list :hit 2)
    'accent (list :accent)
    'any (list :any)
    _ nil))

(def fig-ctx (figs i)
  (let ((f (nth figs i)))
    (dict :fig i :afig (get f :afig) :arep (get f :arep))))

(def norm-sel-all (kind raws figs)
  (let ((xs (map (lambda (r) (norm-sel r figs)) raws)))
    (if (or (empty? xs) (member? nil xs)) nil (cons kind xs))))

;; raw selector data → normalized selector, or nil when it is not one. `figs`
;; (the pattern's expanded figures) lets (nth n S) precompute the figures S
;; picks: S is static, so its per-cycle count K is a constant.
(def norm-sel (raw figs)
  (if (= (nth raw 0) nil)
      (sel-atom raw)
      (let ((h (raw-head raw)) (args (raw-args raw)))
        (match h
          'fig (list :fig (nth args 0))
          'rep (list :rep (nth args 0))
          'every (list :every (nth args 0))
          'not (let ((s (norm-sel (nth args 0) figs))) (if (= s nil) nil (list :not s)))
          'and (norm-sel-all :and args figs)
          'or (norm-sel-all :or args figs)
          'cyc (norm-sel-all :alt args figs)
          'nth (let ((s (if (> (len args) 1) (norm-sel (nth args 1) figs) (list :any))))
                 (if (= s nil)
                     nil
                     (list :nth (nth args 0)
                           (keep (lambda (i) (sel-ok? s (fig-ctx figs i) 0))
                                 (range 0 (len figs))))))
          ;; a list of selectors is one per cycle, like word alternation
          _ (if (or (not (= (sel-atom h) nil)) (not (= (nth h 0) nil)))
                (norm-sel-all :alt raw figs)
                nil)))))

;; event field test; a figure context (no :sym) passes every event term
(def ctx-field? (ctx field want)
  (if (= (get ctx :sym) nil) true (= (get ctx field) want)))

(def index-of* (xs x i)
  (if (empty? xs) -1 (if (= (first xs) x) i (index-of* (rest xs) x (+ i 1)))))

;; (nth n S): occurrence c·K + k of the K figures S picks per cycle
(def nth-active? (s fig cycle)
  (let ((m (nth s 2)) (n (round-int (resolve-arg (nth s 1) cycle))))
    (let ((k (index-of* m fig 0)))
      (and (> n 0) (>= k 0)
           (= 0 (imod (+ (* cycle (len m)) k 1) n))))))

(def sel-arg-index (raw cycle) (- (round-int (resolve-arg raw cycle)) 1))

(def sel-ok? (s ctx cycle)
  (match (first s)
    :any true
    :hand (ctx-field? ctx :hand (nth s 1))
    :sym (ctx-field? ctx :sym (nth s 1))
    :hit (ctx-field? ctx :hit (nth s 1))
    :accent (ctx-field? ctx :accent true)
    :fig (= (get ctx :afig) (sel-arg-index (nth s 1) cycle))
    :rep (= (get ctx :arep) (sel-arg-index (nth s 1) cycle))
    :every (every-active? (round-int (resolve-arg (nth s 1) cycle)) cycle)
    :fig-every (fig-index-active? (nth s 1) (get ctx :fig) cycle)
    :nth (nth-active? s (get ctx :fig) cycle)
    :not (not (sel-ok? (nth s 1) ctx cycle))
    :and (reduce (lambda (acc x) (and acc (sel-ok? x ctx cycle))) true (rest s))
    :or (reduce (lambda (acc x) (or acc (sel-ok? x ctx cycle))) false (rest s))
    :alt (sel-ok? (alt-pick (rest s) cycle) ctx cycle)
    _ false))

;; true when the selector reads an event tag, so it cannot pick whole figures
(def sel-event? (s)
  (match (first s)
    :hand true  :sym true  :hit true  :accent true
    :not (sel-event? (nth s 1))
    :and (member? true (map sel-event? (rest s)))
    :or (member? true (map sel-event? (rest s)))
    :alt (member? true (map sel-event? (rest s)))
    _ false))

;; figure-level scoping: every figure carries the word gated by the selector
;; and its own context — (:on sel xf ctx) in :xf, (:when :on sel ctx on off)
;; in :tm
(def on-figs (p tag update)
  (let ((figs (get p :figs)))
    (merge p
      :figs (map (lambda (i) (update (nth figs i) (fig-ctx figs i)))
                 (range 0 (len figs)))
      :id (str (get p :id) tag)
      :len-id (str (get p :len-id) tag))))

(def on-xf (p sel x tag)
  (on-figs p tag
    (lambda (f ctx) (merge f :xf (append (get f :xf) (list (list :on sel x ctx)))))))

(def on-retime (p sel mode m tag)
  (on-figs p tag
    (lambda (f ctx) (merge f :tm (list :when :on sel ctx (list mode m) (get f :tm))))))

(def rev (p) (xform p 'rev))
(def rot (p n) (xform p (list 'rot n)))
;; (trunc n) cuts n symbols off the end of the WHOLE cycle — the figures
;; concatenated, after each figure's own transforms — working back through
;; the figures: (. -) (. . -) trunc 1 → (. -) (. .), trunc 3 → (. -).
;; Entries are (:all n) or (:every k n) (from (every k (trunc n))); their
;; cuts add up. A (trunc n) inside a fig form, or (every-fig k (trunc n)),
;; still cuts that figure alone.
(def add-gtrunc (p entry)
  (merge p :gtrunc (append (or-default (get p :gtrunc) (list)) (list entry))
           :id (str (get p :id) "|gt:" (source entry))
           :len-id (str (get p :len-id) "|gt:" (source entry))))

(def trunc (p n) (add-gtrunc p (list :all n)))

(def gtrunc-total (p cycle)
  (reduce (lambda (a e)
            (+ a (if (or (= (first e) :all)
                         (every-active? (round-int (resolve-arg (nth e 1) cycle)) cycle))
                     (max 0 (round-int (resolve-arg (last* e) cycle)))
                     0)))
          0 (or-default (get p :gtrunc) (list))))

(def cut-walk (lens-rev rem)
  (if (empty? lens-rev)
      (list)
      (let ((c (min rem (first lens-rev))))
        (cons c (cut-walk (rest lens-rev) (- rem c))))))

;; the cycle's figures, each stamped with :cut — how many of its trailing
;; (transformed) symbols the global trunc removes this cycle
(def cut-figs (p cycle)
  (let ((figs (get p :figs)) (total (gtrunc-total p cycle)))
    (if (<= total 0)
        figs
        (let ((cuts (reverse (cut-walk
                               (reverse (map (lambda (f)
                                               (len (apply-xf-events (get f :events)
                                                                     (get f :xf) cycle)))
                                             figs))
                               total))))
          (map (lambda (i) (merge (nth figs i) :cut (nth cuts i))) (range 0 (len figs)))))))

(def cut-tail (evs fig)
  (let ((k (or-default (get fig :cut) 0)))
    (if (> k 0) (take* (max 0 (- (len evs) k)) evs) evs)))
(def every (p n t) (xform p (list 'every n t)))
(def stac (p) (xform p 'stac))
(def ghost (p) (xform p 'ghost))
(def swap (p) (xform p 'swap))

;; A per-hit word whose argument needs a hit clock (spec §7.2) lowers to
;; (:defer kind raw nil); add-post keys it with the pattern id it creates, so
;; every such word has its own counters.
(def key-defers (op key)
  (match (first op)
    :defer (if (= (nth op 1) :plock)
               ;; the label rides along (apply-defer needs it) and is folded
               ;; into the key, so two plocks keep their own seq counters
               (list :defer :plock (nth op 2) (str key "#" (nth op 4)) (nth op 4))
               (list :defer (nth op 1) (nth op 2) key))
    :every (list :every (nth op 1) (key-defers (nth op 2) key))
    :fig-every (list :fig-every (nth op 1) (key-defers (nth op 2) key))
    :on (list :on (nth op 1) (key-defers (nth op 2) key))
    :alt (cons :alt (map (lambda (x) (key-defers x key)) (rest op)))
    _ op))

(def add-post (p op tag)
  (let ((id (str (get p :id) "|" tag)))
    (merge p :post (append (get p :post) (list (key-defers op id)))
             :id id)))

;; rotate right by n units (post-evaluation phase shift)
(def shift (p n) (add-post p (list :shift n) (str "shift:" (source n))))

;; keyed filter over the evaluated events, e.g. (alez.jaki.core/filter p '(:hand :left))
(def filter (p spec) (add-post p (list :filter spec) (str "filter:" (source spec))))

;; hand-scoped transform, e.g. (alez.jaki.core/for-hand p :left '(stac))
(def for-hand (p hand t)
  (add-post p (list :for-hand hand (norm-xf t))
            (str "fh:" (source hand) ":" (source t))))

;; ── per-cycle argument resolution ((cyc ...), (chan ...), expressions) ──────

;; Tidal-style implicit cyc: a list whose head is a VALUE — a number, a
;; string, or a nested list — is never a callable form, so it reads as
;; (cyc ...) over all its elements, recursively: (1 2 (1 3)) means
;; (cyc 1 2 (cyc 1 3)), and (group ("Drums" "Synths")) rotates group names.
(def implicit-cyc? (raw)
  (let ((h (nth raw 0)))
    (if (= h nil)
        false
        (or (number? h) (or (string? h) (not (= (nth h 0) nil)))))))

(def resolve-arg (raw cycle) (resolve-arg-at raw cycle nil))

;; figure `fig` is picked by (every-fig n …) when (fig + 1) mod n = 0; no
;; figure (nil) is never picked
(def fig-index-active? (nraw fig cycle)
  (if (number? fig)
      (let ((n (round-int (resolve-arg nraw cycle))))
        (and (> n 0) (= 0 (imod (+ fig 1) n))))
      false))

;; `fig` is the figure index the value is for, or nil. Route words gated by
;; `every` / `every-fig` store their argument as (every-gate n on off) /
;; (fig-gate n on off): `on` while the gate is open, else `off` (the value
;; the route had before).
(def resolve-arg-at (raw cycle fig)
  (if (= raw nil)
      1
      ;; literals are their own value; `eval` would compile them from source
      (if (or (number? raw) (string? raw))
          raw
          (let ((h (nth raw 0)))
            (if (= h 'at)
                (resolve-arg-at (nth raw 2) cycle fig)
            (if (or (= h 'cyc) (= h 'seq))
                ;; (seq :clock v…) outside a per-hit value reads per cycle
                ;; (spec §7.2)
                (let ((vals (if (= h 'seq) (drop* 2 raw) (rest raw))))
                  (resolve-cycle-member vals cycle fig))
                (if (= h 'chan)
                    (chan-get (nth raw 1) (nth raw 2))
                    (if (= h 'every-gate)
                        (resolve-arg-at
                          (if (every-active? (round-int (resolve-arg (nth raw 1) cycle)) cycle)
                              (nth raw 2)
                              (nth raw 3))
                          cycle fig)
                        (if (= h 'fig-gate)
                            (resolve-arg-at
                              (if (fig-index-active? (nth raw 1) fig cycle) (nth raw 2) (nth raw 3))
                              cycle fig)
                            (if (implicit-cyc? raw)
                                (resolve-cycle-member raw cycle fig)
                                (if (= h 'deg)
                                    ;; outside emit (no declared chord at
                                    ;; hand): the degree over C major
                                    (alez.jaki.chords/deg-interval
                                      :maj (resolve-arg-at (nth raw 1) cycle fig))
                                    ;; symbols and other forms evaluate as source
                                    (eval (source raw)))))))))))))

;; A per-cycle list picks member (cycle mod n), and a nested list steps once
;; per VISIT, Tidal style: in (I IV V7 (VI IV)) the inner list is visited on
;; cycles 3, 7, 11 … and plays VI, IV, VI … (it sees the visit count, not the
;; cycle, which would land on the same parity every time).
(def resolve-cycle-member (vals cycle fig)
  (let ((n (max 1 (len vals))))
    (resolve-arg-at (nth vals (imod cycle n)) (idiv cycle n) fig)))

(def round-int (x) (floor (+ x 0.5)))
(def every-active? (n cycle) (and (> n 0) (= 0 (imod (+ cycle 1) n))))

;; ── transform application over symbolic events ──────────────────────────────

(def idx-where (evs kw i)
  (if (empty? evs)
      (list)
      (if (= (first evs) kw)
          (cons i (idx-where (rest evs) kw (+ i 1)))
          (idx-where (rest evs) kw (+ i 1)))))

(def dot-pair-indices (evs i)
  (if (>= (+ i 1) (len evs))
      (list)
      (if (and (= (nth evs i) :dot) (= (nth evs (+ i 1)) :dot))
          (cons i (dot-pair-indices evs (+ i 2)))
          (dot-pair-indices evs (+ i 1)))))

(def resolve-target-idx (target indices)
  (match target
    :first (take* 1 indices)
    :last (if (empty? indices) (list) (list (last* indices)))
    :all indices
    _ (if (and (> target 0) (<= target (len indices)))
          (list (nth indices (- target 1)))
          (if (and (< target 0) (<= (* -1 target) (len indices)))
              (list (nth indices (+ (len indices) target)))
              (list)))))

(def split-events (evs target)
  (let ((tgt (resolve-target-idx target (idx-where evs :dash 0))))
    (split-walk evs 0 tgt)))
(def split-walk (evs i tgt)
  (if (>= i (len evs))
      (list)
      (if (and (member? i tgt) (= (nth evs i) :dash))
          (cons :dot (cons :dot (split-walk evs (+ i 1) tgt)))
          (cons (nth evs i) (split-walk evs (+ i 1) tgt)))))

(def merge-events (evs target)
  (let ((tgt (resolve-target-idx target (dot-pair-indices evs 0))))
    (merge-walk evs 0 tgt)))
(def merge-walk (evs i tgt)
  (if (>= i (len evs))
      (list)
      (if (and (member? i tgt)
               (< (+ i 1) (len evs))
               (= (nth evs i) :dot)
               (= (nth evs (+ i 1)) :dot))
          (cons :dash (merge-walk evs (+ i 2) tgt))
          (cons (nth evs i) (merge-walk evs (+ i 1) tgt)))))

(def apply-one-xf (evs f cycle)
  (let ((h (first f)))
    (match h
      :rev (reverse evs)
      :rot (let ((n (len evs)))
             (if (= n 0)
                 evs
                 (let ((sh (imod (round-int (resolve-arg (nth f 1) cycle)) n)))
                   (append (drop* sh evs) (take* sh evs)))))
      :trunc (take* (max 0 (- (len evs) (round-int (resolve-arg (nth f 1) cycle)))) evs)
      :every (if (every-active? (round-int (resolve-arg (nth f 1) cycle)) cycle)
                 (apply-one-xf evs (nth f 2) cycle)
                 evs)
      :fig-every (if (fig-every-active? f cycle) (apply-one-xf evs (nth f 2) cycle) evs)
      :on (if (sel-ok? (nth f 1) (nth f 3) cycle) (apply-one-xf evs (nth f 2) cycle) evs)
      :alt (apply-one-xf evs (alt-pick (rest f) cycle) cycle)
      :split (split-events evs (nth f 1))
      :merge (merge-events evs (nth f 1))
      :half (half-events evs)
      _ evs)))

(def apply-xf-events (evs xfs cycle)
  (reduce (lambda (acc f) (apply-one-xf acc f cycle)) evs xfs))

;; Halftime symbols: :dot2 / :dash2 are a dot / dash at twice the length
;; (2 and 4 units), written only by `half`. They play as dots and dashes —
;; hands, accents, velocity and the :sym of their hits are a dot's / dash's.
(def dash-sym? (e) (or (= e :dash) (= e :dash2)))
(def sym-mul (e) (if (or (= e :dot2) (= e :dash2)) 2 1))
(def sym-units (e) (* (sym-mul e) (if (dash-sym? e) 2 1)))

;; fast (* m): each dot becomes m dots, each dash (2m-2) dots + a dash — at
;; the symbol's own length, so a halftime symbol expands into halftime dots
(def expand-fast (evs m)
  (if (<= m 1)
      evs
      (reduce
        (lambda (acc e)
          (let ((dot (if (= (sym-mul e) 2) :dot2 :dot)))
            (append acc
              (if (dash-sym? e)
                  (append (repeat* dot (- (* 2 m) 2)) (list e))
                  (repeat* dot m)))))
        (list) evs)))

;; Per HIT of (expand-fast evs nf): true for the hits evs was written with,
;; false for the ones the expansion adds. A dot's first copy is the dot; a
;; dash's (2nf-2) dots + dash keep the head on the first dot and the tail on
;; the closing dash's second hit. Rules step only on true hits, so a rule's
;; own (fast n) does not run its counters (the pattern as written).
(def fast-steps (evs nf)
  (reduce (lambda (acc e)
            (append acc
              (if (dash-sym? e)
                  (append (cons true (repeat* false (- (* 2 nf) 3))) (list false true))
                  (cons true (repeat* false (- nf 1))))))
          (list) evs))

;; halftime (Liebezeit), the inverse of (* 2), dashes first: each `-` claims
;; the `. .` right before it and becomes one dash, then the dots left over in
;; each run pair up left to right into single dots — all at twice the length,
;; so the figure keeps its length and its accents (every dash still ends in a
;; dash). `. . . -` is `.` `-`(×2); `. . . . . -` is `.`(×2) `.` `-`(×2); what
;; cannot contract passes through (`. - .`). Dashes first contracts fewer
;; `. .` pairs, and only those change the symbol count's parity, so the hand
;; after the figure moves as little as possible.

;; a run of n plain dots, paired left to right
(def half-dots (n) (append (repeat* :dot2 (idiv n 2)) (repeat* :dot (imod n 2))))

(def half-walk (evs run acc)
  (if (empty? evs)
      (append acc (half-dots run))
      (let ((e (first evs)))
        (if (= e :dot)
            (half-walk (rest evs) (+ run 1) acc)
            (if (and (= e :dash) (>= run 2))
                (half-walk (rest evs) 0
                           (append (append acc (half-dots (- run 2))) (list :dash2)))
                (half-walk (rest evs) 0
                           (append (append acc (half-dots run)) (list e))))))))

(def half-events (evs) (half-walk evs 0 (list)))

(def units (evs) (reduce (lambda (a e) (+ a (sym-units e))) 0 evs))

;; boolean-flag transforms (stac/ghost/swap), honoring (every n ...) scoping
(def flag-active? (xfs kw cycle)
  (reduce
    (lambda (acc f)
      (or acc
          (let ((h (first f)))
            (if (= h kw)
                true
                (if (= h :every)
                    (and (every-active? (round-int (resolve-arg (nth f 1) cycle)) cycle)
                         (flag-active? (list (nth f 2)) kw cycle))
                    (if (= h :fig-every)
                        (and (fig-every-active? f cycle)
                             (flag-active? (list (nth f 2)) kw cycle))
                        (if (= h :on)
                            (and (sel-ok? (nth f 1) (nth f 3) cycle)
                                 (flag-active? (list (nth f 2)) kw cycle))
                            (if (= h :alt)
                                (flag-active? (list (alt-pick (rest f) cycle)) kw cycle)
                                false))))))))
    false xfs))

;; ── velocity model (Swift JakiVelocityState port) ───────────────────────────

;; dot/dash decay 0.5 and min-vel 0 (2026-09-30, the Swift port had 0.85 /
;; 0.9 / 0.3): unaccented hits fall away audibly, so accents read
(def default-params
  (dict :base 0.8 :dot-decay 0.5 :dash-decay 0.5
        :accent-boost 1.15 :min-vel 0 :max-vel 1.0))
(def default-state (dict :cur 0.8 :pwd false :streak 0))
(def mk-state (cur pwd streak) (dict :cur cur :pwd pwd :streak streak))

(def pick (b a k) (let ((v (get b k))) (if (= v nil) (get a k) v)))
(def ov-merge (a b)
  (dict :base (pick b a :base) :dot-decay (pick b a :dot-decay)
        :dash-decay (pick b a :dash-decay)
        :min-vel (pick b a :min-vel) :max-vel (pick b a :max-vel)))
(def apply-overrides (params ov)
  (dict :base (pick ov params :base)
        :dot-decay (pick ov params :dot-decay)
        :dash-decay (pick ov params :dash-decay)
        :accent-boost (get params :accent-boost)
        :min-vel (pick ov params :min-vel)
        :max-vel (pick ov params :max-vel)))

(def vel-overrides (xfs cycle)
  (reduce
    (lambda (acc f)
      (let ((h (first f)))
        (match h
          :basevel (merge acc :base (resolve-arg (nth f 1) cycle))
          :dotdecay (merge acc :dot-decay (resolve-arg (nth f 1) cycle))
          :dashdecay (merge acc :dash-decay (resolve-arg (nth f 1) cycle))
          :minvel (merge acc :min-vel (resolve-arg (nth f 1) cycle))
          :maxvel (merge acc :max-vel (resolve-arg (nth f 1) cycle))
          :every (if (every-active? (round-int (resolve-arg (nth f 1) cycle)) cycle)
                     (ov-merge acc (vel-overrides (list (nth f 2)) cycle))
                     acc)
          :fig-every (if (fig-every-active? f cycle)
                         (ov-merge acc (vel-overrides (list (nth f 2)) cycle))
                         acc)
          :on (if (sel-ok? (nth f 1) (nth f 3) cycle)
                  (ov-merge acc (vel-overrides (list (nth f 2)) cycle))
                  acc)
          :alt (ov-merge acc (vel-overrides (list (alt-pick (rest f) cycle)) cycle))
          _ acc)))
    (dict) xfs))

(def clampv (v params) (max (get params :min-vel) (min (get params :max-vel) v)))

;; one velocity-model step → (dict :vel :accent :st)
(def next-vel (st dash? second? params)
  (let ((cur (get st :cur)) (pwd (get st :pwd)) (streak (get st :streak)))
    (if second?
        ;; second dash hit: decay from the first; pwd/streak unchanged
        (let ((v (clampv (* cur (get params :dash-decay)) params)))
          (dict :vel v :accent false :st (mk-state v pwd streak)))
        (if pwd
            ;; the core Liebezeit rule: accents only after dashes
            (let ((v (clampv (* (get params :base) (get params :accent-boost)) params)))
              (dict :vel v :accent true :st (mk-state v dash? (if dash? 0 1))))
            (if dash?
                (let ((v (clampv (get params :base) params)))
                  (dict :vel v :accent false :st (mk-state v true 0)))
                (if (= streak 0)
                    (let ((v (clampv (get params :base) params)))
                      (dict :vel v :accent false :st (mk-state v false 1)))
                    ;; streak is only ever tested against 0, so it
                    ;; saturates at 1: an unbounded count would make every
                    ;; cycle's state (and its eval-cycle memo key) unique
                    (let ((v (clampv (* cur (get params :dot-decay)) params)))
                      (dict :vel v :accent false
                            :st (mk-state v false 1)))))))))

;; ── hand model (spec §6) ────────────────────────────────────────────────────

(def other-hand (h) (if (= h :left) :right :left))

;; one entry per HIT: dots take the current hand, a dash takes it twice
(def derive-hands (evs hand)
  (if (empty? evs)
      (list)
      (append (if (dash-sym? (first evs)) (list hand hand) (list hand))
              (derive-hands (rest evs) (other-hand hand)))))

;; ── row processes: (proc TRIGGER STAGE… ACTION…) ────────────────────────────
;; (docs/jaki-row-processes-spec.md). A route word that collects a chain into
;; the route pattern's :procs. Chains run inside the figure fold: on every hit
;; the TRIGGER selector holds for, the chain STEPS — the stages fold one number
;; x from 0, then the actions use it. Stage args take keywords, and any number
;; arg may be a nested source that is evaluated on the same step:
;;
;;   (proc any (acc :by (coin :p 0.5) :min 0 :max 16) (note+))
;;
;; Chain state rides `st` as :procs, one flat number list per chain,
;; (n hold pend v0 v1 …): n steps so far, the held x, the pending
;; next-figure flag, and one cell per source node (an acc's offset from its
;; min), node ids numbered in parse order.

(def has-procs? (p) (not (empty? (or-default (get p :procs) (list)))))

;; ── parsing (route time) ──

;; every keyword a stage or action takes: an arg list that starts with one of
;; these is read by keyword, else positionally (the rev 1 spelling)
(def proc-keys '(:p :min :max :by :n :e :in-lo :in-hi :out-lo :out-hi :op :to
                 :step :root :track :amount))

(def kw-list? (a) (and (not (empty? a)) (member? (first a) proc-keys)))

(def kw-arg (a key pos d)
  (if (kw-list? a)
      (let ((i (index-of* a key 0))) (if (>= i 0) (or-default (nth a (+ i 1)) d) d))
      (or-default (nth a pos) d)))

(def compound? (x) (and (not (number? x)) (not (string? x)) (not (= (nth x 0) nil))))

(def source-head? (h) (member? h '(coin rand acc count cyc chan)))

;; node ids, numbered per chain while it parses (reset by norm-proc)
(def proc-ids 0)
(def mk-node (k args) (let ((id proc-ids)) (do (set! proc-ids (+ id 1)) (dict :k k :id id :a args))))

;; a number arg: (:node n) for a nested source, else (:lit raw) — per-cycle
;; data resolve-arg reads
(def norm-arg (raw)
  (if (and (compound? raw) (source-head? (raw-head raw)))
      (let ((n (norm-stage raw))) (if (= n nil) (list :lit 0) (list :node n)))
      (list :lit raw)))

(def remap-node (a)
  (mk-node :remap (list (norm-arg (kw-arg a :in-lo 0 -1)) (norm-arg (kw-arg a :in-hi 1 1))
                        (norm-arg (kw-arg a :out-lo 2 0)) (norm-arg (kw-arg a :out-hi 3 1)))))

;; a stage → node dict (:k kind :id :a arg specs [:op]); nil when unknown
(def norm-stage (it)
  (if (number? it)
      (mk-node :const (list (list :lit it)))
      (let ((h (raw-head it)) (a (raw-args it)))
        (match h
          'coin  (mk-node :coin (list (norm-arg (kw-arg a :p 0 0.5))))
          'rand  (mk-node :rand (list (norm-arg (kw-arg a :min 0 0)) (norm-arg (kw-arg a :max 1 1))))
          'acc   (mk-node :acc (list (norm-arg (kw-arg a :by 0 0.1)) (norm-arg (kw-arg a :min 1 0))
                                     (norm-arg (kw-arg a :max 2 1))))
          'count (mk-node :count (list (norm-arg (kw-arg a :n 0 2))))
          'cyc   (mk-node :cyc (list (list :lit (cons 'cyc a))))
          'chan  (mk-node :chan (list (list :lit (nth a 0)) (list :lit (or-default (nth a 1) 0))))
          'sin   (mk-node :sin (list))
          'abs   (mk-node :abs (list))
          'pow   (mk-node :pow (list (norm-arg (kw-arg a :e 0 2))))
          'remap (remap-node a)
          ;; rev 1's linear map, (scale a b c d); keyword `scale` is musical
          'scale (if (number? (nth a 0)) (remap-node a) nil)
          'clamp (mk-node :clamp (list (norm-arg (kw-arg a :min 0 0)) (norm-arg (kw-arg a :max 1 1))))
          'cmp   (merge (mk-node :cmp (list (norm-arg (kw-arg a :to 1 0.5)))) :op (kw-arg a :op 0 '>=))
          'quant (mk-node :quant (list (norm-arg (kw-arg a :step 0 1))))
          _ nil))))

;; ── musical scales and roots (tonic pitch class relative to transpose 0) ──

(def scale-steps (mode)
  (match mode
    :major '(0 2 4 5 7 9 11)  :minor '(0 2 3 5 7 8 10)
    :dorian '(0 2 3 5 7 9 10)  :phrygian '(0 1 3 5 7 8 10)
    :lydian '(0 2 4 6 7 9 11)  :mixolydian '(0 2 4 5 7 9 10)
    :locrian '(0 1 3 5 6 8 10)
    :harmonic-minor '(0 2 3 5 7 8 11)  :melodic-minor '(0 2 3 5 7 9 11)
    :pentatonic '(0 2 4 7 9)  :minor-pentatonic '(0 3 5 7 10)
    :blues '(0 3 5 6 7 10)  :whole-tone '(0 2 4 6 8 10)
    _ '(0 1 2 3 4 5 6 7 8 9 10 11)))

(def note-pc (r)
  (if (number? r)
      (imod (round-int r) 12)
      (match r
        'C 0 'C# 1 'Db 1 'D 2 'D# 3 'Eb 3 'E 4 'F 5 'F# 6 'Gb 6 'G 7 'G# 8 'Ab 8
        'A 9 'A# 10 'Bb 10 'B 11 _ 0)))

(def scale-mask (mode root)
  (let ((r (note-pc root)))
    (reduce (lambda (m s) (+ m (pow2 (imod (+ s r) 12)))) 0 (scale-steps mode))))

;; (scale :minor :root C) → (:smask m); (harmony :track 2 :amount 1) →
;; (:harm track amount); (snap "chan") → (:snap name). Tags the hit; emit
;; moves its final note (snap-hit).
(def scale-op (a)
  (let ((mode (if (and (not (empty? a)) (not (= (nth a 0) :root))) (nth a 0) :major))
        (i (index-of* a :root 0)))
    (list :smask (scale-mask mode (if (>= i 0) (nth a (+ i 1)) 0)))))

(def note-op (w)
  (let ((h (raw-head w)) (a (raw-args w)))
    (match h
      'scale   (if (number? (nth a 0)) nil (scale-op a))
      'harmony (list :harm (kw-arg a :track 0 0) (kw-arg a :amount 1 1))
      'snap    (list :snap (nth a 0))
      _ nil)))

;; one event word of (then W…) / (next W…) → an event op, or nil
(def ev-op (w)
  (let ((h (raw-head w)) (a (raw-args w)))
    (match h
      'rest  (list :drop)
      'stac  (list :stac)
      'vel*  (list :vmul (nth a 0))
      'vel+  (list :vadd (nth a 0))
      'note+ (list :nadd (nth a 0))
      'note  (list :nset (nth a 0))
      'gate  (list :gmul (nth a 0))
      _ (note-op w))))

;; (next W…) words → (dict :xf :ops :drop :fast). Only words that keep the
;; figure's length (spec §6): fig-len and the super-cycle tables never run
;; processes. `half` is length-preserving by construction (half-walk keeps
;; the units), and so is (fast n) / (* n): n× the hits in the figure's own
;; span. Anything else is ignored.
(def next-xf-words '(rev swap ghost stac half))

(def norm-next (ws)
  (reduce
    (lambda (acc w)
      (let ((h (raw-head w)))
        (if (member? h next-xf-words)
            (merge acc :xf (append (get acc :xf) (list (list (head-kw h)))))
            (if (= h 'rest)
                (merge acc :drop true)
                (if (and (member? h vel-model-words) (not (empty? (raw-args w))))
                    (merge acc :xf (append (get acc :xf) (list (norm-xf w))))
                (if (or (= h 'fast) (= h '*))
                    (merge acc :fast (append (get acc :fast) (list (nth (raw-args w) 0))))
                (let ((op (ev-op w)))
                  (if (and (not (= op nil)) (member? (first op) '(:vmul :vadd :nadd :gmul)))
                      (merge acc :ops (append (get acc :ops) (list op)))
                      acc))))))))
    (dict :xf (list) :ops (list) :drop false :fast (list)) ws))

;; held actions: the target with `x` (or nothing) for its number —
;; (note+ x) adds the chain's value to the note, (vel* x) scales velocity by
;; it. (+ note) and (* dashdecay) are the rev 1 spellings.
(def held-word (it)
  (if (or (number? it) (string? it))
      nil
      (let ((h (raw-head it)) (a (raw-args it)))
        (if (or (empty? a) (and (= (len a) 1) (chain-value? (nth a 0))))
            (match h 'note+ :note 'vel* :vel 'gate* :gate 'dashdecay* :dash-decay
                     'dotdecay* :dot-decay 'basevel* :base _ nil)
            (if (and (= h '+) (= (nth a 0) 'note))
                :note
                (if (= h '*)
                    (match (nth a 0) 'dashdecay :dash-decay 'dotdecay :dot-decay
                                     'basevel :base 'vel :vel 'gate :gate _ nil)
                    nil))))))

;; `$1` (or the rev 3 spelling `x`): the value the stages produced
(def chain-value? (v) (or (= v '$1) (= v 'x)))

;; figure words: they reshape a whole figure, so straight in a chain (or in
;; an if branch) they mean the NEXT figure — `(coin :p 0.2) half`. The
;; velocity-model words set that figure's params: `(minvel 0.4)`.
(def vel-model-words '(basevel dotdecay dashdecay minvel maxvel))

(def fig-word? (it)
  (and (compound-or-word? it)
       (or (member? (raw-head it) '(half rev swap ghost fast))
           (and (member? (raw-head it) vel-model-words) (not (empty? (raw-args it)))))))

;; ── (if COND THEN [ELSE]) ──
;; COND is a small expression over numbers and $ context: $1 the stages'
;; value, $n the rule's steps so far (0-based, like the neural expr cards),
;; $vel the hit's velocity, $cycle, $fig / $rep (1-based, like the (fig n) /
;; (rep n) selectors). Comparisons, and/or/not, + - * / mod min max abs; true
;; is any non-zero number. Every word after COND runs when it holds, and
;; the words of an (else w…) when it does not: event words act on this hit,
;; figure words (half (fast 4) (minvel 0.4) …, or (next …)) on the next
;; figure. (do w…) groups words, as anywhere.
(def truthy? (v) (and (number? v) (not (= v 0))))

(def ctx-var (v env)
  (match v
    '$1 (get env :x)  'x (get env :x)  '$n (get env :n)  '$vel (get env :vel)
    '$cycle (get env :cycle)  '$fig (get env :fig)  '$rep (get env :rep)
    _ 0))

(def cond-val (e env)
  (if (number? e)
      e
      (if (= (nth e 0) nil)
          (ctx-var e env)
          (let ((h (raw-head e)) (a (map (lambda (z) (cond-val z env)) (raw-args e))))
            (let ((a0 (or-default (nth a 0) 0)) (a1 (or-default (nth a 1) 0)))
              (match h
                '=  (b->n (= a0 a1))   '!= (b->n (not (= a0 a1)))
                '<  (b->n (< a0 a1))   '>  (b->n (> a0 a1))
                '<= (b->n (<= a0 a1))  '>= (b->n (>= a0 a1))
                'and (b->n (reduce (lambda (acc v) (and acc (truthy? v))) true a))
                'or  (b->n (reduce (lambda (acc v) (or acc (truthy? v))) false a))
                'not (b->n (not (truthy? a0)))
                '+ (reduce (lambda (acc v) (+ acc v)) 0 a)
                '- (if (= (len a) 1) (* -1 a0) (- a0 a1))
                '* (reduce (lambda (acc v) (* acc v)) 1 a)
                '/ (if (= a1 0) 0 (/ a0 a1))
                'mod (if (= a1 0) 0 (fwrap a0 a1))
                'min (min a0 a1)  'max (max a0 a1)  'abs (abs a0)
                _ 0))))))

;; next-figure word sets, collected per rule while it parses. Each set has
;; a countdown cell in the rule's state: firing it sets the count to its
;; length (:for, raw per-cycle data; 1 unless written (for N w…)), and each
;; figure it applies to counts one down.
(def proc-nexts (list))
(def add-next-set (words dur)
  (if (empty? words)
      -1
      (do (set! proc-nexts (append proc-nexts (list (merge (norm-next words) :for dur))))
          (- (len proc-nexts) 1))))

(def for-form? (w) (= (raw-head w) 'for))

;; the next-figure sets action words fire: (next …) and bare figure words
;; together as one set lasting one figure, each (for N w…) its own
(def collect-sets (ws)
  (keep (lambda (i) (>= i 0))
        (cons (add-next-set (reduce (lambda (acc v)
                                      (if (= (raw-head v) 'next)
                                          (append acc (rest v))
                                          (if (fig-word? v) (append acc (list v)) acc)))
                                    (list) ws)
                            1)
              (map (lambda (f) (add-next-set (drop* 2 f) (nth f 1))) (keep for-form? ws)))))

(def flat-words (ws)
  (reduce (lambda (acc w) (if (= (raw-head w) 'do) (append acc (flat-words (rest w))) (append acc (list w))))
          (list) ws))

(def norm-branch (words)
  (let ((ws (flat-words words)))
    (let ((sets (collect-sets ws))
          (ops (some* (map (lambda (v)
                             (if (or (fig-word? v) (= (raw-head v) 'next) (for-form? v)) nil (ev-op v)))
                           ws))))
      (dict :ops ops :sets sets))))

(def else-form? (w) (= (raw-head w) 'else))

(def norm-if (a)
  (let ((ws (drop* 2 a)))
    (dict :cond (nth a 1)
          :then (norm-branch (keep (lambda (w) (not (else-form? w))) ws))
          :else (norm-branch (reduce (lambda (acc w) (append acc (rest w)))
                                     (list) (keep else-form? ws))))))

;; an event word straight in the chain, (note+ -12) or `rest`, acts on the
;; stepping hit when x ≥ 0.5 — `then` without the wrapper. A bare target
;; word with no number (note+) is the held action instead.
(def gated-word (it)
  (if (and (compound-or-word? it) (= (held-word it) nil) (= (note-op it) nil)
           (not (fig-word? it)) (not (member? (raw-head it) '(if for))))
      (ev-op it)
      nil))

(def proc-action? (it)
  (and (compound-or-word? it)
       (or (not (= (held-word it) nil))
           (member? (raw-head it) '(then next if for))
           (fig-word? it)
           (not (= (note-op it) nil))
           (not (= (gated-word it) nil)))))

(def compound-or-word? (it) (and (not (number? it)) (not (string? it))))

(def some* (l) (keep (lambda (x) (not (= x nil))) l))

;; (proc TRIGGER STAGE… ACTION…) args → chain record, or nil without a valid
;; trigger or any action
(def norm-proc (args figs)
  (let ((sel (norm-sel (nth args 0) figs)) (items (rest args))
        (reset (do (set! proc-ids 0) (set! proc-nexts (list)))))
    (let ((stages (some* (map norm-stage (keep (lambda (it) (not (proc-action? it))) items))))
          (acts (keep proc-action? items)))
      (let ((held (some* (map held-word acts)))
            (hitops (some* (map note-op acts)))
            (thens (reduce (lambda (acc a)
                             (if (= (raw-head a) 'then)
                                 (append acc (some* (map ev-op (rest a))))
                                 (let ((g (gated-word a))) (if (= g nil) acc (append acc (list g))))))
                           (list) acts))
            ;; (next …), bare figure words and (for …): sets gated on $1
            (gsets (collect-sets acts)))
        (let ((ifs (map norm-if (keep (lambda (a) (= (raw-head a) 'if)) acts))))
          (if (or (= sel nil)
                  (and (empty? held) (empty? hitops) (empty? thens) (empty? gsets) (empty? ifs)))
              nil
              (dict :sel sel :stages stages :width (+ 3 proc-ids (len proc-nexts))
                    ;; the sets' countdown cells follow the source cells
                    :nbase (+ 3 proc-ids)
                    :muls (keep (lambda (t) (not (= t :note))) held)
                    :nadd (member? :note held)
                    :hitops hitops :then thens :ifs ifs
                    :nexts proc-nexts :gsets gsets)))))))

(def add-proc (p args)
  (let ((c (norm-proc args (get p :figs))))
    (if (= c nil)
        p
        (merge p :procs (append (or-default (get p :procs) (list)) (list c))
                 :id (str (get p :id) "|proc:" (source args))))))

;; eval-time config: nil for a route without processes
(def proc-config (p cycle)
  (if (has-procs? p)
      (let ((chains (get p :procs)))
        (dict :chains chains
              :seed (round-int (resolve-arg (or-default (get p :seed) 0) cycle))
              :pmul (member? true (map (lambda (c)
                                         (or (member? :dash-decay (get c :muls))
                                             (or (member? :dot-decay (get c :muls))
                                                 (member? :base (get c :muls)))))
                                       chains))))
      nil))

;; ── chain state ──

(def chain-width (c) (get c :width))
(def fresh-chain (c) (repeat* 0 (chain-width c)))

;; st's chain states, fresh when st carries none (or another route's)
(def proc-states (st pc)
  (let ((cs (get st :procs)) (chains (get pc :chains)))
    (if (and (not (= cs nil)) (= (len cs) (len chains)))
        cs
        (map fresh-chain chains))))

(def keep-procs (st2 st)
  (if (= (get st :procs) nil) st2 (merge st2 :procs (get st :procs))))

;; a figure has started: every running next-figure set counts one down
(def tick-sets (c s)
  (reduce (lambda (s j)
            (let ((slot (+ (get c :nbase) j)))
              (if (> (nth s slot) 0) (set-nth s slot (- (nth s slot) 1)) s)))
          s (range 0 (len (or-default (get c :nexts) (list))))))

;; ── hashed randomness (spec §7) ──
;; A pure function of (seed chain node cycle figure hit), so the per-cycle
;; memo, roll-state and seeks reproduce every coin. Integer mixing mod a
;; prime below 2^26: every product stays under 2^53, exact in f64. The
;; quadratic scramble (repeated at the end) breaks the linear structure a
;; plain LCG leaves between neighbouring keys.
(def hash-m 67108859)

(def hash-mix (h x)
  (let ((a (imod (* (+ h (imod (round-int x) hash-m) 1) 40503) hash-m)))
    (imod (+ (* a (+ a 12345)) 6789) hash-m)))

(def hash-u (keys)
  (/ (hash-mix (hash-mix (reduce hash-mix 7 keys) 0) 0) hash-m))

;; ── evaluating a step ──
;; env: (dict :hk hash key (seed chain cycle fig hit) :n steps so far
;; :cycle). Values thread the chain state list cs: → (list value cs).

(def fwrap (v w) (- v (* w (floor (/ v w)))))

(def cmp-op? (op x t)
  (match op '> (> x t) '< (< x t) '>= (>= x t) '<= (<= x t) '= (= x t) _ false))

(def arg-val (spec cs env)
  (if (= (first spec) :node)
      (node-val (nth spec 1) 0 cs env)
      (list (resolve-arg (nth spec 1) (get env :cycle)) cs)))

(def args-vals (specs cs env acc)
  (if (empty? specs)
      (list acc cs)
      (let ((r (arg-val (first specs) cs env)))
        (args-vals (rest specs) (nth r 1) env (append acc (list (first r)))))))

;; one node on input x → (list x2 cs2)
(def node-val (g x cs env)
  (let ((r (args-vals (get g :a) cs env (list))))
    (let ((v (nth r 0)) (cs1 (nth r 1)) (id (get g :id)))
      (let ((a (lambda (i) (nth v i)))
            (u (lambda () (hash-u (cons id (get env :hk))))))
        (match (get g :k)
          :const (list (a 0) cs1)
          :coin (list (if (< (u) (a 0)) 1 0) cs1)
          :rand (list (+ (a 0) (* (- (a 1) (a 0)) (u))) cs1)
          ;; the offset from min is stored, so a fresh chain starts at min
          :acc (let ((w (- (a 2) (a 1))) (slot (+ 3 id)))
                 (let ((a1 (if (<= w 0) 0 (fwrap (+ (nth cs1 slot) (a 0)) w))))
                   (list (+ (a 1) a1) (set-nth cs1 slot a1))))
          :count (list (imod (get env :n) (max 1 (round-int (a 0)))) cs1)
          :cyc (list (a 0) cs1)
          :chan (list (chan-get (a 0) (a 1)) cs1)
          :sin (list (sin (* 6.283185307179586 x)) cs1)
          :abs (list (abs x) cs1)
          :pow (list (pow x (a 0)) cs1)
          :remap (list (if (= (a 0) (a 1))
                           (a 2)
                           (+ (a 2) (/ (* (- x (a 0)) (- (a 3) (a 2))) (- (a 1) (a 0)))))
                       cs1)
          :clamp (list (max (a 0) (min (a 1) x)) cs1)
          :cmp (list (if (cmp-op? (get g :op) x (a 0)) 1 0) cs1)
          :quant (list (if (<= (a 0) 0) x (* (a 0) (round-int (/ x (a 0))))) cs1)
          _ (list x cs1))))))

(def stage-walk (stages x cs env)
  (if (empty? stages)
      (list x cs)
      (let ((r (node-val (first stages) x cs env)))
        (stage-walk (rest stages) (first r) (nth r 1) env))))

;; fire next-figure sets: each runs for its length from the next figure (a
;; set still running keeps the longer of the two)
(def fire-sets (c cs sets cycle)
  (reduce (lambda (cs j)
            (let ((slot (+ (get c :nbase) j))
                  (d (max 1 (round-int (resolve-arg (get (nth (get c :nexts) j) :for) cycle)))))
              (set-nth cs slot (max (nth cs slot) d))))
          cs sets))

;; an op whose value is $1 takes the stages' value
(def subst-x (op x)
  (if (and (> (len op) 1) (chain-value? (nth op 1))) (set-nth op 1 x) op))

;; the rule's ifs on this step → (dict :ops this-hit ops :sets next sets)
(def run-ifs (ifs env)
  (reduce (lambda (acc f)
            (let ((b (get f (if (truthy? (cond-val (get f :cond) env)) :then :else))))
              (dict :ops (append (get acc :ops)
                                 (map (lambda (op) (subst-x op (get env :x))) (get b :ops)))
                    :sets (append (get acc :sets) (get b :sets)))))
          (dict :ops (list) :sets (list)) ifs))

;; one chain on one hit → (list state ops): ops are the chain's note ops on
;; every hit it steps on, its gated words when x ≥ 0.5 (spec §5.2), and its
;; if branches. A rule with no stages has nothing to decide, so x is 1: it
;; always acts ((fig 1) -> (next half) halftimes every figure after figure 1).
(def step-chain (c k s0 hctx cycle seed)
  (if (not (sel-ok? (get c :sel) hctx cycle))
      (list s0 (list))
      (let ((n (nth s0 0)))
        (let ((r (stage-walk (get c :stages) (if (empty? (get c :stages)) 1 0) s0
                             (dict :n n :cycle cycle
                                   :hk (list seed k cycle (get hctx :fig) (get hctx :hidx))))))
          (let ((x (if (number? (first r)) (first r) 0)) (cs (nth r 1)))
            (let ((on? (>= x 0.5))
                  (fi (run-ifs (get c :ifs)
                               (dict :x x :n n :vel (get hctx :vel) :cycle cycle
                                     :fig (+ 1 (or-default (get hctx :afig) 0))
                                     :rep (+ 1 (or-default (get hctx :arep) 0))))))
              (list (fire-sets c (set-nth (set-nth cs 0 (+ n 1)) 1 x)
                               (append (if on? (get c :gsets) (list)) (get fi :sets)) cycle)
                    (append (get c :hitops) (if on? (get c :then) (list)) (get fi :ops)))))))))

(def step-chains (chains cs hctx cycle seed)
  (reduce (lambda (acc k)
            (let ((r (step-chain (nth chains k) k (nth cs k) hctx cycle seed)))
              (dict :cs (append (get acc :cs) (list (first r)))
                    :then (append (get acc :then) (nth r 1)))))
          (dict :cs (list) :then (list))
          (range 0 (len chains))))

;; ── actions ──

;; product of the held values of the chains that hold `target`; 1 (identity)
;; for a chain that has not stepped yet (spec §5.1)
(def held-mul (chains cs target)
  (reduce (lambda (m k)
            (let ((s (nth cs k)))
              (if (and (> (nth s 0) 0) (member? target (get (nth chains k) :muls)))
                  (* m (nth s 1))
                  m)))
          1 (range 0 (len chains))))

(def held-note (chains cs)
  (reduce (lambda (a k)
            (let ((s (nth cs k)))
              (if (and (> (nth s 0) 0) (get (nth chains k) :nadd)) (+ a (round-int (nth s 1))) a)))
          0 (range 0 (len chains))))

;; the velocity-model params as the figure resolved them, times the held values
(def held-params (params chains cs)
  (merge params
    :base (clamp01 (* (get params :base) (held-mul chains cs :base)))
    :dot-decay (clamp01 (* (get params :dot-decay) (held-mul chains cs :dot-decay)))
    :dash-decay (clamp01 (* (get params :dash-decay) (held-mul chains cs :dash-decay)))))

(def gate-rat (s) (rat (round-int (* (max 0 s) 96)) 96))

(def held-event (ev chains cs)
  (let ((nd (held-note chains cs)))
    (let ((e1 (if (= nd 0) ev (merge ev :nadd (+ (or-default (get ev :nadd) 0) nd)))))
      (let ((e2 (if (member? true (map (lambda (c) (member? :vel (get c :muls))) chains))
                    (merge e1 :vel (clamp01 (* (get e1 :vel) (held-mul chains cs :vel))))
                    e1)))
        (if (member? true (map (lambda (c) (member? :gate (get c :muls))) chains))
            (merge e2 :gate (r* (get e2 :gate) (gate-rat (held-mul chains cs :gate))))
            e2)))))

(def ev-op-apply (e op cycle)
  (let ((v (lambda () (resolve-arg (nth op 1) cycle))))
    (match (first op)
      :drop (merge e :drop true)
      :stac (merge e :gate (r-min (get e :gate) (rat 1 4)))
      :vmul (merge e :vel (clamp01 (* (get e :vel) (v))))
      :vadd (merge e :vel (clamp01 (+ (get e :vel) (v))))
      :nadd (merge e :nadd (+ (or-default (get e :nadd) 0) (v)))
      :nset (merge e :nset (v))
      :gmul (merge e :gate (r* (get e :gate) (gate-rat (v))))
      :snap (merge e :snap (nth op 1))
      :smask (merge e :smask (nth op 1))
      :harm (merge e :harm (rest op))
      _ e)))

(def ev-ops-apply (e ops cycle) (reduce (lambda (x op) (ev-op-apply x op cycle)) e ops))

;; one hit with row processes: the velocity model sees the held param values
;; from BEFORE this hit (a step needs the hit's accent), then the chains step
;; and the hit takes the held event values and this step's ops.
(def proc-hit (dash? second? ctx st off hand hidx pc)
  (let ((chains (get pc :chains)) (cs (get st :procs)) (cycle (get ctx :cycle)))
    (let ((r (mk-hit dash? second?
                     (if (get pc :pmul)
                         (merge ctx :params (held-params (get ctx :params) chains cs))
                         ctx)
                     st off hand)))
      (let ((ev (get r :ev)))
        (let ((sr (if (let ((sp (get ctx :steps))) (or (= sp nil) (nth sp hidx)))
                      (step-chains chains cs
                               (merge ev :afig (get ctx :afig) :arep (get ctx :arep) :hidx hidx)
                               cycle (get pc :seed))
                      ;; a hit a rule's fast added: rules do not step on it
                      (dict :cs cs :then (list)))))
          (dict :ev (ev-ops-apply (held-event ev chains (get sr :cs)) (get sr :then) cycle)
                :st (merge (get r :st) :procs (get sr :cs))))))))

(def fold-hit (dash? second? ctx st off hand hidx)
  (let ((pc (get ctx :pc)))
    (if (= pc nil)
        (mk-hit dash? second? ctx st off hand)
        (proc-hit dash? second? ctx st off hand hidx pc))))

;; ── next-figure actions (spec §6) ──

;; the merged (next …) words of every pending chain, or nil
(def merge-next (a nx)
  (let ((a (or-default a (dict :xf (list) :ops (list) :drop false :fast (list)))))
    (dict :xf (append (get a :xf) (get nx :xf))
          :ops (append (get a :ops) (get nx :ops))
          :drop (or (get a :drop) (get nx :drop))
          :fast (append (get a :fast) (get nx :fast)))))

;; the merged next-figure words of every set still running, or nil
(def pending-next (chains cs)
  (reduce (lambda (acc k)
            (let ((c (nth chains k)) (s (nth cs k)))
              (let ((nexts (or-default (get c :nexts) (list))))
                (reduce (lambda (a j) (if (> (nth s (+ (get c :nbase) j)) 0) (merge-next a (nth nexts j)) a))
                        acc (range 0 (len nexts))))))
          nil (range 0 (len chains))))

;; `rest` drops the figure's hits after they threaded hands and velocity
(def apply-next (body nx cycle)
  (merge body :evs
    (map (lambda (e)
           (let ((e2 (ev-ops-apply e (get nx :ops) cycle)))
             (if (get nx :drop) (merge e2 :drop true) e2)))
         (get body :evs))))

;; ── figure fold: symbolic events → timed events ─────────────────────────────

(def hit-ev (off sym hit hand vel accent ctx)
  (dict :off off :sym sym :hit hit :hand hand :vel vel :accent accent
        :fig (get ctx :fig) :gate (get ctx :gate)))

(def mk-hit (dash? second? ctx st off hand)
  (if (and (get ctx :ghost) dash? (not second?))
      ;; ghosted first dash hit: velocity 0, state untouched (accent still
      ;; fires on the event after the dash)
      (dict :ev (hit-ev off :dash 1 hand 0 false ctx) :st st)
      (if (and (get ctx :ghost) dash? second?)
          ;; ghost pickup: dash-decay used directly as the velocity
          (let ((params (get ctx :params)))
            (let ((v (clampv (get params :dash-decay) params)))
              (dict :ev (hit-ev off :dash 2 hand v false ctx)
                    :st (mk-state v true (get st :streak)))))
          (let ((r (next-vel st dash? second? (get ctx :params))))
            (dict :ev (hit-ev off (if dash? :dash :dot) (if second? 2 1)
                              hand (get r :vel) (get r :accent) ctx)
                  :st (get r :st))))))

(def fold-events (evs hands ctx st off hit-idx acc)
  (if (empty? evs)
      (dict :evs acc :st st :off off)
      (let ((dash? (dash-sym? (first evs)))
            (mul (sym-mul (first evs))))
        ;; a halftime symbol spans (and gates) twice its plain length; `hctx`
        ;; is this symbol's context only, the rest fold with `ctx`
        (let ((scale (r* (get ctx :scale) (r-int mul)))
              (hctx (if (= mul 1) ctx (merge ctx :gate (r* (get ctx :gate) (r-int mul))))))
          (let ((r1 (fold-hit dash? false hctx st off (nth hands hit-idx) hit-idx)))
            (if dash?
                (let ((r2 (fold-hit true true hctx (get r1 :st) (r+ off scale)
                                    (nth hands (+ hit-idx 1)) (+ hit-idx 1))))
                  (fold-events (rest evs) hands ctx (get r2 :st)
                               (r+ off (r* scale (r-int 2))) (+ hit-idx 2)
                               (append acc (list (get r1 :ev) (get r2 :ev)))))
                (fold-events (rest evs) hands ctx (get r1 :st)
                             (r+ off scale) (+ hit-idx 1)
                             (append acc (list (get r1 :ev))))))))))

;; post-fold pass: gate doubling before a ghosted pickup, dropping
;; zero-velocity dash hits, and accent gate extension over silent tails
(def count-zero-after (evs i)
  (if (and (< i (len evs)) (= (get (nth evs i) :vel) 0))
      (+ 1 (count-zero-after evs (+ i 1)))
      0))

(def post-fold-walk (evs i acc)
  (if (>= i (len evs))
      acc
      (let ((e (nth evs i)))
        (let ((e2 (if (and (= (get e :sym) :dash) (= (get e :hit) 1)
                           (< (+ i 1) (len evs))
                           (= (get (nth evs (+ i 1)) :hit) 2)
                           (= (get (nth evs (+ i 1)) :vel) 0))
                      (merge e :gate (r* (get e :gate) (r-int 2)))
                      e)))
          (if (and (= (get e2 :vel) 0) (= (get e2 :sym) :dash))
              (post-fold-walk evs (+ i 1) acc)
              (let ((e3 (if (get e2 :accent)
                            (merge e2 :gate
                                   (r* (get e2 :gate)
                                       (r-int (+ 1 (count-zero-after evs (+ i 1))))))
                            e2)))
                (post-fold-walk evs (+ i 1) (append acc (list e3)))))))))

;; alignment padding: dots on the straight unit grid, hand and velocity state
;; threading through (Swift generateAlignmentPadding)
(def gen-padding (count start hand st params figidx)
  (if (<= count 0)
      (dict :evs (list) :hand hand :st st)
      (let ((r (next-vel st false false params)))
        (let ((sub (gen-padding (- count 1) (+ start 1) (other-hand hand)
                                (get r :st) params figidx)))
          (dict :evs (cons (dict :off (r-int start) :sym :dot :hit 1 :hand hand
                                 :vel (get r :vel) :accent (get r :accent)
                                 :fig figidx :gate (rat 4 5))
                           (get sub :evs))
                :hand (get sub :hand)
                :st (get sub :st))))))

;; alignment (spec §4.5): snap the accumulated duration up to a multiple of n;
;; :pad fills the gap with dot events that thread hand and velocity state
(def apply-align (body al cycle off params figidx)
  (if (= al nil)
      body
      (let ((n (max 1 (round-int (resolve-arg (nth al 0) cycle))))
            (pad? (nth al 1))
            (total (r+ off (get body :dur))))
        (let ((d0 (r-ceil total)))
          (let ((dprime (* n (iceil-div d0 n)))
                (dur (lambda (dp) (r- (r-int dp) off))))
            (if (and pad? (> (- dprime d0) 0))
                (let ((pr (gen-padding (- dprime d0) d0 (get body :hand)
                                       (get body :st) params figidx)))
                  (dict :evs (append (get body :evs) (get pr :evs))
                        :dur (dur dprime)
                        :hand (get pr :hand)
                        ;; padding runs no row processes: chain state passes
                        ;; through unchanged
                        :st (keep-procs (get pr :st) (get body :st))))
                (merge body :dur (dur dprime))))))))

;; evaluate one figure → (dict :evs :dur :hand :st). `pc` is the route's row
;; process config (nil: none): pending (next …) words apply to this figure
;; first (spec §6), and hits a chain drops are removed last.
(def eval-fig (fig cycle off hand st figidx pc)
  (if (= pc nil)
      (eval-fig* fig cycle off hand st figidx nil)
      (let ((cs (proc-states st pc)))
        (let ((nx (pending-next (get pc :chains) cs)))
          (let ((body (eval-fig* (if (= nx nil)
                                     fig
                                     (merge fig :xf (append (get fig :xf) (get nx :xf))
                                                :nfast (reduce (lambda (p r) (* p (max 1 (round-int (resolve-arg r cycle)))))
                                                               1 (get nx :fast))))
                                 cycle off hand (merge st :procs (map (lambda (k) (tick-sets (nth (get pc :chains) k) (nth cs k)))
                                                          (range 0 (len cs)))) figidx pc)))
            (let ((body2 (if (= nx nil) body (apply-next body nx cycle))))
              (merge body2 :evs (keep (lambda (e) (not (get e :drop))) (get body2 :evs)))))))))

(def eval-fig* (fig cycle off hand st figidx pc)
  (let ((xfs (get fig :xf))
        (evs1 (cut-tail (apply-xf-events (get fig :events) (get fig :xf) cycle) fig))
        (tm (tm-at (get fig :tm) cycle)))
    (let ((kind0 (if (= tm nil) nil (nth tm 0)))
          (m0 (if (= tm nil) 1 (max 1 (round-int (resolve-arg (nth tm 1) cycle)))))
          (nf (or-default (get fig :nfast) 1)))
     ;; a pending (next (fast n)) multiplies a plain or fast figure's rate;
     ;; length is unchanged either way (fig-len never sees it)
     (let ((kind (if (and (> nf 1) (or (= kind0 nil) (= kind0 :fast))) :fast kind0))
           (m (if (and (> nf 1) (or (= kind0 nil) (= kind0 :fast))) (* m0 nf) m0)))
      ;; a rule's fast expands the figure as written (its own time-mod
      ;; first), and only the hits it was written with step rules (steps)
      (let ((evs2 (if (= kind :fast)
                      (if (> nf 1) (expand-fast (expand-fast evs1 m0) nf) (expand-fast evs1 m))
                      evs1))
            (steps (if (and (> nf 1) (= kind :fast))
                       (fast-steps (expand-fast evs1 (if (= kind0 :fast) m0 1)) nf)
                       nil)))
        (let ((raw (units evs2))
              (params (apply-overrides default-params (vel-overrides xfs cycle))))
          (let ((eff (match kind
                       :fit (r-int m)
                       :fast (rat raw m)
                       :slow (r-int (* raw m))
                       _ (r-int raw))))
            (let ((scale (if (= raw 0) (r-int 1) (r-div eff (r-int raw))))
                  (stac? (flag-active? xfs :stac cycle))
                  (ghost? (flag-active? xfs :ghost cycle))
                  (swap? (flag-active? xfs :swap cycle)))
              (let ((gate (if stac?
                              (r-min (r* scale (rat 1 4)) (rat 1 4))
                              (r* scale (rat 4 5))))
                    (hands0 (derive-hands evs2 hand)))
                (let ((folded (fold-events evs2
                                (if swap? (map other-hand hands0) hands0)
                                (dict :scale scale :gate gate :ghost ghost?
                                      :params params :fig figidx
                                      :pc pc :cycle cycle
                                      :afig (get fig :afig) :arep (get fig :arep)
                                      :steps steps)
                                st off 0 (list))))
                  ;; ending hand from the transformed event count; swap
                  ;; exchanges assignment within the cycle only and does not
                  ;; disturb the threaded alternation
                  (let ((body (dict :evs (post-fold-walk (get folded :evs) 0 (list))
                                    :dur eff
                                    :hand (if (= 0 (imod (len evs2) 2))
                                              hand
                                              (other-hand hand))
                                    :st (get folded :st))))
                    (apply-align body (get fig :align) cycle off params figidx))))))))))))

;; ── whole-pattern evaluation ────────────────────────────────────────────────

(def eval-figs (figs cycle off hand st idx acc pc)
  (if (empty? figs)
      (dict :evs acc :off off :hand hand :st st)
      (let ((r (eval-fig (first figs) cycle off hand st idx pc))
            (a (get (first figs) :afig))
            (k (get (first figs) :arep)))
        (eval-figs (rest figs) cycle (r+ off (get r :dur))
                   (get r :hand) (get r :st) (+ idx 1)
                   (append acc (map (lambda (e) (merge e :afig a :arep k))
                                    (get r :evs)))
                   pc))))

;; quoted true/false arrive as symbols in filter specs
(def as-bool (v) (if (= v 'true) true (if (= v 'false) false v)))

(def spec-ok? (ev spec axis field)
  (let ((want (get spec axis)))
    (if (= want nil)
        true
        (= (get ev field) (as-bool want)))))

(def ev-match? (ev spec)
  (and (spec-ok? ev spec :hand :hand)
       (spec-ok? ev spec :symbol :sym)
       (spec-ok? ev spec :accent :accent)
       (spec-ok? ev spec :hit :hit)
       (spec-ok? ev spec :figure :fig)))

;; gate extension to the next surviving event, last to cycle end (spec §7)
(def extend-kept (evs total)
  (if (empty? evs)
      evs
      (let ((next-off (if (empty? (rest evs)) total (get (nth evs 1) :off))))
        (cons (merge (first evs) :gate (r- next-off (get (first evs) :off)))
              (extend-kept (rest evs) total)))))

(def first-off-after (all off total)
  (if (empty? all)
      total
      (if (r< off (get (first all) :off))
          (get (first all) :off)
          (first-off-after (rest all) off total))))

;; gate extension to the next unfiltered event (accent filter style)
(def extend-unfiltered (kept all total)
  (map (lambda (e)
         (merge e :gate (r- (first-off-after all (get e :off) total) (get e :off))))
       kept))

(def apply-filter (res spec)
  (let ((all (get res :events))
        (total (get res :len)))
    (let ((kept (keep (lambda (e) (ev-match? e spec)) all)))
      (let ((mode (if (not (= (get spec :hand) nil))
                      :kept
                      (if (= (as-bool (get spec :accent)) true)
                          :unfiltered
                          (if (= (as-bool (get spec :legato)) true) :kept :none)))))
        (merge res :events
          (match mode
            :kept (extend-kept kept total)
            :unfiltered (extend-unfiltered kept all total)
            _ kept))))))

(def apply-shift (res n)
  (let ((total (get res :len)))
    (if (r<= total (r-int 0))
        res
        (merge res :events
          (sort-evs
            (map (lambda (e)
                   (merge e :off (r-mod (r+ (get e :off) (r-int n)) total)))
                 (get res :events)))))))

(def fh-one (e hand xf cycle)
  (let ((h (first xf)))
    (match h
      :every (if (every-active? (round-int (resolve-arg (nth xf 1) cycle)) cycle)
                 (fh-one e hand (nth xf 2) cycle)
                 e)
      :stac (if (= (get e :hand) hand)
                (merge e :gate (r-min (get e :gate) (rat 1 4)))
                e)
      _ e)))

(def apply-for-hand (res hand xf cycle)
  (merge res :events
    (map (lambda (e) (fh-one e hand xf cycle)) (get res :events))))

;; quantize as a post op: snap every event offset to the nearest multiple of
;; q (a rational, in units), wrapping into the cycle like `shift`. Route-word
;; `(quant tb)` resolves tb to units at route time so the memoized evaluator
;; stays resolution-independent.
(def apply-quant (res q)
  (let ((total (get res :len)))
    (if (r<= q (r-int 0))
        res
        (merge res :events
          (sort-evs
            (map (lambda (e)
                   (let ((snapped (r* (r-int (round-int (r->f (r-div (get e :off) q)))) q)))
                     (merge e :off
                       (if (r<= total (r-int 0)) snapped (r-mod snapped total)))))
                 (get res :events)))))))

;; staccato as a post op: cap every gate at 1/4 unit. Route-word `stac` lands
;; here (not as the xf flag) so it applies in authored word order relative to
;; the gate-extending filters — `left stac` caps after the extension.
(def apply-stac (res)
  (merge res :events
    (map (lambda (e) (merge e :gate (r-min (get e :gate) (rat 1 4))))
         (get res :events))))

;; gate scale as a post op: multiply every gate by s (resolved per cycle,
;; rationalized over 96, clamped at 0). Route words `(gate s)` / `(dur s)`
;; land here, in authored word order like stac — `left (gate 0.5)` scales the
;; filter-extended gates, `(gate 0.5) left` scales before the extension.
(def apply-gate-scale (res s)
  (let ((scale (rat (round-int (* (max 0 s) 96)) 96)))
    (merge res :events
      (map (lambda (e) (merge e :gate (r* (get e :gate) scale)))
           (get res :events)))))

;; per-event velocity / note adjustments (spec §7.1.2): vel* / vel+ clamp to
;; 0..1; note+ accumulates on :nadd, (note n) inside `on` sets :nset, and
;; `emit` plays (:nset or the route's note) + :nadd
(def clamp01 (v) (max 0 (min 1 v)))

(def map-events (res f) (merge res :events (map f (get res :events))))

(def apply-event-op (res h v)
  (map-events res
    (lambda (e)
      (match h
        :vmul (merge e :vel (clamp01 (* (get e :vel) v)))
        :vadd (merge e :vel (clamp01 (+ (get e :vel) v)))
        :nadd (merge e :nadd (+ (or-default (get e :nadd) 0) v))
        _ (merge e :nset v)))))

;; run a post op on the events `on?` picks; the rest pass through untouched
(def apply-scoped (res on? op cycle)
  (let ((evs (get res :events)))
    (let ((sub (apply-post-one (merge res :events (keep on? evs)) op cycle)))
      (merge sub :events
        (sort-evs (append (get sub :events)
                          (keep (lambda (e) (not (on? e))) evs)))))))

(def apply-post-one (res op cycle)
  (let ((h (first op)))
    (match h
      :filter (apply-filter res (nth op 1))
      :shift (apply-shift res (round-int (resolve-arg (nth op 1) cycle)))
      :for-hand (apply-for-hand res (nth op 1) (nth op 2) cycle)
      :stac (apply-stac res)
      :gate (apply-gate-scale res (resolve-arg (nth op 1) cycle))
      :quant (apply-quant res (nth op 1))
      :every (if (every-active? (round-int (resolve-arg (nth op 1) cycle)) cycle)
                 (apply-post-one res (nth op 2) cycle)
                 res)
      :alt (apply-post-one res (alt-pick (rest op) cycle) cycle)
      ;; (every-fig n post): the post op sees only the picked figures'
      ;; events; the rest pass through untouched
      :fig-every (apply-scoped res
                   (lambda (e) (fig-index-active? (nth op 1) (get e :fig) cycle))
                   (nth op 2) cycle)
      ;; (on SEL post): the post op sees only the events SEL matches
      :on (apply-scoped res (lambda (e) (sel-ok? (nth op 1) e cycle)) (nth op 2) cycle)
      :vmul (apply-event-op res :vmul (resolve-arg (nth op 1) cycle))
      :vadd (apply-event-op res :vadd (resolve-arg (nth op 1) cycle))
      :nadd (apply-event-op res :nadd (resolve-arg (nth op 1) cycle))
      :nset (apply-event-op res :nset (resolve-arg (nth op 1) cycle))
      :plock (let ((l (nth op 1)) (v (resolve-arg (nth op 2) cycle)))
               (map-events res (lambda (e) (merge e :params (plock-set (get e :params) l v)))))
      ;; row item idx applied to the matched events: emit reports them so the
      ;; kind panel lights the item (spec §7.3)
      :tag (let ((idx (nth op 2)))
             (map-events res
               (lambda (e)
                 (if (sel-ok? (nth op 1) e cycle)
                     (merge e :lits (cons idx (or-default (get e :lits) (list))))
                     e))))
      ;; resolved at emit against its counters (spec §7.2)
      :defer (map-events res
               (lambda (e) (merge e :defer (append (or-default (get e :defer) (list))
                                                   (list (rest op))))))
      ;; drop every event, keeping the cycle's length/timing: the silent
      ;; alternation member ((left right none)) and (every n none) thinning
      :none (merge res :events (list))
      ;; a note op (snap / scale / harmony), resolved at emit
      :snap (map-events res (lambda (e) (merge e :snap (nth op 1))))
      :noteop (map-events res (lambda (e) (ev-op-apply e (nth op 1) cycle)))
      :chordv (map-events res (lambda (e) (merge e :chordv (nth op 1))))
      _ res)))

;; evaluate a pattern for one cycle with explicit threading state
(def eval-at (p cycle hand st)
  (let ((r (eval-figs (cut-figs p cycle) cycle (r-int 0) hand st 0 (list) (proc-config p cycle))))
    (reduce (lambda (acc op) (apply-post-one acc op cycle))
            (dict :events (sort-evs (get r :evs)) :len (get r :off)
                  :end-hand (get r :hand) :end-st (get r :st))
            (get p :post))))

;; ── per-cycle memo (assoc list in scheduler-VM globals, spec §8.2) ──────────

(def memo-store (list))     ; periodic keys (cycle mod :period)
(def exact-memo (list))     ; exact cycle keys, kept apart: see eval-cycle-entry
(def len-memo (list))
(def prepared-memo (list))   ; `prepared` below: body → route records

(def memo-find (m key) (memo-find-at m key 0 (len m)))

;; walks by index: `(rest m)` would copy the rest of the list every step
(def memo-find-at (m key i n)
  (if (>= i n) nil (memo-check m key i n (nth m i))))

(def memo-check (m key i n entry)
  (if (= (nth entry 0) key) (nth entry 1) (memo-find-at m key (+ i 1) n)))

;; ── evaluation period ───────────────────────────────────────────────────────
;; eval-at depends on the cycle index only through per-cycle arguments ((cyc
;; …), implicit cyc), `every` gates and word alternations, so for a pattern
;; built from those alone eval-at(c) = eval-at(c mod P). Keying the memo on
;; c mod P turns every steady-state cycle boundary into a lookup instead of a
;; full re-evaluation. 0 means "not provably periodic" — some argument is an
;; arbitrary expression, which `resolve-arg` evaluates from source — and
;; keeps the exact cycle key.

(def lcm0 (a b) (if (or (= a 0) (= b 0)) 0 (lcm* a b)))

;; n members, each nested list stepping per visit: n × the members' period
(def members-period (vals)
  (* (max 1 (len vals)) (reduce (lambda (a x) (lcm0 a (raw-period x))) 1 vals)))

;; period of (resolve-arg raw c) as a function of c
(def raw-period (raw)
  (if (or (= raw nil) (number? raw) (string? raw))
      1
      (let ((h (nth raw 0)))
        (if (= h 'at)
            (raw-period (nth raw 2))
        (if (or (= h 'cyc) (= h 'seq))
            (members-period (if (= h 'seq) (drop* 2 raw) (rest raw)))
            (if (= h 'chan)
                1
                (if (= h 'every-gate)
                    (lcm0 (every-period (nth raw 1))
                          (lcm0 (raw-period (nth raw 2)) (raw-period (nth raw 3))))
                    (if (= h 'fig-gate)
                        (lcm0 (raw-period (nth raw 1))
                              (lcm0 (raw-period (nth raw 2)) (raw-period (nth raw 3))))
                        (if (implicit-cyc? raw) (members-period raw) 0)))))))))

;; every numeric value raw can resolve to, or nil when one is not a number
(def raw-leaves (raw)
  (if (= raw nil)
      (list 1)
      (if (number? raw)
          (list raw)
          (let ((h (nth raw 0)))
            (let ((vals (if (= h 'cyc) (rest raw)
                            (if (= h 'at) (list (nth raw 2))
                            (if (= h 'seq) (drop* 2 raw)
                                (if (implicit-cyc? raw) raw nil))))))
              (if (= vals nil)
                  nil
                  (reduce (lambda (acc x)
                            (if (= acc nil)
                                nil
                                (let ((l (raw-leaves x)))
                                  (if (= l nil) nil (append acc l)))))
                          (list) vals)))))))

;; (every n …) is active when (c + 1) mod n = 0, n itself resolved per cycle
(def every-period (nraw)
  (let ((ns (raw-leaves nraw)))
    (if (= ns nil)
        0
        (reduce (lambda (a n) (lcm0 a (max 1 (round-int n)))) (raw-period nraw) ns))))

(def xf-period* (f)
  (if (= f nil)
      1
      (match (first f)
        :every (lcm0 (every-period (nth f 1)) (xf-period* (nth f 2)))
        ;; the figure index, not the cycle, picks which figures: only n's
        ;; own per-cycle variation matters
        :fig-every (lcm0 (raw-period (nth f 1)) (xf-period* (nth f 2)))
        :on (lcm0 (sel-period* (nth f 1)) (xf-period* (nth f 2)))
        :alt (reduce (lambda (a x) (lcm0 a (xf-period* x)))
                     (max 1 (len (rest f))) (rest f))
        :L (xf-period* (nth f 1))
        :R (xf-period* (nth f 1))
        :split 1
        :merge 1
        _ (reduce (lambda (a x) (lcm0 a (raw-period x))) 1 (rest f)))))

(def tm-period* (tm)
  (if (= tm nil)
      1
      (if (= (first tm) :when)
          (lcm0 (match (nth tm 1)
                  :every (every-period (nth tm 2))
                  :on (sel-period* (nth tm 2))
                  _ (raw-period (nth tm 2)))
                (lcm0 (tm-period* (nth tm 4)) (tm-period* (nth tm 5))))
          (raw-period (nth tm 1)))))

(def fig-period* (fig)
  (let ((tm (get fig :tm)) (al (get fig :align)))
    (reduce (lambda (a f) (lcm0 a (xf-period* f)))
            (lcm0 (tm-period* tm)
                  (if (= al nil) 1 (raw-period (nth al 0))))
            (get fig :xf))))

(def post-period* (op)
  (match (first op)
    :shift (raw-period (nth op 1))
    :gate (raw-period (nth op 1))
    :for-hand (xf-period* (nth op 2))
    :every (lcm0 (every-period (nth op 1)) (post-period* (nth op 2)))
    :fig-every (lcm0 (raw-period (nth op 1)) (post-period* (nth op 2)))
    :on (lcm0 (sel-period* (nth op 1)) (post-period* (nth op 2)))
    :vmul (raw-period (nth op 1))
    :vadd (raw-period (nth op 1))
    :nadd (raw-period (nth op 1))
    :nset (raw-period (nth op 1))
    :plock (raw-period (nth op 2))
    :defer 1
    :tag (sel-period* (nth op 1))
    :alt (reduce (lambda (a x) (lcm0 a (post-period* x)))
                 (max 1 (len (rest op))) (rest op))
    _ 1))

;; period of a selector's truth as a function of the cycle; 0 = unknown
(def sel-period* (s)
  (match (first s)
    :fig (raw-period (nth s 1))
    :rep (raw-period (nth s 1))
    :every (every-period (nth s 1))
    :fig-every (raw-period (nth s 1))
    ;; (c·K + k + 1) mod n repeats within n cycles
    :nth (every-period (nth s 1))
    :not (sel-period* (nth s 1))
    :and (reduce (lambda (a x) (lcm0 a (sel-period* x))) 1 (rest s))
    :or (reduce (lambda (a x) (lcm0 a (sel-period* x))) 1 (rest s))
    :alt (reduce (lambda (a x) (lcm0 a (sel-period* x))) (max 1 (len (rest s))) (rest s))
    _ 1))

;; row processes hash the cycle and carry non-periodic state: exact keys
(def gtrunc-period* (e)
  (if (= (first e) :all)
      (raw-period (nth e 1))
      (lcm0 (every-period (nth e 1)) (raw-period (nth e 2)))))

(def eval-period (p)
  (let ((pd (reduce (lambda (a op) (lcm0 a (post-period* op)))
                    (reduce (lambda (a e) (lcm0 a (gtrunc-period* e)))
                            (reduce (lambda (a f) (lcm0 a (fig-period* f))) 1 (get p :figs))
                            (or-default (get p :gtrunc) (list)))
                    (get p :post))))
    (if (or (> pd 256) (has-procs? p)) 0 pd)))

;; `prepare` stamps :period on each route's pattern; patterns built by hand
;; (emit, emit*) carry none and keep the exact cycle key.
(def with-period (p) (merge p :period (eval-period p)))

(def periodic? (p) (let ((pd (get p :period))) (and (number? pd) (> pd 0))))

(def memo-cycle (p cycle)
  (if (periodic? p) (imod cycle (get p :period)) cycle))

(def eval-cycle (p cycle hand st) (first (eval-cycle-entry p cycle hand st)))

;; (list result buckets): eval-cycle's result and its `unit-buckets`
(def eval-cycle-entry (p cycle hand st)
  ;; Payload channels can alter evaluated event data but never cycle length.
  ;; Include their epoch here only: len-memo and lens-memo intentionally keep
  ;; their structural keys across channel writes.
  ;; Exact-cycle keys (row processes, unstamped patterns) never recur, yet
  ;; such a route inserts one every cycle: sharing the 64-entry store let
  ;; one instance's row processes evict another's periodic steady-state
  ;; entries, so every ~20 cycles each route re-evaluated at once on the
  ;; same bar-start tick (eseq-8cim). They get their own small store.
  (let ((key (list (get p :id) (memo-cycle p cycle) hand
                   (get st :cur) (get st :pwd) (get st :streak)
                   (chan-epoch) (get st :procs))))
    (let ((hit (memo-find (if (periodic? p) memo-store exact-memo) key)))
      (if (= hit nil)
          (let ((r (eval-at p cycle hand st)))
            (let ((entry (list r (unit-buckets r))))
              (do (if (periodic? p)
                      (set! memo-store (cons (list key entry) (take* 63 memo-store)))
                      (set! exact-memo (cons (list key entry) (take* 31 exact-memo))))
                  entry)))
          hit))))

;; A cycle's events grouped by the unit they start in, (floor :off): bucket
;; u holds, in order, exactly the events `emit-window` can emit for unit u,
;; so a tick looks at those instead of the whole cycle.
(def unit-buckets (r)
  (reduce (lambda (buckets e) (add-to-bucket buckets (ev-unit e) e))
          (repeat* (list) (max 1 (r-ceil (get r :len))))
          (get r :events)))

(def ev-unit (e) (idiv (r-num (get e :off)) (r-den (get e :off))))

;; an event outside the cycle's buckets makes them unusable (nil)
(def add-to-bucket (buckets u e)
  (if (and (not (= buckets nil)) (>= u 0) (< u (len buckets)))
      (set-nth buckets u (append (nth buckets u) (list e)))
      nil))

;; the events `emit-window` needs for unit u: its bucket when the cycle has
;; buckets (nil: an event outside them), else every event
(def window-events (r units u)
  (if (and (not (= units nil)) (>= u 0) (< u (len units)))
      (nth units u)
      (get r :events)))

;; length-only figure evaluation: cycle length depends on the symbolic-event
;; transforms, the time-mod, and alignment — never on hands, velocities, or
;; post ops. Skipping the full fold makes the pat-lens warm-up (pd cycles per
;; pattern, on the scheduler thread after a re-eval) cheap instead of an
;; audible pause. MUST stay in lockstep with eval-fig's duration math:
;; :fast expansion multiplies units by exactly m, so eff reduces to the
;; pre-expansion unit count.
(def fig-len (fig cycle off)
  (let ((evs1 (cut-tail (apply-xf-events (get fig :events) (get fig :xf) cycle) fig))
        (tm (tm-at (get fig :tm) cycle)))
    (let ((kind (if (= tm nil) nil (nth tm 0)))
          (m (if (= tm nil) 1 (max 1 (round-int (resolve-arg (nth tm 1) cycle))))))
      (let ((raw (units evs1)))
        (let ((eff (match kind
                     :fit (r-int m)
                     :fast (r-int raw)
                     :slow (r-int (* raw m))
                     _ (r-int raw))))
          (let ((al (get fig :align)))
            (if (= al nil)
                eff
                (let ((n (max 1 (round-int (resolve-arg (nth al 0) cycle))))
                      (total (r+ off eff)))
                  (r- (r-int (* n (iceil-div (r-ceil total) n))) off)))))))))

(def cycle-len-figs (figs cycle off)
  (if (empty? figs)
      off
      (cycle-len-figs (rest figs) cycle (r+ off (fig-len (first figs) cycle off)))))

;; integer length in units of one cycle (state-independent)
(def cycle-length (p k)
  (let ((key (list (get p :len-id) k)))
    (let ((hit (memo-find len-memo key)))
      (if (= hit nil)
          (let ((l (r->f (cycle-len-figs (cut-figs p k) k (r-int 0)))))
            (do (set! len-memo (cons (list key l) (take* 31 len-memo)))
                l))
          hit))))

;; ── cycle indexing: closed form over the length super-cycle (spec §8.1) ─────

(def arg-period (raw)
  (if (at-wrapped? raw)
      (arg-period (nth raw 2))
  (if (= raw nil)
      1
      (if (or (= (nth raw 0) 'cyc) (= (nth raw 0) 'seq))
          (let ((vals (if (= (nth raw 0) 'seq) (drop* 2 raw) (rest raw))))
            (* (max 1 (len vals)) (reduce (lambda (a x) (lcm* a (arg-period x))) 1 vals)))
          (if (implicit-cyc? raw)
              (* (max 1 (len raw)) (reduce (lambda (a x) (lcm* a (arg-period x))) 1 raw))
              1)))))

(def xf-period (f)
  (let ((h (first f)))
    (match h
      :every (lcm* (max 1 (round-int (resolve-arg (nth f 1) 0)))
                   (lcm* (arg-period (nth f 1)) (xf-period (nth f 2))))
      :alt (reduce (lambda (a x) (lcm* a (xf-period x)))
                   (max 1 (len (rest f))) (rest f))
      :fig-every (lcm* (arg-period (nth f 1)) (xf-period (nth f 2)))
      :on (lcm* (sel-period (nth f 1)) (xf-period (nth f 2)))
      :L (xf-period (nth f 1))
      :R (xf-period (nth f 1))
      _ (reduce (lambda (a x) (lcm* a (arg-period x))) 1 (rest f)))))

(def sel-period (s) (let ((pd (sel-period* s))) (if (> pd 0) (min 64 pd) 1)))

(def tm-period (tm)
  (if (= tm nil)
      1
      (if (= (first tm) :when)
          (lcm* (match (nth tm 1)
                  :every (max 1 (every-period (nth tm 2)))
                  :on (sel-period (nth tm 2))
                  _ (arg-period (nth tm 2)))
                (lcm* (tm-period (nth tm 4)) (tm-period (nth tm 5))))
          (arg-period (nth tm 1)))))

(def fig-period (f)
  (lcm* (tm-period (get f :tm))
        (lcm* (reduce (lambda (a x) (lcm* a (xf-period x))) 1 (get f :xf))
              (arg-period (if (= (get f :align) nil) nil (nth (get f :align) 0))))))

(def gtrunc-period (e)
  (if (= (first e) :all)
      (arg-period (nth e 1))
      (lcm* (max 1 (every-period (nth e 1))) (arg-period (nth e 2)))))

(def pat-period (p)
  (min 64 (max 1 (reduce (lambda (a e) (lcm* a (gtrunc-period e)))
                         (reduce (lambda (a f) (lcm* a (fig-period f))) 1 (get p :figs))
                         (or-default (get p :gtrunc) (list))))))

(def prefix-sum (lens k) (sum* (take* k lens)))

(def scan-cycle (lens rem k)
  (if (empty? (rest lens))
      k
      (if (< rem (first lens))
          k
          (scan-cycle (rest lens) (- rem (first lens)) (+ k 1)))))

;; per-pattern super-cycle table (lens total pd), memoized by pattern id.
;; Cycle lengths are state-independent, so the table is computed once per
;; pattern — without this, `locate` re-evaluated pd cycles per tick per route,
;; thrashing the small shared memos (rich patterns pinned the scheduler).
(def lens-memo (list))

(def pat-lens (p)
  (let ((hit (memo-find lens-memo (get p :len-id))))
    (if (= hit nil)
        (let ((pd (pat-period p)))
          (let ((lens (map (lambda (k) (cycle-length p k)) (range 0 pd))))
            (let ((entry (list lens (sum* lens) pd)))
              (do (set! lens-memo
                        (cons (list (get p :len-id) entry) (take* 23 lens-memo)))
                  entry))))
        hit)))

;; position (integer units) → (cycle-index cycle-start-unit)
(def locate (p pos)
  (let ((entry (pat-lens p)))
    (let ((lens (nth entry 0)) (total (nth entry 1)) (pd (nth entry 2)))
      (if (<= total 0)
          (list 0 pos)
          (let ((full (idiv pos total)))
            (let ((rem (- pos (* full total))))
              (let ((k (scan-cycle lens rem 0)))
                (list (+ (* full pd) k)
                      (+ (* full total) (prefix-sum lens k))))))))))

(def cycle-index (p pos) (first (locate p pos)))

;; ── generator wiring (spec §8.1): state cells + emission ────────────────────

(def hand->n (h) (if (= h :left) 0 1))
(def n->hand (n) (if (= n 0) :left :right))
(def b->n (b) (if b 1 0))
(def n->b (n) (not (= n 0)))

;; Threading state cells are keyed per pattern id: derived route patterns can
;; disagree about cycle structure (per-route fast/slow, trunc), and sharing
;; cells would make them fight over "which cycle are we in" every tick.
;; Structurally identical patterns thread to identical values independently.
(def cell (p name) (str name ":" (get p :id)))

(def load-state (p)
  (let ((st (mk-state (state-get (cell p "jaki-vel") 0.8)
                      (n->b (state-get (cell p "jaki-pwd") 0))
                      (state-get (cell p "jaki-streak") 0))))
    (if (has-procs? p) (merge st :procs (load-procs p)) st)))

(def store-state (p c hand st)
  (do (state-set! (cell p "jaki-cycle") c)
      (state-set! (cell p "jaki-hand") (hand->n hand))
      (state-set! (cell p "jaki-vel") (get st :cur))
      (state-set! (cell p "jaki-pwd") (b->n (get st :pwd)))
      (state-set! (cell p "jaki-streak") (get st :streak))
      (if (has-procs? p) (store-procs p st) nil)
      nil))

;; Row process chain state (spec §8): chain k's numbers live in scalar cells
;; jp<k>:n, jp<k>:hold, jp<k>:pend, jp<k>:acc<s> — state-set! stores numbers
;; only. `default-state` carries no :procs, so a jump stores fresh chains.
(def proc-cell (p k i)
  (cell p (str "jp" k ":" (if (= i 0) "n" (if (= i 1) "hold" (if (= i 2) "pend" (str "acc" (- i 3))))))))

(def load-procs (p)
  (let ((chains (get p :procs)))
    (map (lambda (k)
           (map (lambda (i) (state-get (proc-cell p k i) 0))
                (range 0 (chain-width (nth chains k)))))
         (range 0 (len chains)))))

(def store-procs (p st)
  (let ((cs (proc-states st (dict :chains (get p :procs)))))
    (map (lambda (k)
           (let ((s (nth cs k)))
             (map (lambda (i) (state-set! (proc-cell p k i) (nth s i)))
                  (range 0 (len s)))))
         (range 0 (len cs)))))

(def roll-state (p from to)
  (if (>= from to)
      nil
      (let ((r (eval-cycle p from (n->hand (state-get (cell p "jaki-hand") 0))
                           (load-state p))))
        (do (store-state p (+ from 1) (get r :end-hand) (get r :end-st))
            (roll-state p (+ from 1) to)))))

;; advance the threaded hand/velocity state to cycle c: contiguous advances
;; roll the ending state forward; jumps (transport relocation, generator
;; reset) restart from defaults — cycle indexing itself stays closed-form
(def ensure-state (p c)
  (let ((stored (state-get (cell p "jaki-cycle") -1)))
    (if (= stored c)
        nil
        (if (and (>= stored 0) (> c stored) (<= (- c stored) 8))
            (roll-state p stored c)
            ;; a jump: threading restarts, and so do seq counters (§7.2)
            (do (store-state p c :left default-state)
                (state-set! (cell p "jaki-seq-epoch") (+ 1 (seq-epoch p))))))))

;; idempotent per-tick setup: record the beat length of one unit
(def init (res) (do (state-set! "jaki-unit" (beats res)) nil))

;; ── clock (docs/jaki-trig-modes-spec.md) ─────────────────────────────────────
;; The unit position a tick plays. :loop and :gate run on the transport
;; (gen-tick, less the tick of the last restart); :retrig and :continue on a
;; playhead that advances only while another sequencer (a neuron routed here)
;; holds the gate open: :retrig rewinds it on every fire, :continue picks up
;; where it stopped. Everything downstream — locate, cycles, (every …),
;; threading — sees this position, so a gated pattern evolves only while it
;; plays. A rewind is a backward cycle jump, which ensure-state already treats
;; as a fresh start.

;; A tick that never ran step-clock (a hand-written def-sequencer calling
;; emit directly) plays at the transport position, as before modes.
(def play-pos ()
  (let ((tick (gen-tick)))
    (if (= (state-get "jaki-pos-tick" -1) tick) (state-get "jaki-pos" 0) tick)))

(def playhead-mode? (mode) (or (= mode :retrig) (= mode :continue)))

;; Set this tick's position and the gate's note/vel pass-through (:loop takes
;; none); true when the tick plays.
(def step-clock (mode)
  (let ((g (gen-gate)) (tick (gen-tick)))
    (let ((fired (> (get g :epoch) (state-get "jaki-gate-seen" 0)))
          (restart (> (get g :restart) (state-get "jaki-restart-seen" 0)))
          (gated (not (= mode :loop))))
      (do (state-set! "jaki-pos-tick" tick)
          (state-set! "jaki-gate-seen" (get g :epoch))
          (state-set! "jaki-restart-seen" (get g :restart))
          (state-set! "jaki-gnote" (if gated (get g :note) 0))
          (state-set! "jaki-gvel" (if gated (get g :vel) 1))
          (if (playhead-mode? mode)
              (let ((head (if (or restart (and fired (= mode :retrig)))
                              0
                              (state-get "jaki-play" 0))))
                (if (get g :open)
                    (do (state-set! "jaki-pos" head)
                        (state-set! "jaki-play" (+ head 1))
                        true)
                    (do (state-set! "jaki-play" head) false)))
              (let ((anchor (if restart tick (state-get "jaki-anchor" 0))))
                (do (state-set! "jaki-anchor" anchor)
                    (state-set! "jaki-pos" (- tick anchor))
                    (or (not gated) (get g :open)))))))))

(def reset ()
  (do (set! memo-store (list))
      (set! exact-memo (list))
      (set! len-memo (list))
      (set! lens-memo (list))
      (set! prepared-memo (list))
      (state-set! "jaki-cycle" -1)
      nil))

(def or-default (v d) (if (= v nil) d v))


;; opts stay raw and resolve per event: (every-fig n (note m)) differs per
;; figure within one cycle
;; ── value sequences: (seq :clock v…) (spec §7.2) ────────────────────────────
;; :hit / :fig / :span step only in per-hit values, resolved here at emit:
;; each value (a route's note/vel opt, or a deferred word) has a key and
;; counters in the generator state, advanced per emitted hit in time order and
;; restarted with the seq epoch when the transport jumps (ensure-state).

;; ── value picks (spec §7.4) ──
;; A list value of a per-hit word is wrapped (at PATH raw) at route build,
;; PATH its slot path in the row (item index first). Resolved at emit, it
;; records the path of the member that played in lit-picks; emit reports one
;; number per item so the kind panel lights that member.
(def sub-path (p i) (if (= p nil) nil (append p (list i))))

(def lit-raw (raw path)
  (if (or (= path nil) (= raw nil) (number? raw) (string? raw) (= (nth raw 0) nil))
      raw
      (list 'at path raw)))

(def at-wrapped? (raw)
  (and (not (= raw nil)) (not (number? raw)) (not (string? raw)) (= (nth raw 0) 'at)))

(def lit-picks (list))
(def lit-pick (here) (if (= here nil) nil (set! lit-picks (cons here lit-picks))))

;; a slot path below the item → one number: digit j (base 64) is element j + 1
(def path-code (rel) (if (empty? rel) 0 (+ (+ 1 (first rel)) (* 64 (path-code (rest rel))))))

(def ev-clock? (k) (or (= k :hit) (= k :fig) (= k :span)))

;; true when raw holds a seq whose clock needs the hit
(def needs-ev? (raw)
  (if (or (= raw nil) (number? raw) (string? raw) (= (nth raw 0) nil))
      false
      ;; (deg n) reads the chord declared at the hit: resolved at emit
      (if (or (and (= (nth raw 0) 'seq) (ev-clock? (nth raw 1))) (= (nth raw 0) 'deg))
          true
          (reduce (lambda (a x) (or a (needs-ev? x))) false raw))))

(def defer-or (kind plain raw)
  (if (or (needs-ev? raw) (at-wrapped? raw)) (list :defer kind raw nil) (list plain raw)))

(def seq-member (vals i) (nth vals (imod i (max 1 (len vals)))))

;; a :hit / :fig seq member steps its own nested :hit / :fig seqs once per
;; visit: visit v of member k is step v of whatever it holds. `here` is the
;; seq's slot path; its members start at element `off`.
(def seq-visit (vals n ctx here off)
  (let ((l (max 1 (len vals))))
    (let ((k (imod n l)))
      (resolve-ev (nth vals k)
                  (merge ctx :hit (idiv (round-int n) l) :figs (idiv (round-int n) l))
                  (sub-path here (+ off k))))))

;; per cycle: member (cycle mod n), its nested lists stepping per visit
;; (resolve-cycle-member)
(def pick-cycle (vals c ctx here off)
  (let ((n (max 1 (len vals))))
    (let ((k (imod c n)))
      (resolve-ev (nth vals k) (merge ctx :cycle (idiv c n)) (sub-path here (+ off k))))))

(def pick-member (vals i ctx here off)
  (let ((k (imod i (max 1 (len vals)))))
    (resolve-ev (nth vals k) ctx (sub-path here (+ off k)))))

;; ctx: (dict :cycle :fig :hit :figs :pos); here: raw's slot path or nil
(def resolve-ev (raw ctx here)
  (if (= raw nil)
      1
      (if (or (number? raw) (string? raw))
          (do (lit-pick here) raw)
          (let ((h (nth raw 0)) (c (get ctx :cycle)))
            (if (= h 'at)
                (resolve-ev (nth raw 2) ctx (nth raw 1))
            (if (= h 'seq)
                (let ((clock (nth raw 1)) (vals (drop* 2 raw)))
                  (match clock
                    :hit (seq-visit vals (get ctx :hit) ctx here 2)
                    :fig (seq-visit vals (get ctx :figs) ctx here 2)
                    :span (pick-member vals (floor (* (get ctx :pos) (max 1 (len vals)))) ctx here 2)
                    _ (pick-cycle vals c ctx here 2)))
                (if (= h 'cyc)
                    (pick-cycle (rest raw) c ctx here 1)
                    (if (= h 'every-gate)
                        (resolve-ev (if (every-active? (round-int (resolve-arg (nth raw 1) c)) c)
                                        (nth raw 2) (nth raw 3))
                                    ctx nil)
                        (if (= h 'fig-gate)
                            (resolve-ev (if (fig-index-active? (nth raw 1) (get ctx :fig) c)
                                            (nth raw 2) (nth raw 3))
                                        ctx nil)
                            (if (implicit-cyc? raw)
                                (pick-cycle raw c ctx here 0)
                                (if (= h 'deg)
                                    (deg-value (resolve-ev (nth raw 1) ctx (sub-path here 1)))
                                    (resolve-arg-at raw c (get ctx :fig)))))))))))))

;; (deg n) at emit: degree n of the chord the `-> chords` row declared (root
;; plus the degree's interval); before any declaration, over C major
(def deg-value (n)
  (let ((f (alez.jaki.chords/field-get "chords")))
    (if (= f nil)
        (alez.jaki.chords/deg-interval :maj n)
        (+ (get f :root) (alez.jaki.chords/deg-interval (get f :q) n)))))

(def seq-epoch (p) (state-get (cell p "jaki-seq-epoch") 0))

;; advance value `key`'s counters for hit e and return its resolution context:
;; :hit counts this value's hits, :figs its figure occurrences (it moves on
;; when the hit's cycle or figure differs from the previous hit's)
;; The epoch a value's counters belong to lives beside them (":e"), not in
;; the key: a jump restarts them in place on first use, so rewinds (every
;; :retrig fire) never grow the generator's state with stale key sets.
(def ev-ctx (p key e c clen)
  (let ((k (str "seq:" (get p :id) ":" key)) (ep (seq-epoch p)))
    (let ((fresh (not (= (state-get (str k ":e") -1) ep))))
    (let ((n (if fresh 0 (state-get (str k ":h") 0)))
          (last (if fresh -1 (state-get (str k ":l") -1)))
          (m (if fresh 0 (state-get (str k ":f") 0)))
          (cur (+ (* c 4096) (get e :fig))))
      (let ((m2 (if (= cur last) m (+ m 1))))
        (do (state-set! (str k ":e") ep)
            (state-set! (str k ":h") (+ n 1))
            (state-set! (str k ":l") cur)
            (state-set! (str k ":f") m2)
            (dict :cycle c :fig (get e :fig) :hit n :figs (- m2 1)
                  :pos (if (> clen 0) (/ (r->f (get e :off)) clen) 0))))))))

;; value raw for hit e: stepped by its clock when it has one, else the
;; per-cycle resolution; d when raw is nil
(def ev-value (p key raw e c clen d)
  (if (= raw nil)
      d
      (if (needs-ev? raw)
          (resolve-ev raw (ev-ctx p key e c clen) nil)
          (if (at-wrapped? raw)
              (resolve-ev raw (dict :cycle c :fig (get e :fig) :hit 0 :figs 0 :pos 0) nil)
              (resolve-arg-at raw c (get e :fig))))))

(def apply-defer (acc d p rkey e c clen)
  (let ((v (ev-value p (str rkey (nth d 2)) (nth d 1) e c clen 0)))
    (match (nth d 0)
      :vmul (merge acc :vel (clamp01 (* (get acc :vel) v)))
      :vadd (merge acc :vel (clamp01 (+ (get acc :vel) v)))
      :nadd (merge acc :nadd (+ (get acc :nadd) v))
      :nset (merge acc :nset v)
      :gmul (merge acc :gmul (* (get acc :gmul) (max 0 v)))
      :plock (merge acc :params (plock-set (get acc :params) (nth d 3) v))
      _ acc)))

;; opts stay raw and resolve per event: (every-fig n (note m)) differs per
;; figure within one cycle, (seq :hit …) per hit. Deferred words apply after
;; the ones applied during evaluation.
(def emit-one (p e u track opts c clen)
  (let ((unit (state-get "jaki-unit" 0.25)) (rkey (or-default (get opts :rkey) ""))
        (reset (set! lit-picks (list))))
    (let ((a (reduce (lambda (acc d) (apply-defer acc d p rkey e c clen))
                     (dict :vel (get e :vel) :nadd (or-default (get e :nadd) 0)
                           :nset (get e :nset) :gmul 1 :params (get e :params))
                     (or-default (get e :defer) (list))))
          (opt-note (ev-value p (str rkey "opt:note") (get opts :note) e c clen 0))
          (opt-vel (ev-value p (str rkey "opt:vel") (get opts :vel-scale) e c clen 1))
          ;; the hit's chord (a per-hit (on … (chord X)) first), resolved
          ;; before the lit marks so the member playing lights up
          (chord-q (let ((raw (or-default (get e :chordv) (get opts :chord))))
                     (if (= raw nil) nil (ev-value p (str rkey "opt:chord") raw e c clen :maj)))))
      (let ((at (* (r->f (r- (get e :off) (r-int u))) unit))
            (dur (* (* (r->f (get e :gate)) unit) (get a :gmul))))
      (do
        ;; the row items applied to this hit, lit for its length (§7.3)
        (if (get opts :lights)
            (do (gen-mark (lit-mask (or-default (get e :lits) (list))) (get opts :rindex) at)
                (gen-mark 0 (get opts :rindex) (+ at dur)))
            nil)
        ;; the list members this hit played, one mark per item (§7.4)
        (map (lambda (path)
               (gen-mark (path-code (rest path)) (str (get opts :rindex) "." (first path)) at))
             lit-picks)
      (let ((base (let ((ns (get a :nset))) (if (= ns nil) opt-note ns))))
        ;; a gating fire's payload rides on top of everything the row
        ;; computes: its velocity scales, its note adds after an absolute
        ;; (note …) set (docs/jaki-trig-modes-spec.md §3). A (voice) row
        ;; first moves its note to the nearest free declared chord tone.
        (emit-notes chord-q c opts track at dur
                    (* (* (get a :vel) opt-vel) (state-get "jaki-gvel" 1))
                    (snap-hit e (+ (+ (if (get opts :voice)
                                          (alez.jaki.chords/voice-pick
                                            rkey base (alez.jaki.chords/field-get "chords") (gen-tick)
                                            (ev-value p (str rkey "opt:voice") (get opts :voice-offset) e c clen 0))
                                          base)
                                      (get a :nadd))
                                   (state-get "jaki-gnote" 0))
                              c)
                    ;; (plock …) values, a flat label/value list (nil: none)
                    (get a :params))))))))

;; A (chord X) value: chord words (Am7, iv7, V7/iv) become codes
;; (alez.jaki.chords/chord-code) so seq / per-cycle lists carry them;
;; nothing, :inv or :declared means the declared chord; a list of plain
;; qualities (:min7 :dom7) is one per cycle.
(def chord-value (q)
  (if (or (= q nil) (= q :inv) (= q :declared))
      :field
      (let ((v (alez.jaki.chords/encode-raw q)))
        ;; a lit-wrapped value (at PATH raw) keeps its wrapper
        (if (at-wrapped? v)
            (list 'at (nth v 1) (cyc-list (nth v 2)))
            (cyc-list v)))))

;; a list headed by a keyword (:min7 :dom7) is not an implicit cyc: mark it
(def cyc-list (v)
  (if (and (not (number? v)) (not (= (nth v 0) nil))
           (not (member? (nth v 0) '(seq cyc at))) (not (number? (nth v 0))))
      (cons 'cyc v)
      v))

;; One hit's sound (docs/harmony-declaration-spec.md): a `-> chords` row
;; declares (and with :play sounds) the chord rooted on the hit's note; a
;; (chord Q) row plays chord Q on it, a bare (chord) the declared chord
;; transposed by it; any other row plays the one note.
(def emit-notes (q c opts track at dur vel note params)
  (let ((inv (resolve-arg (or-default (get opts :chord-inv) 0) c)))
    (do
      ;; a chord word's code → its root (on the key, for numerals) plus the
      ;; hit's note, so note / note+ transpose the progression
      (let ((code (alez.jaki.chords/decode-code q (or-default (get opts :key-root) 0))))
        (let ((root (if (= code nil) note (+ (first code) note)))
              (qual (if (= code nil) q (nth code 1))))
          (if (string? (get opts :declare))
              (let ((qd (if (or (= qual nil) (= qual :field)) :maj qual)))
                (do (alez.jaki.chords/field-set! (get opts :declare) root qd)
                    (gen-mark (alez.jaki.chords/field-code root qd) "chord" at)
                    (if (= (get opts :play) nil)
                        nil
                        (emit-bundle (round-int (resolve-arg (get opts :play) c)) at vel dur params
                                     (alez.jaki.chords/chord-notes root qd inv)))))
              (if (= qual nil)
                  (seq-emit :track track :at at :vel vel :note note :dur dur :params params)
                  (emit-bundle track at vel dur params
                    (if (= qual :field)
                        (let ((f (alez.jaki.chords/field-get "chords")))
                          (if (= f nil)
                              (list note)
                              (alez.jaki.chords/chord-notes (+ (get f :root) note) (get f :q) inv)))
                        (alez.jaki.chords/chord-notes root qual inv))))))))))

(def emit-bundle (track at vel dur params notes)
  (map (lambda (n) (seq-emit :track track :at at :vel vel :note n :dur dur :params params))
       notes))

;; ── note ops at emit (docs/jaki-row-processes-spec.md §13) ──
;; A hit tagged by (harmony :track n :amount a), (scale :minor :root C) or
;; (snap "chan") — route words, `on` words, or chain actions — has its final
;; transpose moved here, in that order: harmony holds it to another track's
;; chord and key at strictness a (lane-harmony's tiers, via harmonic-snap on
;; gen-track-harmony), then a scale or a channel's pitch-class mask moves it
;; to the nearest allowed pitch class (octave kept, ties downward). No read,
;; an empty mask: the note stays.
(def pc-bit? (mask pc) (= 1 (imod (idiv mask (pow2 pc)) 2)))

(def snap-walk (r m d)
  (if (> d 6)
      r
      (if (pc-bit? m (imod (- r d) 12))
          (- r d)
          (if (pc-bit? m (imod (+ r d) 12)) (+ r d) (snap-walk r m (+ d 1))))))

(def snap-note (n mask)
  (let ((m (if (number? mask) (imod (round-int mask) 4096) 0)))
    (if (= m 0) n (snap-walk (round-int n) m 0))))

(def harm-note (note h c)
  (let ((r (gen-track-harmony (round-int (resolve-arg (nth h 0) c)))))
    (if (= r nil)
        note
        (+ note (harmonic-snap (nth r 0) (nth r 1) note (resolve-arg (nth h 1) c) 0)))))

;; `c` is the hit's cycle: (harmony :track … :amount …) args cycle like
;; every other slot
(def snap-hit (e note c)
  (let ((n1 (let ((h (get e :harm))) (if (= h nil) note (harm-note note h c)))))
    (let ((n2 (let ((m (get e :smask))) (if (= m nil) n1 (snap-note n1 m)))))
      (let ((ch (get e :snap))) (if (string? ch) (snap-note n2 (chan-get ch 0)) n2)))))

;; item indices → bitmask (duplicates count once)
(def pow2 (i) (if (<= i 0) 1 (* 2 (pow2 (- i 1)))))
(def lit-mask (idxs)
  (reduce (lambda (m i) (+ m (pow2 i)))
          0 (reduce (lambda (acc i) (if (member? i acc) acc (cons i acc))) (list) idxs)))

;; emit every event whose offset falls in this tick's unit window [u, u+1) —
;; exact rational membership, no epsilon (spec §8.3)
(def emit-window (p evs u track opts c clen)
  (reduce
    (lambda (n e)
      (if (and (r<= (r-int u) (get e :off))
               (r< (get e :off) (r-int (+ u 1))))
          (do (emit-one p e u track opts c clen) (+ n 1))
          n))
    0 evs))

;; evaluate the pattern for the current tick's cycle and emit this window's
;; events; returns the number of events emitted. `track` and the emit opts
;; (:note, :vel-scale) are raw per-cycle argument data — numbers, or (cyc …)
;; to cycle the destination / transpose / velocity scale per cycle.
(def emit* (p track opts)
  (let ((tick (play-pos)))
    (let ((loc (locate p tick)))
      (let ((c (first loc)) (cstart (nth loc 1)))
        (do (ensure-state p c)
            (let ((entry (eval-cycle-entry p c (n->hand (state-get (cell p "jaki-hand") 0))
                                           (load-state p))))
              (let ((r (first entry)))
                (emit-window p (window-events r (nth entry 1) (- tick cstart)) (- tick cstart)
                             (round-int (resolve-arg track c))
                             opts c (r->f (get r :len))))))))))

(def emit (p track) (emit* p track (dict)))

;; ── tier-2 route surface: (jak "name" :res events… -> track words…) ────────
;;
;; The `jak` macro exported by alez.jaki.surface expands to a def-sequencer
;; whose tick calls (alez.jaki.core/run body).
;; `run` interprets the body data: segments split at `->` symbols — segment
;; zero is the pattern (same grammar as alez.jaki.core/pat), each later segment is
;; `track word…`. Route words:
;;   left right accent rev stac ghost swap
;;   (shift n) (rot n) (trunc n) (every n t) (for-hand h t)
;;   (every-fig n t) — figure transform t on every nth figure of the cycle
;;   (fast n) (slow n) — n may be (cyc …) for conditional retiming
;;   Any per-cycle arg also reads Tidal-style implicit cyc: a list headed by
;;   a value is (cyc …) recursively — (1 2 (1 3)) = (cyc 1 2 (cyc 1 3))
;;   Whole WORDS alternate the same way: (right right right left) picks one
;;   modifier per cycle ((cyc w…) is the explicit spelling, `id` the no-op,
;;   `none`/`rest` the silent cycle — also useful as (every n rest));
;;   members must all be post-lowerable or all figure transforms
;;   (vel s) (note n)
;;   (plock NAME v) — per-hit parameter lock, NAME a macro-editor label
;;   string ("instrument:cutoff"); v any per-hit value, e.g. (seq :hit …)
;;   any figure transform: (basevel v) (dotdecay v) (dashdecay v) (minvel v)
;;   (maxvel v) (split t) (merge t) (L w) (R w)
;;   (gate s) / (dur s) — multiply every gate by s (per-cycle arg: number,
;;   (cyc …), (chan …)); applies in authored word order like stac
;;   (scale :minor :root C) — hold every note to a scale
;;   (harmony :track n :amount a) — hold every note to track n's chord/key
;;   (snap "chan") — snap every hit's note to the pitch classes the channel
;;   holds (alez.jaki.harmony publishes a track's chord there)
;;   (rule TRIGGER STAGE… ACTION…) — a rule run on the hits TRIGGER selects
;;   (docs/jaki-row-processes-spec.md; `proc` is the old name); (seed n)
;;   rerolls its coins
;; A route containing a (mute T) or (solo T) form — T a track number or
;; (group "name") — is a CONTROL route: events become timed mute/solo holds
;; instead of notes, and the route word `inv` complements the windows
;; (docs/jaki-mixer-control-routes-spec.md). The control form may sit
;; anywhere in the segment. Targets are per-cycle argument data like every
;; other route arg: (mute (cyc 1 2)) and (solo (group (cyc "Drums" "Synths")))
;; rotate the destination per cycle.
;; Multi-voice: when the first body element is a list containing a top-level
;; `->`, every element is one voice line with its own pattern and routes.

;; quant grid in units, as an exact rational: (beats tb)/(jaki unit),
;; rationalized over 96 so straight and triplet timebase ratios stay exact
;; (e.g. :16t on a :16 jak → 2/3). Resolved at route time so the memoized
;; evaluator never depends on the generator's resolution.
(def quant-units (tb)
  (let ((u (state-get "jaki-unit" 0.25)))
    (rat (round-int (* (/ (beats tb) u) 96)) 96)))

;; (plock NAME V) (docs/jaki-plock-spec.md §4): a per-hit parameter lock.
;; NAME is a macro-editor label string ("instrument:cutoff",
;; "fx2:filterbank:freq", "rack-macro:macro_1"), checked ONCE here with the
;; host's `param-label?` — a label seq-emit would reject (which fails the
;; whole call, silencing the note) lowers to the no-op (:id), so a typo
;; drops only the lock. V is any per-hit value, like note's: a clocked
;; value defers to emit as (:defer :plock raw key label).
(def plock-op (args wp)
  (let ((label (nth args 0)) (raw (nth args 1)))
    (if (and (string? label) (and (not (= raw nil)) (param-label? label)))
        (let ((v (lit-raw raw (sub-path wp 2))))
          (if (or (needs-ev? v) (at-wrapped? v))
              (list :defer :plock v nil label)
              (list :plock label v)))
        (list :id))))

;; a hit's locks: flat (label value …), the list seq-emit :params takes; a
;; later lock of the same label replaces the earlier one; non-numbers drop
(def plock-index (l label i)
  (if (>= i (len l)) -1 (if (= (nth l i) label) i (plock-index l label (+ i 2)))))

(def plock-set (l label v)
  (if (not (number? v))
      l
      (if (= l nil)
          (list label v)
          (let ((i (plock-index l label 0)))
            (if (>= i 0) (set-nth l (+ i 1) v) (append l (list label v)))))))

;; route words that lower to post ops, so `(every n w)` can wrap them
;; cycle-gated while staying in authored word order; nil for xf-able words.
;; An alternation lowers to (:alt post…) only when EVERY member lowers, so a
;; mixed alternation can still fall back to the xf path as a whole.
;; (route-post-op-at w wp): wp is w's slot path in its row item (or nil), so
;; a list value is wrapped (at path raw) and reports the member it plays
;; (spec §7.4)
(def route-post-op (w) (route-post-op-at w nil))

(def route-post-op-at (w wp)
  (if (alt-word? w)
      (let ((posts (map route-post-op (alt-members w))))
        (if (member? nil posts) nil (cons :alt posts)))
      (let ((h (raw-head w)) (args (raw-args w)) (v (lit-raw (nth (raw-args w) 0) (sub-path wp 1))))
        (match h
          'stac   (list :stac)
          'id     (list :id)
          'none   (list :none)
          'rest   (list :none)
          'gate   (defer-or :gmul :gate v)
          'dur    (defer-or :gmul :gate v)
          'quant  (list :quant (quant-units (nth args 0)))
          'shift  (list :shift (nth args 0))
          'left   (list :filter '(:hand :left))
          'right  (list :filter '(:hand :right))
          'accent (list :filter '(:accent true))
          'vel*   (defer-or :vmul :vmul v)
          'vel+   (defer-or :vadd :vadd v)
          'note+  (defer-or :nadd :nadd v)
          'plock  (plock-op args wp)
          'snap   (list :snap (nth args 0))
          ;; (on accent (chord E7)) / (every 4 (chord F)): a per-hit chord
          'chord  (list :chordv (chord-value (lit-raw (nth args 0) (sub-path wp 1))))
          'scale  (let ((op (note-op w))) (if (= op nil) nil (list :noteop op)))
          'harmony (list :noteop (note-op w))
          'every  (let ((inner (route-post-op-at (nth args 1) (sub-path wp 2))))
                    (if (= inner nil)
                        nil
                        (list :every (nth args 0) inner)))
          _ nil))))

(def split-arrows (l cur acc)
  (if (empty? l)
      (append acc (list cur))
      (if (= (first l) '->)
          (split-arrows (rest l) (list) (append acc (list cur)))
          (split-arrows (rest l) (append cur (list (first l))) acc))))

;; acc = (dict :p pattern :opts emit-opts); w is one route word (data).
;; An alternation list picks one member per cycle: post-lowerable members
;; become an (:alt post…) op in authored word order; otherwise the whole
;; alternation lowers to an (:alt xf…) figure transform. Members must all be
;; one kind — a mix of post-only and xf-only words is ignored like any
;; unknown word.
(def route-step (acc w)
  (if (alt-word? w)
      (let ((post (route-post-op w)))
        (if (not (= post nil))
            (merge acc :p (add-post (get acc :p) post (str "alt:" (source w))))
            (if (= (norm-xf w) nil)
                acc
                (merge acc :p (xform (get acc :p) w)))))
      (route-step-word acc w)))

;; (every n w) / (every-fig n w): w is any route word. Retiming words gate the
;; figures' time-mod, note/vel gate the emit opt, post-lowerable words become
;; a gated post op, and figure transforms a gated xf. kind is :every (on the
;; cycle) or :fig (on each figure's index).
(def gate-opt (opts key kind n m d)
  (merge opts key (list (if (= kind :every) 'every-gate 'fig-gate) n m
                        (or-default (get opts key) d))))

(def route-gated (acc kind n w)
  (let ((h (raw-head w)) (p (get acc :p)) (wp (sub-path (get acc :wpath) 2)))
   (let ((m (lit-raw (nth (raw-args w) 0) (sub-path wp 1))))
    (match h
      'fast (merge acc :p (when-retime p kind n :fast (nth (raw-args w) 0)))
      'slow (merge acc :p (when-retime p kind n :slow (nth (raw-args w) 0)))
      ;; (every k (trunc n)) cuts the whole cycle; every-fig cuts its figures
      'trunc (merge acc :p (if (= kind :every)
                               (add-gtrunc p (list :every n (nth (raw-args w) 0)))
                               (every-fig p n w)))
      'note (merge acc :opts (gate-opt (get acc :opts) :note kind n m 0))
      'vel  (merge acc :opts (gate-opt (get acc :opts) :vel-scale kind n m 1))
      _ (let ((post (route-post-op-at w wp)))
          (if (= post nil)
              (merge acc :p (if (= kind :every) (every p n w) (every-fig p n w)))
              (merge acc :p (add-post p
                                      (list (if (= kind :every) :every :fig-every) n post)
                                      (str (if (= kind :every) "every:" "fe:")
                                           (source n) ":" (source post))))))))))

(def route-step-word (acc w)
  (let ((h (raw-head w)) (args (raw-args w)))
    (match h
      'left   (merge acc :p (filter (get acc :p) '(:hand :left)))
      'right  (merge acc :p (filter (get acc :p) '(:hand :right)))
      'accent (merge acc :p (filter (get acc :p) '(:accent true)))
      'rev    (merge acc :p (rev (get acc :p)))
      'stac   (merge acc :p (add-post (get acc :p) (list :stac) "stac"))
      'none   (merge acc :p (add-post (get acc :p) (list :none) "none"))
      'rest   (merge acc :p (add-post (get acc :p) (list :none) "none"))
      'ghost  (merge acc :p (ghost (get acc :p)))
      'swap   (merge acc :p (swap (get acc :p)))
      'rot    (merge acc :p (rot (get acc :p) (nth args 0)))
      'trunc  (merge acc :p (trunc (get acc :p) (nth args 0)))
      'shift  (merge acc :p (shift (get acc :p) (nth args 0)))
      'fast   (merge acc :p (fast (get acc :p) (nth args 0)))
      'slow   (merge acc :p (slow (get acc :p) (nth args 0)))
      'gate   (merge acc :p (add-post (get acc :p) (route-post-op-at w (get acc :wpath))
                                      (str "gate:" (source (nth args 0)))))
      'dur    (merge acc :p (add-post (get acc :p) (route-post-op-at w (get acc :wpath))
                                      (str "gate:" (source (nth args 0)))))
      'quant  (let ((q (quant-units (nth args 0))))
                (merge acc :p (add-post (get acc :p) (list :quant q)
                                        (str "quant:" (source q)))))
      'every  (add-tag (route-gated acc :every (nth args 0) (nth args 1))
                       (list :every (nth args 0)))
      'every-fig (add-tag (route-gated acc :fig (nth args 0) (nth args 1))
                          (list :fig-every (nth args 0)))
      'for-hand (merge acc :p (for-hand (get acc :p) (nth args 0) (nth args 1)))
      'vel    (merge acc :opts (merge (get acc :opts)
                                  :vel-scale (lit-raw (nth args 0) (sub-path (get acc :wpath) 1))))
      'note   (merge acc :opts (merge (get acc :opts)
                                  :note (lit-raw (nth args 0) (sub-path (get acc :wpath) 1))))
      ;; (chord Q [:inv n]): on a track row, each hit plays chord Q on its
      ;; note; bare (chord) plays the declared chord. On a `-> chords` row,
      ;; Q is the quality the hit declares. Q is per-hit value data like
      ;; note's: a keyword, (:min7 :dom7) per cycle, (seq :fig …).
      'chord  (let ((i (index-of* args :inv 0)))
                (merge acc :opts (merge (get acc :opts)
                                   :chord (chord-value (lit-raw (nth args 0) (sub-path (get acc :wpath) 1)))
                                   :chord-inv (if (>= i 0) (nth args (+ i 1)) 0))))
      ;; (key A :minor): the tonic the lane's Roman numerals count from
      'key    (merge acc :opts (merge (get acc :opts)
                                 :key-root (or-default (alez.jaki.chords/parse-root (nth args 0)) 0)
                                 :key-mode (or-default (nth args 1) :major)))
      ;; (voice): move to the nearest free tone of the declared chord
      ;; (voice n): n chord tones from it (a per-hit value: (seq :hit 0 1 2))
      'voice  (merge acc :opts (merge (get acc :opts)
                                 :voice true :voice-offset (or-default (nth args 0) 0)))
      'vel*   (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'vel+   (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'note+  (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'plock  (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'snap   (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'harmony (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'scale  (let ((post (route-post-op-at w (get acc :wpath))))
                (if (= post nil) acc (add-route-post acc post w)))
      'on     (route-on acc (nth args 0) (rest args))
      'inv    (merge acc :inv true)
      'rule   (merge acc :p (add-proc (get acc :p) args))
      'proc   (merge acc :p (add-proc (get acc :p) args))
      'seed   (let ((p (get acc :p)))
                (merge acc :p (merge p :seed (nth args 0)
                                       :id (str (get p :id) "|seed:" (source (nth args 0))))))
      ;; Any other figure transform (basevel dotdecay dashdecay minvel
      ;; maxvel split merge L R …) applies to every figure of the route.
      _ (if (= (norm-xf w) nil) acc (merge acc :p (xform (get acc :p) w))))))

(def add-route-post (acc post w)
  (merge acc :p (add-post (get acc :p) post (source w))))

;; (on SEL word…) (spec §7.1): each word applies only where SEL holds. Event
;; words (post ops, vel*/vel+/note+, and vel/note, which become per-event
;; vel*/note-set here) take any selector; structural words (fast/slow and
;; figure transforms) need a figure-level selector and are ignored otherwise.
;; A nested (on S2 w) or (every n w) narrows the selector.
(def route-on (acc sel-raw words)
  (let ((sel (norm-sel sel-raw (get (get acc :p) :figs))))
    (if (= sel nil)
        acc
        (on-words acc sel words (get acc :wpath)))))

;; the words of an (on SEL w…) at slot path wp: word j sits at wp + (2 + j)
(def on-words (acc sel words wp)
  (reduce (lambda (a j) (route-on-word a sel (nth words j) (sub-path wp (+ 2 j))))
          acc (range 0 (len words))))

(def on-tag (sel w) (str "|on:" (source sel) ":" (source w)))

(def on-post (acc sel post w)
  (add-tag (merge acc :p (add-post (get acc :p) (list :on sel post)
                                   (str "on:" (source sel) ":" (source w))))
           sel))

(def route-on-word (acc sel w wp)
  (let ((h (raw-head w)) (args (raw-args w)) (p (get acc :p))
        (figure? (not (sel-event? sel))) (v (lit-raw (nth (raw-args w) 0) (sub-path wp 1))))
    (if (alt-word? w)
        (let ((post (route-post-op w)))
          (if (not (= post nil))
              (on-post acc sel post w)
              (let ((x (norm-xf w)))
                (if (and figure? (not (= x nil)))
                    (add-tag (merge acc :p (on-xf p sel x (on-tag sel w))) sel)
                    acc))))
        (match h
          'on (let ((inner (norm-sel (nth args 0) (get p :figs))))
                (if (= inner nil)
                    acc
                    (on-words acc (list :and sel inner) (rest args) wp)))
          'every (if (> (len args) 1)
                     (route-on-word acc (list :and sel (list :every (nth args 0))) (nth args 1)
                                    (sub-path wp 2))
                     acc)
          'fast (if figure? (add-tag (merge acc :p (on-retime p sel :fast (nth args 0) (on-tag sel w))) sel) acc)
          'slow (if figure? (add-tag (merge acc :p (on-retime p sel :slow (nth args 0) (on-tag sel w))) sel) acc)
          'vel  (on-post acc sel (defer-or :vmul :vmul v) w)
          'note (on-post acc sel (defer-or :nset :nset v) w)
          _ (let ((post (route-post-op-at w wp)))
              (if (not (= post nil))
                  (on-post acc sel post w)
                  (let ((x (norm-xf w)))
                    (if (and figure? (not (= x nil)))
                        (add-tag (merge acc :p (on-xf p sel x (on-tag sel w))) sel)
                        acc))))))))

;; UI preview (the jaki kind's hit strip): `cycles` cycles of the pattern
;; body from cycle `start`, hand and velocity threaded from the defaults, each
;; event tagged with its :cycle.
(def preview (body start cycles)
  (preview-walk (from-list body) start (+ start cycles) :left default-state (list)))

;; the same window with a row's (proc …) words and seed applied (the kind's
;; row process editor, docs/jaki-row-processes-spec.md §9): the chains only,
;; no other route words, so it needs no route preparation. Hits the chains
;; drop are absent. State starts fresh at `start`, so a long-running
;; accumulator can differ from what is playing: a preview, not a meter.
(def preview-procs (body words seed start cycles)
  (let ((p (reduce (lambda (p w) (add-proc p (rest w))) (from-list body) words)))
    (preview-walk (merge p :seed seed) start (+ start cycles) :left default-state (list))))

(def preview-walk (p c end hand st acc)
  (if (>= c end)
      acc
      (let ((r (eval-at p c hand st)))
        (preview-walk p (+ c 1) end (get r :end-hand) (get r :end-st)
          (append acc (map (lambda (e) (merge e :cycle c)) (get r :events)))))))

;; function form: (alez.jaki.core/on p 'left '((vel* 0.5) stac))
(def on (p sel words)
  (get (route-on (dict :p p :opts (dict)) sel words) :p))

;; words stepped with :widx, the word's index in the route (= the row item
;; index in the jaki kind), which tag ops report
(def route-steps (acc words)
  (reduce (lambda (a i) (route-step (merge a :widx i :wpath (list i)) (nth words i)))
          acc (range 0 (len words))))

;; (:tag sel idx): hits sel matches report item idx as applied (§7.3)
(def add-tag (acc sel)
  (let ((idx (get acc :widx)))
    (if (= idx nil)
        acc
        (merge acc :lights true
                   :p (add-post (get acc :p) (list :tag sel idx) (str "tag:" idx ":" (source sel)))))))

;; `-> chords` / `-> (chords "name" [:play track])`: the row declares the
;; chord instead of playing a track (alez.jaki.chords). :play also sounds the
;; declared chord on that track.
(def chords-dest? (d)
  (and (not (number? d)) (not (string? d)) (= (raw-head d) 'chords)))

(def prepare-route (p seg)
  (let ((r (route-steps (dict :p p :opts (dict)) (rest seg))) (dest (first seg)))
    (let ((opts (merge (get r :opts) :lights (= (get r :lights) true))))
      (if (chords-dest? dest)
          (let ((a (raw-args dest)))
            (let ((i (index-of* a :play 0)))
              (dict :kind :note :p (with-period (get r :p))
                    :track (if (>= i 0) (nth a (+ i 1)) 0)
                    :opts (merge opts :declare (if (string? (nth a 0)) (nth a 0) "chords")
                                      :play (if (>= i 0) (nth a (+ i 1)) nil)))))
          (dict :kind :note :p (with-period (get r :p)) :track dest :opts opts)))))

;; ── control routes: -> (mute T) / (solo T) — sequenced mixer holds ─────────
;; (docs/jaki-mixer-control-routes-spec.md). Events become gate windows
;; (union of [off, off+gate)); `inv` complements them within the cycle; each
;; window starting in this tick's unit window is emitted as one
;; seq-emit-control hold. All pattern-transforming route words compose;
;; filters extend gates legato-style, so `left stac` gives short punches.

(def control-route-head? (x)
  (let ((h (raw-head x))) (or (= h 'mute) (= h 'solo))))

;; (mute 3) / (solo (group "Drums")) → (dict :op :kind :track-raw|:name-raw).
;; Targets stay RAW per-cycle argument data — a number/string, (cyc …), or an
;; expression — resolved by resolve-arg once the tick's cycle is located, so
;; (mute (cyc 1 2)) and (group (cyc "Drums" "Synths")) rotate per cycle.
(def parse-control-target (form)
  (let ((op (if (= (raw-head form) 'mute) "mute" "solo"))
        (tgt (nth form 1)))
    (if (= (raw-head tgt) 'group)
        (dict :op op :kind :group :name-raw (nth tgt 1))
        (dict :op op :kind :track :track-raw tgt))))

(def resolve-control-spec (spec c)
  (if (= (get spec :kind) :group)
      (merge spec :name (resolve-arg (get spec :name-raw) c))
      (merge spec :track (round-int (resolve-arg (get spec :track-raw) c)))))

;; sorted [start end) rational intervals → union-merged intervals
(def merge-windows (ivs)
  (reduce
    (lambda (acc iv)
      (if (empty? acc)
          (list iv)
          (let ((prev (last* acc)))
            (if (r<= (first iv) (nth prev 1))
                (append (take* (- (len acc) 1) acc)
                        (list (list (first prev)
                                    (if (r< (nth prev 1) (nth iv 1))
                                        (nth iv 1)
                                        (nth prev 1)))))
                (append acc (list iv))))))
    (list) ivs))

;; evaluated (sorted) events → merged gate windows, clamped to [0, total)
(def event-windows (evs total)
  (merge-windows
    (map (lambda (e)
           (let ((end (r+ (get e :off) (get e :gate))))
             (list (get e :off) (if (r< total end) total end))))
         evs)))

(def invert-walk (wins cursor total acc)
  (if (empty? wins)
      (if (r< cursor total) (append acc (list (list cursor total))) acc)
      (let ((w (first wins)))
        (invert-walk (rest wins)
                     (if (r< cursor (nth w 1)) (nth w 1) cursor)
                     total
                     (if (r< cursor (first w))
                         (append acc (list (list cursor (first w))))
                         acc)))))

;; complement of the merged windows within [0, total)
(def invert-windows (wins total) (invert-walk wins (r-int 0) total (list)))

(def emit-control-one (w u unit spec)
  (let ((at (* (r->f (r- (first w) (r-int u))) unit))
        (dur (* (r->f (r- (nth w 1) (first w))) unit)))
    (if (= (get spec :kind) :group)
        (seq-emit-control :op (get spec :op) :group (get spec :name)
                          :at at :dur dur)
        (seq-emit-control :op (get spec :op) :track (get spec :track)
                          :at at :dur dur))))

;; emit every window whose START falls in this tick's unit window [u, u+1);
;; the hold carries its full duration even when it extends past the window
(def emit-window-controls (wins u spec)
  (let ((unit (state-get "jaki-unit" 0.25)))
    (reduce
      (lambda (n w)
        (if (and (r<= (r-int u) (first w))
                 (r< (first w) (r-int (+ u 1)))
                 (r< (first w) (nth w 1)))
            (do (emit-control-one w u unit spec) (+ n 1))
            n))
      0 wins)))

(def prepare-control-route (p0 target-form words)
  (let ((r (route-steps (dict :p p0 :opts (dict)) words)))
    (dict :kind :control :p (with-period (get r :p)) :inv (get r :inv)
          :spec (parse-control-target target-form))))

(def play-control-route (route)
  (let ((p (get route :p))
        (tick (play-pos)))
    (let ((loc (locate p tick)))
      (let ((c (first loc)) (cstart (nth loc 1)))
        (do (ensure-state p c)
            (let ((res (eval-cycle p c (n->hand (state-get (cell p "jaki-hand") 0))
                                   (load-state p))))
              (let ((wins0 (event-windows (get res :events) (get res :len))))
                (emit-window-controls
                  (if (get route :inv)
                      (invert-windows wins0 (get res :len))
                      wins0)
                  (- tick cstart) (resolve-control-spec (get route :spec) c)))))))))

;; A (mute …)/(solo …) form is unambiguous, so it may sit anywhere in the
;; segment — `-> (shift 2) (mute 9) left` and `-> (mute 9) (shift 2) left`
;; are the same route. The first control form is the target; everything else
;; is route words. Note routes keep destination-first (a bare number is only
;; a destination in that position).
(def split-control-seg (seg)
  (reduce
    (lambda (acc x)
      (if (and (= (get acc :target) nil) (control-route-head? x))
          (merge acc :target x)
          (merge acc :words (append (get acc :words) (list x)))))
    (dict :target nil :words (list))
    seg))

(def prepare-seg (p seg)
  (let ((split (split-control-seg seg)))
    (if (= (get split :target) nil)
        (prepare-route p seg)
        (prepare-control-route p (get split :target) (get split :words)))))

(def prepare-voice (l)
  (let ((segs (split-arrows l (list) (list))))
    (let ((p (from-list (first segs))))
      (if (empty? (rest segs))
          (list (dict :kind :note :p (with-period p) :track 0 :opts (dict)))
          (map (lambda (seg) (prepare-seg p seg)) (rest segs))))))

;; a voice line is a list whose own top level contains a `->`
(def voice-line? (x)
  (if (= (nth x 0) nil) false (member? '-> x)))

;; body data → route records (dict :kind :note|:control :p …). Pure given the
;; body and the unit length (`quant` reads it), so it is memoized below.
;; Each route's opts carry :rkey, its index, so value counters (§7.2) stay
;; per route even when routes share a pattern and words.
(def prepare (body)
  (let ((routes (if (voice-line? (first body))
                    (reduce (lambda (acc l) (append acc (prepare-voice l))) (list) body)
                    (prepare-voice body))))
    (map (lambda (i)
           (let ((r (seed-route (nth routes i) i)))
             (merge r :opts (merge (or-default (get r :opts) (dict))
                                   :rkey (str "r" i ":") :rindex (str i)))))
         (range 0 (len routes)))))

;; a route with row processes and no (seed n) word seeds its hash with its
;; index; the seed joins the id so equal routes keep their own coins and cells
(def seed-route (r i)
  (let ((p (get r :p)))
    (if (or (not (has-procs? p)) (not (= (get p :seed) nil)))
        r
        (merge r :p (merge p :seed i :id (str (get p :id) "|seed:" i))))))

;; Everything up to here is a function of the body alone; only locate/emit
;; depend on the tick. Re-preparing every tick (parse, route words, id
;; strings) was most of the scheduler cost, so the prepared routes are kept
;; per body — `=` is a native deep compare, far cheaper than rebuilding. A
;; few entries so several sequencers sharing this VM don't evict each other.

(def prepared (body)
  (let ((key (list body (state-get "jaki-unit" 0.25))))
    (let ((hit (memo-find prepared-memo key)))
      (if (= hit nil)
          (let ((routes (prepare body)))
            (do (set! prepared-memo (cons (list key routes) (take* 7 prepared-memo)))
                routes))
          hit))))

(def play-route (route)
  (if (= (get route :kind) :control)
      (play-control-route route)
      (emit* (get route :p) (get route :track) (get route :opts))))

;; this tick's routes at the clock's position (`step-clock` ran first).
;; `-> chords` rows play first, so the others read this tick's chord.
(def declares? (r) (string? (get (get r :opts) :declare)))

(def play-routes (body)
  (let ((routes (prepared body)))
    (sum* (map play-route (append (keep declares? routes)
                                  (keep (lambda (r) (not (declares? r))) routes))))))

;; one tick in `mode` (docs/jaki-trig-modes-spec.md §2); 0 when gated shut
(def run-in (mode body)
  (if (step-clock mode) (play-routes body) 0))

(def run (body) (run-in :loop body))
