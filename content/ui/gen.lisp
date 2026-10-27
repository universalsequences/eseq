;; eseq.gen — programs that generate instruments (docs/program-generators-spec.md).
;;
;; A program is a map with a callable :generator: data that knows how to turn
;; itself into DGenLisp. A script whose last value is a program becomes an
;; instrument the same way whoever wrote the generator: C-c C-c plays it on
;; the buffer's draft track, `eseq generate` writes it to a folder. eseq.modal
;; is one generator (m/patch returns a program); a script can hold its own.
;;
;;   (import eseq.gen :as gen)
;;   (def my-synth (p) (dict :name (get p :name) :dsp (...)))
;;   (gen/program my-synth :name "my-synth" :partials 8)
;;
;; A generator returns a result map:
;;   :kind        :instrument (the default; the only kind so far)
;;   :name        the instrument's name: its folder and engine name
;;   :dsp         the dsp.lisp source
;;   :ui          the ui.lisp source, or nil for the stock panel
;;   :voice-mode  the param the host binds its voice mode to, or nil
;;   :assets      absolute paths of files ui.lisp reads, copied beside it
;; or (dict :errors messages) when the program is wrong.
;;
;; The rest of the module is what generators share for building DGenLisp as
;; data: list and form helpers, form builders, param specs (pspec) and their
;; (param ...) forms, values that follow the played note (by-note), the voice
;; preamble every generated instrument starts with, and event clocks
;; (change-tick-forms) for work that runs only when its inputs change.

(module eseq.gen)

(export program generate concat-all index-of member? any? all? first-occurrence? every-other
        take head? head-of-3? def-form? form-atoms def-names map-atoms rename-in defines? nm
        sum-form pick-form tensor-form table-form table-row-form pole-form below-nyquist
        white-noise-form sig-figs by-note-ref by-note by-note? by-note-hz by-note-values
        by-note-log? by-note-errors by-note-segments by-note-raw by-note-at by-note-forms pspec
        spec-ref param-form host-name modulatable? mod-sources preamble mod-hold
        change-tick-forms voice-mode-sym voice-mode-params voice-mode)

;; ── lists ─────────────────────────────────────────────────────────────────

;; Append a list of lists into one list.
;;   lists  a list whose items are lists
(def concat-all (lists) (apply append lists))

;; The first position of x in xs, or nil.
;;   xs  a list
;;   x   a value
(def index-of (xs x)
  (reduce (lambda (found k) (if (= found nil) (if (= (nth xs k) x) k nil) found)) nil (range (len xs))))

;; True when xs holds x.
;;   xs  a list
;;   x   a value
(def member? (xs x) (not (= (index-of xs x) nil)))

;; True when pred holds for some item of xs.
;;   pred  a predicate
;;   xs    a list
(def any? (pred xs) (not (empty? (filter pred xs))))

;; True when pred holds for every item of xs.
;;   pred  a predicate
;;   xs    a list
(def all? (pred xs) (= (len (filter pred xs)) (len xs)))

;; True when xs[k] appears nowhere before k.
;;   xs  a list
;;   k   a position in it
(def first-occurrence? (xs k) (= (index-of xs (nth xs k)) k))

;; Every other item of xs, from start (0 or 1).
;;   xs     a list
;;   start  the first index
(def every-other (xs start) (map (lambda (k) (nth xs (+ start (* 2 k)))) (range (floor (/ (len xs) 2)))))

;; The first n items of xs (all of them when there are fewer).
;;   xs  a list
;;   n   how many
(def take (xs n) (map (lambda (i) (nth xs i)) (range (min n (len xs)))))

;; ── forms as data ─────────────────────────────────────────────────────────
;; Generated code is built as quoted forms (quasiquote) and analysed as data.

;; True when f is a list whose first item is head.
;;   f     a form
;;   head  a symbol
(def head? (f head) (and (list? f) (not (empty? f)) (= (first f) head)))

;; True when f is a three-item (head x y) form: (latch x trigger),
;; (event-hold x clock).
;;   f     a form
;;   head  a symbol
(def head-of-3? (f head) (and (head? f head) (= (len f) 3)))

