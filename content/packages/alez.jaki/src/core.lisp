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

(export pat from-list xform rev rot trunc every every-fig stac ghost swap
        shift filter for-hand fast slow on preview
        eval-at eval-cycle cycle-length locate cycle-index
        default-state mk-state
        init emit emit* reset
        run run-in step-clock play-routes play-pos)

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

;; stable insertion sort by rational :off
(def insert-ev (ev l)
  (if (empty? l)
      (list ev)
      (if (r< (get ev :off) (get (first l) :off))
          (cons ev l)
          (cons (first l) (insert-ev ev (rest l))))))
(def sort-walk (l acc) (if (empty? l) acc (sort-walk (rest l) (insert-ev (first l) acc))))
(def sort-evs (l) (sort-walk l (list)))

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
(def trunc (p n) (xform p (list 'trunc n)))
(def every (p n t) (xform p (list 'every n t)))
(def stac (p) (xform p 'stac))
(def ghost (p) (xform p 'ghost))
(def swap (p) (xform p 'swap))

;; A per-hit word whose argument needs a hit clock (spec §7.2) lowers to
;; (:defer kind raw nil); add-post keys it with the pattern id it creates, so
;; every such word has its own counters.
(def key-defers (op key)
  (match (first op)
    :defer (list :defer (nth op 1) (nth op 2) key)
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
                  (resolve-arg-at (nth vals (imod cycle (max 1 (len vals)))) cycle fig))
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
                                (resolve-arg-at (nth raw (imod cycle (max 1 (len raw)))) cycle fig)
                                ;; symbols and other forms evaluate as source
                                (eval (source raw))))))))))))

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

(def default-params
  (dict :base 0.8 :dot-decay 0.85 :dash-decay 0.9
        :accent-boost 1.15 :min-vel 0.3 :max-vel 1.0))
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
          (let ((r1 (mk-hit dash? false hctx st off (nth hands hit-idx))))
            (if dash?
                (let ((r2 (mk-hit true true hctx (get r1 :st) (r+ off scale)
                                  (nth hands (+ hit-idx 1)))))
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
                        :st (get pr :st)))
                (merge body :dur (dur dprime))))))))

;; evaluate one figure → (dict :evs :dur :hand :st)
(def eval-fig (fig cycle off hand st figidx)
  (let ((xfs (get fig :xf))
        (evs1 (apply-xf-events (get fig :events) (get fig :xf) cycle))
        (tm (tm-at (get fig :tm) cycle)))
    (let ((kind (if (= tm nil) nil (nth tm 0)))
          (m (if (= tm nil) 1 (max 1 (round-int (resolve-arg (nth tm 1) cycle))))))
      (let ((evs2 (if (= kind :fast) (expand-fast evs1 m) evs1)))
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
                                      :params params :fig figidx)
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
                    (apply-align body (get fig :align) cycle off params figidx)))))))))))

;; ── whole-pattern evaluation ────────────────────────────────────────────────

(def eval-figs (figs cycle off hand st idx acc)
  (if (empty? figs)
      (dict :evs acc :off off :hand hand :st st)
      (let ((r (eval-fig (first figs) cycle off hand st idx))
            (a (get (first figs) :afig))
            (k (get (first figs) :arep)))
        (eval-figs (rest figs) cycle (r+ off (get r :dur))
                   (get r :hand) (get r :st) (+ idx 1)
                   (append acc (map (lambda (e) (merge e :afig a :arep k))
                                    (get r :evs)))))))

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
      _ res)))

;; evaluate a pattern for one cycle with explicit threading state
(def eval-at (p cycle hand st)
  (let ((r (eval-figs (get p :figs) cycle (r-int 0) hand st 0 (list))))
    (reduce (lambda (acc op) (apply-post-one acc op cycle))
            (dict :events (sort-evs (get r :evs)) :len (get r :off)
                  :end-hand (get r :hand) :end-st (get r :st))
            (get p :post))))

;; ── per-cycle memo (assoc list in scheduler-VM globals, spec §8.2) ──────────

(def memo-store (list))
(def len-memo (list))
(def prepared-memo (list))   ; `prepared` below: body → route records

(def memo-find (m key)
  (if (empty? m)
      nil
      (if (= (first (first m)) key)
          (nth (first m) 1)
          (memo-find (rest m) key))))

