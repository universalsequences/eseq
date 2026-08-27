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
;; :hand :left|:right, :vel number, :accent bool, :fig index, :gate rational.
;; `alez.jaki.core/eval-at` returns (dict :events ... :len rational :end-hand hand
;; :end-st velocity-state); velocity state is (dict :cur :pwd :streak).

(module alez.jaki.core)

(export pat from-list xform rev rot trunc every stac ghost swap
        shift filter for-hand fast slow
        eval-at eval-cycle cycle-length locate cycle-index
        default-state mk-state
        init emit emit* reset
        run)

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
  (member? w '(left right accent rev stac ghost swap id)))

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
                        (let ((x (norm-xf it)))
                          (if (= x nil)
                              acc
                              (merge acc :xf (append (get acc :xf) (list x)))))))))))
    (dict :events (list) :xf (list) :tm nil :align nil)
    items))

(def fig-form? (x) (= (raw-head x) 'fig))

(def from-list (body)
  (dict :id (source body)
        :figs (if (fig-form? (first body))
                  (map (lambda (f) (parse-fig (rest f))) body)
                  (list (parse-fig body)))
        :post (list)))

(defmacro pat (&rest body)
  `(alez.jaki.core/from-list '(,@body)))

;; ── whole-pattern transform functions (spec §4.6) ───────────────────────────

;; append a transform (quoted data, e.g. '(every 2 rev)) to every figure
(def xform (p t)
  (merge p
    :figs (map (lambda (f) (merge f :xf (append (get f :xf) (list (norm-xf t)))))
               (get p :figs))
    :id (str (get p :id) "|xf:" (source t))))

;; retime every figure: (fast p 2), (slow p (cyc 1 2)) — n is raw per-cycle
;; argument data, same as a figure's own (* n)/(/ n) time-mod
(def retime (p mode n)
  (merge p
    :figs (map (lambda (f) (merge f :tm (list mode n))) (get p :figs))
    :id (str (get p :id) "|tm:" (source (list mode n)))))
(def fast (p n) (retime p :fast n))
(def slow (p n) (retime p :slow n))

(def rev (p) (xform p 'rev))
(def rot (p n) (xform p (list 'rot n)))
(def trunc (p n) (xform p (list 'trunc n)))
(def every (p n t) (xform p (list 'every n t)))
(def stac (p) (xform p 'stac))
(def ghost (p) (xform p 'ghost))
(def swap (p) (xform p 'swap))

(def add-post (p op tag)
  (merge p :post (append (get p :post) (list op))
           :id (str (get p :id) "|" tag)))

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

(def resolve-arg (raw cycle)
  (if (= raw nil)
      1
      (let ((h (nth raw 0)))
        (if (= h 'cyc)
            (let ((vals (rest raw)))
              (resolve-arg (nth vals (imod cycle (max 1 (len vals)))) cycle))
            (if (= h 'chan)
                (chan-get (nth raw 1) (nth raw 2))
                (if (implicit-cyc? raw)
                    (resolve-arg (nth raw (imod cycle (max 1 (len raw)))) cycle)
                    ;; numbers, symbols, and other forms evaluate as source
                    (eval (source raw))))))))

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
      :alt (apply-one-xf evs (alt-pick (rest f) cycle) cycle)
      :split (split-events evs (nth f 1))
      :merge (merge-events evs (nth f 1))
      _ evs)))

(def apply-xf-events (evs xfs cycle)
  (reduce (lambda (acc f) (apply-one-xf acc f cycle)) evs xfs))

;; fast (* m): each dot becomes m dots, each dash (2m-2) dots + a dash
(def expand-fast (evs m)
  (if (<= m 1)
      evs
      (reduce
        (lambda (acc e)
          (append acc
            (if (= e :dot)
                (repeat* :dot m)
                (append (repeat* :dot (- (* 2 m) 2)) (list :dash)))))
        (list) evs)))

(def units (evs) (reduce (lambda (a e) (+ a (if (= e :dash) 2 1))) 0 evs))

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
                    (if (= h :alt)
                        (flag-active? (list (alt-pick (rest f) cycle)) kw cycle)
                        false))))))
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
                    (let ((v (clampv (* cur (get params :dot-decay)) params)))
                      (dict :vel v :accent false
                            :st (mk-state v false (+ streak 1))))))))))

;; ── hand model (spec §6) ────────────────────────────────────────────────────

(def other-hand (h) (if (= h :left) :right :left))

;; one entry per HIT: dots take the current hand, a dash takes it twice
(def derive-hands (evs hand)
  (if (empty? evs)
      (list)
      (append (if (= (first evs) :dash) (list hand hand) (list hand))
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
      (let ((dash? (= (first evs) :dash))
            (scale (get ctx :scale)))
        (let ((r1 (mk-hit dash? false ctx st off (nth hands hit-idx))))
          (if dash?
              (let ((r2 (mk-hit true true ctx (get r1 :st) (r+ off scale)
                                (nth hands (+ hit-idx 1)))))
                (fold-events (rest evs) hands ctx (get r2 :st)
                             (r+ off (r* scale (r-int 2))) (+ hit-idx 2)
                             (append acc (list (get r1 :ev) (get r2 :ev)))))
              (fold-events (rest evs) hands ctx (get r1 :st)
                           (r+ off scale) (+ hit-idx 1)
                           (append acc (list (get r1 :ev)))))))))

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
        (tm (get fig :tm)))
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
      (let ((r (eval-fig (first figs) cycle off hand st idx)))
        (eval-figs (rest figs) cycle (r+ off (get r :dur))
                   (get r :hand) (get r :st) (+ idx 1)
                   (append acc (get r :evs))))))

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

(def memo-find (m key)
  (if (empty? m)
      nil
      (if (= (first (first m)) key)
          (nth (first m) 1)
          (memo-find (rest m) key))))

(def eval-cycle (p cycle hand st)
  ;; Payload channels can alter evaluated event data but never cycle length.
  ;; Include their epoch here only: len-memo and lens-memo intentionally keep
  ;; their structural keys across channel writes.
  (let ((key (list (get p :id) cycle hand
                   (get st :cur) (get st :pwd) (get st :streak)
                   (chan-epoch))))
    (let ((hit (memo-find memo-store key)))
      (if (= hit nil)
          (let ((r (eval-at p cycle hand st)))
            (do (set! memo-store (cons (list key r) (take* 15 memo-store)))
                r))
          hit))))

;; integer length in units of one cycle (state-independent)
(def cycle-length (p k)
  (let ((key (list (get p :id) k)))
    (let ((hit (memo-find len-memo key)))
      (if (= hit nil)
          (let ((l (r->f (get (eval-cycle p k :left default-state) :len))))
            (do (set! len-memo (cons (list key l) (take* 31 len-memo)))
                l))
          hit))))

;; ── cycle indexing: closed form over the length super-cycle (spec §8.1) ─────

(def arg-period (raw)
  (if (= raw nil)
      1
      (if (= (nth raw 0) 'cyc)
          (reduce (lambda (a x) (lcm* a (arg-period x)))
                  (max 1 (len (rest raw))) (rest raw))
          (if (implicit-cyc? raw)
              (reduce (lambda (a x) (lcm* a (arg-period x)))
                      (max 1 (len raw)) raw)
              1))))

(def xf-period (f)
  (let ((h (first f)))
    (match h
      :every (lcm* (max 1 (round-int (resolve-arg (nth f 1) 0)))
                   (lcm* (arg-period (nth f 1)) (xf-period (nth f 2))))
      :alt (reduce (lambda (a x) (lcm* a (xf-period x)))
                   (max 1 (len (rest f))) (rest f))
      :L (xf-period (nth f 1))
      :R (xf-period (nth f 1))
      _ (reduce (lambda (a x) (lcm* a (arg-period x))) 1 (rest f)))))

(def fig-period (f)
  (lcm* (arg-period (if (= (get f :tm) nil) nil (nth (get f :tm) 1)))
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
  (let ((hit (memo-find lens-memo (get p :id))))
    (if (= hit nil)
        (let ((pd (pat-period p)))
          (let ((lens (map (lambda (k) (cycle-length p k)) (range 0 pd))))
            (let ((entry (list lens (sum* lens) pd)))
              (do (set! lens-memo
                        (cons (list (get p :id) entry) (take* 23 lens-memo)))
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
            (store-state p c :left default-state)))))

;; idempotent per-tick setup: record the beat length of one unit
(def init (res) (do (state-set! "jaki-unit" (beats res)) nil))

(def reset ()
  (do (set! memo-store (list))
      (set! len-memo (list))
      (set! lens-memo (list))
      (state-set! "jaki-cycle" -1)
      nil))

(def or-default (v d) (if (= v nil) d v))

(def emit-one (e u track opts)
  (let ((unit (state-get "jaki-unit" 0.25)))
    (seq-emit :track track
              :at (* (r->f (r- (get e :off) (r-int u))) unit)
              :vel (* (get e :vel) (or-default (get opts :vel-scale) 1))
              :note (or-default (get opts :note) 0)
              :dur (* (r->f (get e :gate)) unit))))

;; emit every event whose offset falls in this tick's unit window [u, u+1) —
;; exact rational membership, no epsilon (spec §8.3)
(def emit-window (evs u track opts)
  (reduce
    (lambda (n e)
      (if (and (r<= (r-int u) (get e :off))
               (r< (get e :off) (r-int (+ u 1))))
          (do (emit-one e u track opts) (+ n 1))
          n))
    0 evs))

(def resolve-opt (opts k c)
  (let ((v (get opts k)))
    (if (= v nil) nil (resolve-arg v c))))

;; evaluate the pattern for the current tick's cycle and emit this window's
;; events; returns the number of events emitted. `track` and the emit opts
;; (:note, :vel-scale) are raw per-cycle argument data — numbers, or (cyc …)
;; to cycle the destination / transpose / velocity scale per cycle.
(def emit* (p track opts)
  (let ((tick (gen-tick)))
    (let ((loc (locate p tick)))
      (let ((c (first loc)) (cstart (nth loc 1)))
        (do (ensure-state p c)
            (let ((r (eval-cycle p c (n->hand (state-get (cell p "jaki-hand") 0))
                                 (load-state p))))
              (emit-window (get r :events) (- tick cstart)
                           (round-int (resolve-arg track c))
                           (dict :note (resolve-opt opts :note c)
                                 :vel-scale (resolve-opt opts :vel-scale c)))))))))

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
;;   (fast n) (slow n) — n may be (cyc …) for conditional retiming
;;   Any per-cycle arg also reads Tidal-style implicit cyc: a list headed by
;;   a value is (cyc …) recursively — (1 2 (1 3)) = (cyc 1 2 (cyc 1 3))
;;   Whole WORDS alternate the same way: (right right right left) picks one
;;   modifier per cycle ((cyc w…) is the explicit spelling, `id` the no-op);
;;   members must all be post-lowerable or all figure transforms
;;   (vel s) (note n)
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
(def route-post-op (w)
  (if (alt-word? w)
      (let ((posts (map route-post-op (alt-members w))))
        (if (member? nil posts) nil (cons :alt posts)))
      (let ((h (raw-head w)) (args (raw-args w)))
        (match h
          'stac   (list :stac)
          'id     (list :id)
          'gate   (list :gate (nth args 0))
          'dur    (list :gate (nth args 0))
          'quant  (list :quant (quant-units (nth args 0)))
          'shift  (list :shift (nth args 0))
          'left   (list :filter '(:hand :left))
          'right  (list :filter '(:hand :right))
          'accent (list :filter '(:accent true))
          'every  (let ((inner (route-post-op (nth args 1))))
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

(def route-step-word (acc w)
  (let ((h (raw-head w)) (args (raw-args w)))
    (match h
      'left   (merge acc :p (filter (get acc :p) '(:hand :left)))
      'right  (merge acc :p (filter (get acc :p) '(:hand :right)))
      'accent (merge acc :p (filter (get acc :p) '(:accent true)))
      'rev    (merge acc :p (rev (get acc :p)))
      'stac   (merge acc :p (add-post (get acc :p) (list :stac) "stac"))
      'ghost  (merge acc :p (ghost (get acc :p)))
      'swap   (merge acc :p (swap (get acc :p)))
      'rot    (merge acc :p (rot (get acc :p) (nth args 0)))
      'trunc  (merge acc :p (trunc (get acc :p) (nth args 0)))
      'shift  (merge acc :p (shift (get acc :p) (nth args 0)))
      'fast   (merge acc :p (fast (get acc :p) (nth args 0)))
      'slow   (merge acc :p (slow (get acc :p) (nth args 0)))
      'gate   (merge acc :p (add-post (get acc :p) (list :gate (nth args 0))
                                      (str "gate:" (source (nth args 0)))))
      'dur    (merge acc :p (add-post (get acc :p) (list :gate (nth args 0))
                                      (str "gate:" (source (nth args 0)))))
      'quant  (let ((q (quant-units (nth args 0))))
                (merge acc :p (add-post (get acc :p) (list :quant q)
                                        (str "quant:" (source q)))))
      'every  (let ((post (route-post-op (nth args 1))))
                (if (= post nil)
                    (merge acc :p (every (get acc :p) (nth args 0) (nth args 1)))
                    (merge acc :p (add-post (get acc :p)
                                            (list :every (nth args 0) post)
                                            (str "every:" (source (nth args 0))
                                                 ":" (source post))))))
      'for-hand (merge acc :p (for-hand (get acc :p) (nth args 0) (nth args 1)))
      'vel    (merge acc :opts (merge (get acc :opts) :vel-scale (nth args 0)))
      'note   (merge acc :opts (merge (get acc :opts) :note (nth args 0)))
      'inv    (merge acc :inv true)
      _ acc)))

(def run-route (p seg)
  (let ((r (reduce route-step (dict :p p :opts (dict)) (rest seg))))
    (emit* (get r :p) (first seg) (get r :opts))))

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

(def run-control-route (p0 target-form words)
  (let ((spec (parse-control-target target-form))
        (r (reduce route-step (dict :p p0 :opts (dict)) words)))
    (let ((p (get r :p))
          (tick (gen-tick)))
      (let ((loc (locate p tick)))
        (let ((c (first loc)) (cstart (nth loc 1)))
          (do (ensure-state p c)
              (let ((res (eval-cycle p c (n->hand (state-get (cell p "jaki-hand") 0))
                                     (load-state p))))
                (let ((wins0 (event-windows (get res :events) (get res :len))))
                  (emit-window-controls
                    (if (get r :inv)
                        (invert-windows wins0 (get res :len))
                        wins0)
                    (- tick cstart) (resolve-control-spec spec c))))))))))

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

(def run-seg (p seg)
  (let ((split (split-control-seg seg)))
    (if (= (get split :target) nil)
        (run-route p seg)
        (run-control-route p (get split :target) (get split :words)))))

(def run-voice (l)
  (let ((segs (split-arrows l (list) (list))))
    (let ((p (from-list (first segs))))
      (if (empty? (rest segs))
          (emit p 0)
          (sum* (map (lambda (seg) (run-seg p seg)) (rest segs)))))))

;; a voice line is a list whose own top level contains a `->`
(def voice-line? (x)
  (if (= (nth x 0) nil) false (member? '-> x)))

(def run (body)
  (if (voice-line? (first body))
      (sum* (map run-voice body))
      (run-voice body)))
