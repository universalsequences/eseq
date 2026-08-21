;; Jaki sequencer library — pure-Lisp evaluator core, pattern surface, and
;; generator wiring (docs/jaki-sequencer-spec.md §11 phases 1-3, bead eseq-5k5).
;;
;; Patterns use the variadic `jaki/pat` macro:
;;
;;   (jaki/pat . . - (every 2 swap) (* (cyc 1 2)))
;;   (jaki/pat (fig (. . -) (* 2)) (fig (. -) (/ 3)))
;;
;; The tick body that builds patterns is authored on the UI VM but runs on the
;; scheduler VM: def-sequencer auto-quotes the body and re-serializes it as
;; source, so the macro call is expanded in the scheduler VM that owns the Jaki
;; runtime.
;;
;; Offsets and gates are exact rationals — normalized (numerator denominator)
;; 2-lists — so tuplet window membership never needs an epsilon (spec §8.3).
;; One unit = one tick of the generator's :resolution grid.
;;
;; Evaluated event fields (dicts): :off rational, :sym :dot|:dash, :hit 1|2,
;; :hand :left|:right, :vel number, :accent bool, :fig index, :gate rational.
;; `jaki/eval-at` returns (dict :events ... :len rational :end-hand hand
;; :end-st velocity-state); velocity state is (dict :cur :pwd :streak).

(module jaki)

(export pat from-list xform rev rot trunc every stac ghost swap
        shift filter for-hand
        eval-at eval-cycle cycle-length locate cycle-index
        default-state mk-state
        init emit emit* reset)

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
    _ nil))

(def norm-target (t) (match t 'first :first 'last :last 'all :all _ t))

;; normalize a transform (symbol or list) to a keyword-headed list; unknown → nil
(def norm-xf (f)
  (let ((h (head-kw (raw-head f))) (args (raw-args f)))
    (match h
      :every (list :every (nth args 0) (norm-xf (nth args 1)))
      :L (list :L (norm-xf (nth args 0)))
      :R (list :R (norm-xf (nth args 0)))
      :split (list :split (norm-target (nth args 0)))
      :merge (list :merge (norm-target (nth args 0)))
      _ (if (= h nil) nil (cons h args)))))

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
  `(jaki/from-list '(,@body)))

;; ── whole-pattern transform functions (spec §4.6) ───────────────────────────

;; append a transform (quoted data, e.g. '(every 2 rev)) to every figure
(def xform (p t)
  (merge p
    :figs (map (lambda (f) (merge f :xf (append (get f :xf) (list (norm-xf t)))))
               (get p :figs))
    :id (str (get p :id) "|xf:" (source t))))

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

;; keyed filter over the evaluated events, e.g. (jaki/filter p '(:hand :left))
(def filter (p spec) (add-post p (list :filter spec) (str "filter:" (source spec))))

;; hand-scoped transform, e.g. (jaki/for-hand p :left '(stac))
(def for-hand (p hand t)
  (add-post p (list :for-hand hand (norm-xf t))
            (str "fh:" (source hand) ":" (source t))))

;; ── per-cycle argument resolution ((cyc ...) and expression fallback) ───────

(def resolve-arg (raw cycle)
  (if (= raw nil)
      1
      (let ((h (nth raw 0)))
        (if (= h 'cyc)
            (let ((vals (rest raw)))
              (resolve-arg (nth vals (imod cycle (max 1 (len vals)))) cycle))
            ;; numbers, symbols, and non-cyc forms all evaluate as source —
            ;; the (param-get ...) escape hatch once that native lands
            (eval (source raw))))))

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
                    false)))))
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

(def apply-post-one (res op cycle)
  (let ((h (first op)))
    (match h
      :filter (apply-filter res (nth op 1))
      :shift (apply-shift res (round-int (resolve-arg (nth op 1) cycle)))
      :for-hand (apply-for-hand res (nth op 1) (nth op 2) cycle)
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
  (let ((key (list (get p :id) cycle hand
                   (get st :cur) (get st :pwd) (get st :streak))))
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
      (if (= (nth raw 0) 'cyc) (max 1 (len (rest raw))) 1)))

(def xf-period (f)
  (let ((h (first f)))
    (match h
      :every (lcm* (max 1 (round-int (resolve-arg (nth f 1) 0)))
                   (lcm* (arg-period (nth f 1)) (xf-period (nth f 2))))
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

;; position (integer units) → (cycle-index cycle-start-unit)
(def locate (p pos)
  (let ((pd (pat-period p)))
    (let ((lens (map (lambda (k) (cycle-length p k)) (range 0 pd))))
      (let ((total (sum* lens)))
        (if (<= total 0)
            (list 0 pos)
            (let ((full (idiv pos total)))
              (let ((rem (- pos (* full total))))
                (let ((k (scan-cycle lens rem 0)))
                  (list (+ (* full pd) k)
                        (+ (* full total) (prefix-sum lens k)))))))))))

(def cycle-index (p pos) (first (locate p pos)))

;; ── generator wiring (spec §8.1): state cells + emission ────────────────────

(def hand->n (h) (if (= h :left) 0 1))
(def n->hand (n) (if (= n 0) :left :right))
(def b->n (b) (if b 1 0))
(def n->b (n) (not (= n 0)))

(def load-state ()
  (mk-state (state-get "jaki-vel" 0.8)
            (n->b (state-get "jaki-pwd" 0))
            (state-get "jaki-streak" 0)))

(def store-state (c hand st)
  (do (state-set! "jaki-cycle" c)
      (state-set! "jaki-hand" (hand->n hand))
      (state-set! "jaki-vel" (get st :cur))
      (state-set! "jaki-pwd" (b->n (get st :pwd)))
      (state-set! "jaki-streak" (get st :streak))
      nil))

(def roll-state (p from to)
  (if (>= from to)
      nil
      (let ((r (eval-cycle p from (n->hand (state-get "jaki-hand" 0)) (load-state))))
        (do (store-state (+ from 1) (get r :end-hand) (get r :end-st))
            (roll-state p (+ from 1) to)))))

;; advance the threaded hand/velocity state to cycle c: contiguous advances
;; roll the ending state forward; jumps (transport relocation, generator
;; reset) restart from defaults — cycle indexing itself stays closed-form
(def ensure-state (p c)
  (let ((stored (state-get "jaki-cycle" -1)))
    (if (= stored c)
        nil
        (if (and (>= stored 0) (> c stored) (<= (- c stored) 8))
            (roll-state p stored c)
            (store-state c :left default-state)))))

;; idempotent per-tick setup: record the beat length of one unit
(def init (res) (do (state-set! "jaki-unit" (beats res)) nil))

(def reset ()
  (do (set! memo-store (list))
      (set! len-memo (list))
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

;; evaluate the pattern for the current tick's cycle and emit this window's
;; events; returns the number of events emitted
(def emit* (p track opts)
  (let ((tick (gen-tick)))
    (let ((loc (locate p tick)))
      (let ((c (first loc)) (cstart (nth loc 1)))
        (do (ensure-state p c)
            (let ((r (eval-cycle p c (n->hand (state-get "jaki-hand" 0)) (load-state))))
              (emit-window (get r :events) (- tick cstart) track opts)))))))

(def emit (p track) (emit* p track (dict)))