;; ── evaluation period ───────────────────────────────────────────────────────
;; eval-at depends on the cycle index only through per-cycle arguments ((cyc
;; …), implicit cyc), `every` gates and word alternations, so for a pattern
;; built from those alone eval-at(c) = eval-at(c mod P). Keying the memo on
;; c mod P turns every steady-state cycle boundary into a lookup instead of a
;; full re-evaluation. 0 means "not provably periodic" — some argument is an
;; arbitrary expression, which `resolve-arg` evaluates from source — and
;; keeps the exact cycle key.

(def lcm0 (a b) (if (or (= a 0) (= b 0)) 0 (lcm* a b)))

(def members-period (vals)
  (reduce (lambda (a x) (lcm0 a (raw-period x))) (max 1 (len vals)) vals))

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

(def eval-period (p)
  (let ((pd (reduce (lambda (a op) (lcm0 a (post-period* op)))
                    (reduce (lambda (a f) (lcm0 a (fig-period* f))) 1 (get p :figs))
                    (get p :post))))
    (if (> pd 256) 0 pd)))

;; `prepare` stamps :period on each route's pattern; patterns built by hand
;; (emit, emit*) carry none and keep the exact cycle key.
(def with-period (p) (merge p :period (eval-period p)))

(def memo-cycle (p cycle)
  (let ((pd (get p :period)))
    (if (and (number? pd) (> pd 0)) (imod cycle pd) cycle)))

(def eval-cycle (p cycle hand st)
  ;; Payload channels can alter evaluated event data but never cycle length.
  ;; Include their epoch here only: len-memo and lens-memo intentionally keep
  ;; their structural keys across channel writes.
  (let ((key (list (get p :id) (memo-cycle p cycle) hand
                   (get st :cur) (get st :pwd) (get st :streak)
                   (chan-epoch))))
    (let ((hit (memo-find memo-store key)))
      (if (= hit nil)
          (let ((r (eval-at p cycle hand st)))
            (do (set! memo-store (cons (list key r) (take* 63 memo-store)))
                r))
          hit))))

;; length-only figure evaluation: cycle length depends on the symbolic-event
;; transforms, the time-mod, and alignment — never on hands, velocities, or
;; post ops. Skipping the full fold makes the pat-lens warm-up (pd cycles per
;; pattern, on the scheduler thread after a re-eval) cheap instead of an
;; audible pause. MUST stay in lockstep with eval-fig's duration math:
;; :fast expansion multiplies units by exactly m, so eff reduces to the
;; pre-expansion unit count.
(def fig-len (fig cycle off)
  (let ((evs1 (apply-xf-events (get fig :events) (get fig :xf) cycle))
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
          (let ((l (r->f (cycle-len-figs (get p :figs) k (r-int 0)))))
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
            (reduce (lambda (a x) (lcm* a (arg-period x)))
                    (max 1 (len vals)) vals))
          (if (implicit-cyc? raw)
              (reduce (lambda (a x) (lcm* a (arg-period x)))
                      (max 1 (len raw)) raw)
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

(def pat-period (p)
  (min 64 (max 1 (reduce (lambda (a f) (lcm* a (fig-period f))) 1 (get p :figs)))))

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
  (mk-state (state-get (cell p "jaki-vel") 0.8)
            (n->b (state-get (cell p "jaki-pwd") 0))
            (state-get (cell p "jaki-streak") 0)))

(def store-state (p c hand st)
  (do (state-set! (cell p "jaki-cycle") c)
      (state-set! (cell p "jaki-hand") (hand->n hand))
      (state-set! (cell p "jaki-vel") (get st :cur))
      (state-set! (cell p "jaki-pwd") (b->n (get st :pwd)))
      (state-set! (cell p "jaki-streak") (get st :streak))
      nil))

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
      (if (and (= (nth raw 0) 'seq) (ev-clock? (nth raw 1)))
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
                    _ (pick-member vals c ctx here 2)))
                (if (= h 'cyc)
                    (pick-member (rest raw) c ctx here 1)
                    (if (= h 'every-gate)
                        (resolve-ev (if (every-active? (round-int (resolve-arg (nth raw 1) c)) c)
                                        (nth raw 2) (nth raw 3))
                                    ctx nil)
                        (if (= h 'fig-gate)
                            (resolve-ev (if (fig-index-active? (nth raw 1) (get ctx :fig) c)
                                            (nth raw 2) (nth raw 3))
                                        ctx nil)
                            (if (implicit-cyc? raw)
                                (pick-member raw c ctx here 0)
                                (resolve-arg-at raw c (get ctx :fig))))))))))))

(def seq-epoch (p) (state-get (cell p "jaki-seq-epoch") 0))

;; advance value `key`'s counters for hit e and return its resolution context:
;; :hit counts this value's hits, :figs its figure occurrences (it moves on
;; when the hit's cycle or figure differs from the previous hit's)
(def ev-ctx (p key e c clen)
  (let ((k (str "seq:" (seq-epoch p) ":" (get p :id) ":" key)))
    (let ((n (state-get (str k ":h") 0))
          (last (state-get (str k ":l") -1))
          (m (state-get (str k ":f") 0))
          (cur (+ (* c 4096) (get e :fig))))
      (let ((m2 (if (= cur last) m (+ m 1))))
        (do (state-set! (str k ":h") (+ n 1))
            (state-set! (str k ":l") cur)
            (state-set! (str k ":f") m2)
            (dict :cycle c :fig (get e :fig) :hit n :figs (- m2 1)
                  :pos (if (> clen 0) (/ (r->f (get e :off)) clen) 0)))))))

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
      _ acc)))