;; True when f is a top-level (def ...) form.
;;   f  a form
(def def-form? (f) (head? f 'def))

;; Every atom (symbol, number, keyword) in a form, nested lists flattened.
;;   form  a form
(def form-atoms (form) (if (list? form) (concat-all (map form-atoms form)) (list form)))

;; The names a def form defines (one, or a list for (def (a b) ...)).
;;   f  a (def target expr) form
(def def-names (f) (let ((target (nth f 1))) (if (list? target) target (list target))))

;; form with every atom a replaced by (f a).
;;   form  a form
;;   f     a function of an atom
(def map-atoms (form f) (if (list? form) (map (lambda (x) (map-atoms x f)) form) (f form)))

;; form with every symbol in names replaced by itself with suffix.
;;   form    a form
;;   names   the symbols to rename
;;   suffix  a string
(def rename-in (form names suffix)
  (map-atoms form (lambda (a) (if (member? names a) (symbol a suffix) a))))

;; A predicate: true for a def defining one of names.
;;   names  symbols
(def defines? (names) (lambda (f) (and (def-form? f) (any? (lambda (n) (member? names n)) (def-names f)))))

;; ── DGenLisp form builders ────────────────────────────────────────────────

;; The symbol id_suffix, the name of everything a part of the instrument
;; introduces, so two parts never collide: (nm 'str "freqs") -> str_freqs.
;;   id      the owning part's id (a symbol)
;;   suffix  what the name is, as a string or symbol
(def nm (id suffix) (symbol id "_" suffix))

;; A form summing terms: 0 for none, the term itself for one, else (+ ...).
;;   terms  a list of forms
(def sum-form (terms)
  (if (empty? terms) 0 (if (= (len terms) 1) (first terms) (cons '+ terms))))

;; A form choosing among values by a menu param's index: values[idx], with
;; DGenLisp's selector (1-based; the index is a param, so it can change
;; while playing). A single value is just itself.
;;   idx     the menu param's symbol
;;   values  one number per menu option
(def pick-form (idx values)
  (if (= (len values) 1) (first values)
    `(selector (+ 1 (clip (round ,idx) 0 ,(- (len values) 1))) ,@values)))

;; A tensor literal of values (one per mode, usually).
;;   values  a list of numbers
(def tensor-form (values) `(tensor @shape [ ,(len values) ] @data [ ,@values ]))

;; A tensor literal of rows (a list of equal-length lists).
;;   rows  the rows
(def table-form (rows)
  `(tensor @shape [ ,(len rows) ,(len (first rows)) ] @data [ ,@(concat-all rows) ]))

;; A form for row k of a table tensor of rows n wide.
;;   table  the table's symbol
;;   n      its row width
;;   k      a form for the row
(def table-row-form (table n k) `(gather ,table (+ (iota ,n) (* ,k ,n))))

;; A form for a one-pole decay's per-sample pole: e^(-1 / (seconds sr)).
;;   seconds  a form for the time constant, s
(def pole-form (seconds) `(exp (/ -1 (* ,seconds samplerate))))

;; A form for hz kept below Nyquist (at 0.45 of the sample rate), for filters.
;;   hz  a form
(def below-nyquist (hz) `(min ,hz (* 0.45 samplerate)))

;; White noise in -1..1 (DGenLisp's noise is 0..1).
(def white-noise-form () '(- (* 2 (noise)) 1))

;; x rounded to n significant figures (a knob default that prints short).
;;   x  a number
;;   n  how many figures
(def sig-figs (x n)
  (if (= x 0) 0
    (let ((k (pow 10 (- (- n 1) (floor (/ (log (abs x)) (log 10)))))))
      (/ (round (* x k)) k))))

;; ── values that follow the note ─────────────────────────────────────────────
;; Any knob argument can follow the played note instead of being one number,
;; as a piano's strings shorten and thin up the keyboard (in eseq.modal):
;;   (m/string 'str :length (m/by-note 27.5 2.0 262 0.66 4186 0.05))
;; Between the given pitches it is interpolated over log frequency: of the
;; logs of the values when all are positive (so a length halves smoothly
;; over an octave), of the values themselves otherwise; outside them it holds
;; the end value. The knob sets the value at C4 and moves the whole curve
;; with it: it scales a positive curve and shifts any other.

;; The pitch a by-note knob shows its value at, Hz.
(def by-note-ref 261.63)

;; A value following the played note.
;;   points  pitch value pitch value ...: pitches in Hz, rising
(def by-note (&rest points) (dict :kind :by-note :points points))

;; True when x is a by-note value.
;;   x  anything
(def by-note? (x) (and (map? x) (= (get x :kind) :by-note)))

;; A by-note's pitches and values, as two lists.
;;   curve  a by-note
(def by-note-hz (curve) (every-other (get curve :points) 0))
(def by-note-values (curve) (every-other (get curve :points) 1))

;; True when a by-note interpolates logs of its values (all positive).
;;   curve  a by-note
(def by-note-log? (curve) (all? (lambda (v) (> v 0)) (by-note-values curve)))

;; What is wrong with a by-note as an argument: pairs of numbers, at least
;; two, pitches positive and rising.
;;   what   the param, for messages ("str_length")
;;   curve  a by-note
(def by-note-errors (what curve)
  (let ((pts (get curve :points)))
    (if (or (< (len pts) 4) (not (= (mod (len pts) 2) 0)) (not (all? number? pts)))
      (list (str what ": by-note takes pitch value pairs, at least two, got " (source pts)))
      (let ((hz (by-note-hz curve)))
        (if (and (> (first hz) 0) (all? (lambda (k) (< (nth hz k) (nth hz (+ k 1)))) (range (- (len hz) 1)))) nil
          (list (str what ": by-note's pitches must be positive and rising, got " (source hz))))))))

;; A by-note in the domain it is interpolated in (log pitch; log value when
;; all are positive): its first value and each segment as (x0 width slope).
;; The curve at log pitch x is y0 plus every slope times clamp(x - x0, 0,
;; width).
;;   curve  a by-note that by-note-errors finds nothing wrong with
(def by-note-segments (curve)
  (let ((xs (map log (by-note-hz curve))) (lg (by-note-log? curve)))
    (let ((ys (map (lambda (v) (if lg (log v) v)) (by-note-values curve))))
      (dict :y0 (first ys)
            :segs (map (lambda (k) (let ((w (- (nth xs (+ k 1)) (nth xs k))))
                                     (list (nth xs k) w (/ (- (nth ys (+ k 1)) (nth ys k)) w))))
                       (range (- (len xs) 1)))))))

;; The curve at pitch hz, in its interpolation domain.
;;   curve  a by-note
;;   hz     a pitch, Hz
(def by-note-raw (curve hz)
  (let ((sg (by-note-segments curve)) (x (log hz)))
    (reduce (lambda (y sv) (+ y (* (nth sv 2) (clamp (- x (nth sv 0)) 0 (nth sv 1))))) (get sg :y0) (get sg :segs))))

;; The curve at pitch hz, as the generated code interpolates it.
;;   curve  a by-note
;;   hz     a pitch, Hz
(def by-note-at (curve hz) (let ((y (by-note-raw curve hz))) (if (by-note-log? curve) (exp y) y)))

;; Forms defining every by-note param's value from its knob: the curve at
;; the note, scaled or shifted so it passes through the knob's value at C4.
;; It changes only when the pitch or a knob does, so it is worked out on
;; bynote_tick (see change-tick-forms) and latched back to audio rate.
;;   specs  every param spec
(def by-note-forms (specs)
  (let ((curved (filter (lambda (p) (get p :by-note)) specs)))
    (if (empty? curved) (list)
      (append
        (change-tick-forms "bynote" (cons 'f0 (map (lambda (p) (get p :sym)) curved)))
        (list '(def note_ln (log (event-hold f0 bynote_tick))))
        (concat-all
          (map (lambda (p)
                 (let ((curve (get p :by-note)) (sg (by-note-segments (get p :by-note))))
                   (let ((offset (sum-form (cons (- (get sg :y0) (by-note-raw curve by-note-ref))
                                                 (map (lambda (sv) `(* ,(nth sv 2) (clip (- note_ln ,(nth sv 0)) 0 ,(nth sv 1))))
                                                      (get sg :segs)))))
                         (knob `(event-hold ,(get p :sym) bynote_tick)))
                     (list `(def ,(symbol (get p :uses) "_e") ,(if (by-note-log? curve) `(* ,knob (exp ,offset)) `(+ ,knob ,offset)))
                           `(def ,(get p :uses) (latch ,(symbol (get p :uses) "_e") bynote_tick))))))
               curved))))))

;; ── params ────────────────────────────────────────────────────────────────

;; A param spec: one DGenLisp param of a part of the instrument (in
;; eseq.modal, an object, connection or mic), plus how a generated panel
;; shows it.
;;   id       the owner's id; the param is named id_name and grouped by id
;;   name     the param's own name, a string ("length")
;;   default  its initial value (for a menu, the option's index), or a
;;            by-note: the param id_name then follows the note and its
;;            knob, id_name_c4, is the value at C4
;;   lo, hi   its range
;;   options  a list of option strings: makes the param a menu
;;   unit     a unit symbol for @unit ('m, 'mm, 'Hz, 'ms), or nil
;;   title    the panel label (default: name)
;;   taper    the panel knob's taper, :linear or :log
;;   dec      decimals shown (default: picked from hi, 2 / 1 / 0)
(def pspec (id name default lo hi &key (options nil) (unit nil) (title nil) (taper :linear) (dec nil))
  (let ((curve (if (by-note? default) default nil)))
    (dict :sym (if curve (nm id (str name "_c4")) (nm id name)) :uses (if curve (nm id name) nil) :by-note curve
          :name name :group id
          ;; a malformed curve (check reports it) shows its first value
          :default (if curve (if (empty? (by-note-errors "" curve)) (sig-figs (by-note-at curve by-note-ref) 4) lo) default)
          :min lo :max hi :options options :unit unit
          :title (if title title name) :taper taper
          :dec (if dec dec (if (<= hi 10) 2 (if (<= hi 100) 1 0))))))

;; The symbol generated code reads a param by: its knob's, or for a by-note
;; param the value at the note its knob scales.
;;   p  a param spec
(def spec-ref (p) (if (get p :uses) (get p :uses) (get p :sym)))

;; The DGenLisp (param ...) form declaring a param spec: a menu with
;; @options when it has options, else a range with @min / @max (and @unit).
;;   p  a param spec
(def param-form (p)
  (if (get p :options)
    `(param ,(get p :sym) @group ,(get p :group) @options [ ,@(get p :options) ] @default ,(get p :default))
    (append `(param ,(get p :sym) @group ,(get p :group) @default ,(get p :default) @min ,(get p :min) @max ,(get p :max))
            (if (get p :unit) `(@unit ,(get p :unit)) nil))))

;; The name the host knows a grouped param by, group.name: "str.str_length".
;;   p  a param spec
(def host-name (p) (str (source (get p :group)) "." (source (get p :sym))))

;; The param the host binds its voice mode to: what a result's :voice-mode
;; names when voice-mode-params made one.
(def voice-mode-sym 'out_voice_mode)

;; The host's Voicing param, out_voice_mode (bound in instrument.json by the
;; result's :voice-mode), for a voicing: 0 poly, 1 one voice (retriggered),
;; as the host's voice modes; none for nil.
;;   voicing  nil, :poly or :one
(def voice-mode-params (voicing)
  (if (= voicing nil) (list) (list (pspec 'out "voice_mode" (if (= voicing :poly) 0 1) 0 3 :title "Voicing" :dec 0))))

;; A result's :voice-mode for a voicing: voice-mode-sym when
;; voice-mode-params makes the param, else nil.
;;   voicing  nil, :poly or :one
(def voice-mode (voicing) (if (= voicing nil) nil voice-mode-sym))

;; True when the host may modulate a param (see eseq.gen.modulate): every range
;; param but the Voicing, which the host reads for itself.
;;   p  a param spec
(def modulatable? (p) (and (not (get p :options)) (not (= (get p :sym) voice-mode-sym))))

;; ── the voice ─────────────────────────────────────────────────────────────

;; The host's modulation sources, the voice inputs after clock: what every
;; modulatable param reads (see eseq.gen.modulate).
(def mod-sources '(mod1 mod2 mod3 mod4))

;; The forms a generated voice starts with: the voice inputs (gate,
;; pitch, velocity, trigger, clock, then mod-sources), and from them hit (1
;; on the sample a note starts), age (seconds since it did), hit_velocity
;; (the velocity it started with) and f0 (the note's frequency, at least
;; 20 Hz).
(def preamble ()
  (append
   (list '(def gate (in 1 @name gate))
         '(def pitch (in 2 @name pitch))
         '(def velocity (in 3 @name velocity))
         '(def trigger (in 4 @name trigger))
         '(def clock (in 5 @name clock)))
   (map (lambda (k) `(def ,(nth mod-sources k) (in ,(+ k 6) @name ,(nth mod-sources k) @modulator ,(+ k 1))))
        (range (len mod-sources)))
   (list
        '(defmacro onset-of (trigger)
           (make-history previous)
           (def high (gt trigger 0.5))
           (def rising (* high (lte (read-history previous) 0.5)))
           (write-history previous high)
           rising)
        '(defmacro seconds-since (onset)
           (make-history elapsed)
           (make-history armed)
           (def played (max onset (read-history armed)))
           (write-history armed played)
           (def samples (gswitch onset 0 (min (* 8 samplerate) (+ 1 (read-history elapsed)))))
           (write-history elapsed samples)
           (gswitch played (/ samples samplerate) 8))
        '(def hit (onset-of trigger))
        '(def age (seconds-since hit))
        '(def hit_velocity (latch velocity hit))
        ;; note-following objects' coefficients follow f0 (no hop-hold: it
        ;; silences an instrument, eseq-k0m2.31); DGen works them out again
        ;; only when it changes (see eseq.gen.modulate's slow-names)
        '(def f0 (max pitch 20)))))

;; ── event clocks ──────────────────────────────────────────────────────────

;; Samples between reads of the modulation sources: 0.7 ms at 48 kHz.
(def mod-hold 32)

;; Forms defining <name>_tick: 1 on a note's first sample, on the
;; instrument's first, whenever one of inputs changed since the last sample
;; (each kept in a history <name>_in<k>), and every mod-hold samples when a
;; modulation source (mod1..mod4) has moved since the last tick or a note
;; started since. The sources: what the tick's work reads may be modulated
;; ((mod p), see eseq.gen.modulate), and a clock must not read (mod p) itself. The
;; refresh after a start: what the first tick computes can be stale
;; (eseq.modal's held Ride Kit wash), and the next puts it right.
;;   name    the tick's prefix, a string
;;   inputs  forms to watch
(def change-tick-forms (name inputs)
  (let ((n (lambda (suffix) (symbol name suffix)))
        (h (lambda (k) (symbol name "_in" k)))
        (src (lambda (k) (symbol name "_src" k))))
    (let ((tick (n "_tick")) (first-sample (n "_first")) (phase (n "_phase")) (pending (n "_pending")))
      (append (list `(make-history ,(n "_started")) `(make-history ,pending))
              (map (lambda (k) `(make-history ,(h k))) (range (len inputs)))
              (map (lambda (k) `(make-history ,(src k))) (range (len mod-sources)))
              (list `(def ,first-sample (lte (read-history ,(n "_started")) 0))
                    `(def ,phase (eq (accum 1 0 0 ,mod-hold) 0))
                    `(def ,tick (max hit ,first-sample
                                     ,@(map (lambda (k) `(gt (abs (- ,(nth inputs k) (read-history ,(h k)))) 0))
                                            (range (len inputs)))
                                     (* ,phase (max (read-history ,pending)
                                                    ,@(map (lambda (k) `(gt (abs (- ,(nth mod-sources k) (read-history ,(src k)))) 0))
                                                           (range (len mod-sources)))))))
                    `(write-history ,(n "_started") 1)
                    `(write-history ,pending (max hit ,first-sample (* (- 1 ,phase) (read-history ,pending)))))
              (map (lambda (k) `(write-history ,(h k) ,(nth inputs k))) (range (len inputs)))
              (map (lambda (k) `(write-history ,(src k) (gswitch ,tick ,(nth mod-sources k) (read-history ,(src k))))) (range (len mod-sources)))))))

;; ── the protocol ──────────────────────────────────────────────────────────

;; A program: a map whose :generator turns it into a result map.
;;   generator  a function from the program to a result map
;;   kvs        the program's own data, as keyword value pairs
(def program (generator &rest kvs)
  (apply dict (append (list :generator generator) kvs)))

;; True when x can be called: not nil, and none of the data types.
;;   x  anything
(def callable? (x)
  (not (or (= x nil) (number? x) (string? x) (list? x) (map? x))))

;; What is wrong with a generator's result: nothing for (dict :errors ...),
;; else a missing :name or :dsp, a :ui or :assets of the wrong type, or a
;; :kind other than :instrument.
;;   result  what the generator returned
(def result-errors (result)
  (if (not (map? result)) (list (str "the generator returned " (source result) ", not a result map"))
    (if (get result :errors) nil
      (append (if (and (string? (get result :name)) (not (= (get result :name) ""))) nil
                (list "the generator's result has no :name"))
              (if (string? (get result :dsp)) nil (list "the generator's result has no :dsp source"))
              (let ((ui (get result :ui)))
                (if (or (= ui nil) (string? ui)) nil (list "the generator's :ui must be ui.lisp source, a string")))
              (let ((assets (get result :assets)))
                (if (or (= assets nil) (and (list? assets) (all? string? assets))) nil
                  (list "the generator's :assets must be a list of file paths")))
              (let ((kind (get result :kind)))
                (if (or (= kind nil) (= kind :instrument)) nil
                  (list (str "the generator made a " (source kind) "; only :instrument is supported"))))))))

;; What a host runs on a script's last value: the generator's result, or
;; (dict :errors messages) when the value is not a program or the result is
;; malformed. eseq generate and the app's C-c C-c drafts both call this.
;;   prog  the script's last value
(def generate (prog)
  (if (not (and (map? prog) (callable? (get prog :generator))))
    (dict :errors (list "the script's last value must be a program: a map with a :generator"))
    (let ((result ((get prog :generator) prog)))
      (let ((errors (result-errors result)))
        (if (empty? errors) result (dict :errors errors))))))