;; opts stay raw and resolve per event: (every-fig n (note m)) differs per
;; figure within one cycle, (seq :hit …) per hit. Deferred words apply after
;; the ones applied during evaluation.
(def emit-one (p e u track opts c clen)
  (let ((unit (state-get "jaki-unit" 0.25)) (rkey (or-default (get opts :rkey) ""))
        (reset (set! lit-picks (list))))
    (let ((a (reduce (lambda (acc d) (apply-defer acc d p rkey e c clen))
                     (dict :vel (get e :vel) :nadd (or-default (get e :nadd) 0)
                           :nset (get e :nset) :gmul 1)
                     (or-default (get e :defer) (list))))
          (opt-note (ev-value p (str rkey "opt:note") (get opts :note) e c clen 0))
          (opt-vel (ev-value p (str rkey "opt:vel") (get opts :vel-scale) e c clen 1)))
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
      (seq-emit :track track
                :at at
;; a gating fire's payload rides on top of everything the row
                ;; computes: its velocity scales, its note adds after an
                ;; absolute (note …) set (docs/jaki-trig-modes-spec.md §3)
                :vel (* (* (get a :vel) opt-vel) (state-get "jaki-gvel" 1))
                :note (+ (+ (let ((ns (get a :nset))) (if (= ns nil) opt-note ns))
                            (get a :nadd))
                         (state-get "jaki-gnote" 0))
                :dur dur))))))

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
            (let ((r (eval-cycle p c (n->hand (state-get (cell p "jaki-hand") 0))
                                 (load-state p))))
              (emit-window p (get r :events) (- tick cstart)
                           (round-int (resolve-arg track c))
                           opts c (r->f (get r :len)))))))))

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
;;   any figure transform: (basevel v) (dotdecay v) (dashdecay v) (minvel v)
;;   (maxvel v) (split t) (merge t) (L w) (R w)
;;   (gate s) / (dur s) — multiply every gate by s (per-cycle arg: number,
;;   (cyc …), (chan …)); applies in authored word order like stac
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
      'vel*   (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'vel+   (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'note+  (add-route-post acc (route-post-op-at w (get acc :wpath)) w)
      'on     (route-on acc (nth args 0) (rest args))
      'inv    (merge acc :inv true)
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

(def prepare-route (p seg)
  (let ((r (route-steps (dict :p p :opts (dict)) (rest seg))))
    (dict :kind :note :p (with-period (get r :p)) :track (first seg)
          :opts (merge (get r :opts) :lights (= (get r :lights) true)))))

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
           (let ((r (nth routes i)))
             (merge r :opts (merge (or-default (get r :opts) (dict))
                                   :rkey (str "r" i ":") :rindex (str i)))))
         (range 0 (len routes)))))

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

;; this tick's routes at the clock's position (`step-clock` ran first)
(def play-routes (body)
  (sum* (map play-route (prepared body))))

;; one tick in `mode` (docs/jaki-trig-modes-spec.md §2); 0 when gated shut
(def run-in (mode body)
  (if (step-clock mode) (play-routes body) 0))

(def run (body) (run-in :loop body))
