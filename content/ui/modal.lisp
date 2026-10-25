;; eseq.modal — Modalys-style instruments from objects and connections.
;;
;; A patch is plain data: objects (string, plate, membrane, finger, mallet),
;; connections between their access points (pluck, spring, felt, bow), and
;; listening points. (dsp patch) turns it into the source of an ordinary
;; DGenLisp instrument calling the protocol macros in content/defmacros
;; (modal-predict, modal-point, a force law, modal-advance), in the order one
;; sample needs:
;;   1 predict every object   2 + 3 solve the exciters (pluck, felt) in turn,
;;   each reading the objects as earlier ones left them, then springs and
;;   bows by Gauss-Seidel sweeps   4 commit every object
;; Every name an object or connection introduces is prefixed with its id, so
;; two strings never collide; their params are prefixed the same way and
;; grouped by id, so the instrument panel shows one section per object.
;;
;;   (import eseq.modal :as m)
;;   (import eseq.modal.materials :as mat)
;;   (m/patch :name "my-cello"
;;     :objects     (list (m/string 'str :made-of mat/nylon) (m/plate 'body))
;;     :connections (list (m/bow 'bow 'str 0.12)
;;                        (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4)))
;;     :listens     (list (m/listen 'str 0.85)))
;;
;; (check patch) lists what is wrong with a patch by name ("spring bridge: no
;; object bdy"); (dsp patch) assumes a patch that checks clean.
;;
;; Vocabulary used in the comments below:
;;   object      a dict made by string / plate / membrane / finger / mallet
;;   connection  a dict made by pluck / spring / felt / bow ("c" below)
;;   listen      a dict made by listen ("l" below)
;;   param spec  a dict made by pspec ("p" below)
;;   section     a dict made by sections, one per panel section ("sec" below)
;;   form        quoted DGenLisp (or ui.lisp) source, built with quasiquote

(module eseq.modal)

(import eseq.modal.materials :as mat)

(export string plate membrane finger mallet modal-data bar rect-membrane oscillator circ-plate solid
        melt mix-hybrid tube
        pluck spring mount felt bow strike stop fingerboard force position speed listen patch plane
        tone-hole reed lips jet strokes stroke-point
        envelope noise sine constant signal tap-position tap-speed tap-force tap-contact speaker spectral
        pickup amp
        check dsp ui generate)

;; ── small helpers ───────────────────────────────────────────────────────────

;; The symbol id_suffix, the name of everything an object or connection
;; introduces: (nm 'str "freqs") -> str_freqs.
;;   id      the owning object's or connection's id (a symbol)
;;   suffix  what the name is, as a string or symbol
(def nm (id suffix) (symbol id "_" suffix))

;; Append a list of lists into one list.
;;   lists  a list whose items are lists
(def concat-all (lists) (apply append lists))

;; A form summing terms: 0 for none, the term itself for one, else (+ ...).
;;   terms  a list of forms
(def sum-form (terms)
  (if (empty? terms) 0 (if (= (len terms) 1) (first terms) (cons '+ terms))))

;; A param spec: one DGenLisp param of an object, connection or mic, plus
;; how the generated panel shows it.
;;   id       the owner's id; the param is named id_name and grouped by id
;;   name     the param's own name, a string ("length")
;;   default  its initial value (for a menu, the option's index)
;;   lo, hi   its range
;;   options  a list of option strings: makes the param a menu
;;   unit     a unit symbol for @unit ('m, 'mm, 'Hz, 'ms), or nil
;;   title    the panel label (default: name)
;;   taper    the panel knob's taper, :linear or :log
;;   dec      decimals shown (default: picked from hi, 2 / 1 / 0)
(def pspec (id name default lo hi &key (options nil) (unit nil) (title nil) (taper :linear) (dec nil))
  (dict :sym (nm id name) :group id :default default :min lo :max hi :options options :unit unit
        :title (if title title name) :taper taper
        :dec (if dec dec (if (<= hi 10) 2 (if (<= hi 100) 1 0)))))

;; The DGenLisp (param ...) form declaring a param spec: a menu with
;; @options when it has options, else a range with @min / @max (and @unit).
;;   p  a param spec
(def param-form (p)
  (if (get p :options)
    `(param ,(get p :sym) @group ,(get p :group) @options [ ,@(get p :options) ] @default ,(get p :default))
    (append `(param ,(get p :sym) @group ,(get p :group) @default ,(get p :default) @min ,(get p :min) @max ,(get p :max))
            (if (get p :unit) `(@unit ,(get p :unit)) nil))))

;; What an object is made of, as given: one material (eseq.modal.materials)
;; or a list of them to choose between. The materials as a list either way.
;;   made-of  a material or a list of materials
(def material-choices (made-of) (if (mat/material? made-of) (list made-of) made-of))

;; The object's "material" menu param when it offers several materials (the
;; first is the default); none when it is made of one.
;;   id       the object's id
;;   made-of  a material or a list of materials
(def material-params (id made-of)
  (let ((choices (material-choices made-of)))
    (if (= (len choices) 1) nil
      (list (pspec id "material" 0 0 (- (len choices) 1)
                   :options (map (lambda (m) (get m :name)) choices) :title "Material")))))

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

;; strokes with f applied to each one's residues (:re and :im).
;;   f        a function from a list of values to a list
;;   strokes  a list of strokes
(def map-residues (f strokes) (map (lambda (st) (merge st :re (f (get st :re)) :im (f (get st :im)))) strokes))

;; A form for a one-pole decay's per-sample pole: e^(-1 / (seconds sr)).
;;   seconds  a form for the time constant, s
(def pole-form (seconds) `(exp (/ -1 (* ,seconds samplerate))))

;; A form for hz kept below Nyquist (at 0.45 of the sample rate), for filters.
;;   hz  a form
(def below-nyquist (hz) `(min ,hz (* 0.45 samplerate)))

;; White noise in -1..1 (DGenLisp's noise is 0..1).
(def white-noise-form () '(- (* 2 (noise)) 1))

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

;; True when xs[k] appears nowhere before k.
;;   xs  a list
;;   k   a position in it
(def first-occurrence? (xs k) (= (index-of xs (nth xs k)) k))

;; The params every note-following object ends with: mute (extra damping
;; while the note is released), tune (a frequency ratio) and mass (relative
;; to one modal mass).
;;   id                the object's id
;;   tune, mute, mass  their starting values
(def note-params (id tune mute mass)
  (list (pspec id "mute" mute 0 60 :title "Mute")
        (pspec id "tune" tune 0.25 4 :title "Tune" :dec 3)
        (pspec id "mass" mass 0.01 1000 :title "Mass" :taper :log)))

;; ── objects ─────────────────────────────────────────────────────────────────
;; Each constructor returns an object dict:
;;   :id      the name everything the object introduces is prefixed with
;;   :title   the panel section's heading
;;   :kind    picks the emitters below (:string :plate :membrane :two-mass :mallet)
;;   :lumped  true for objects with one coordinate (scalar state, phi = 1)
;;            instead of a mode vector
;;   :modes   how many modes its state vectors hold
;;   :stuff   what it is made of as written (:made-of), for check and the
;;            material defs; objects with a material only. Not :material,
;;            whose value every call auto-quotes (it carries shader code
;;            for widgets)
;;   :params  its param specs, in panel order
;;   :fixed-pitch  true when its pitch is its own (material, size) rather
;;                 than the played note's
;; Lengths are in metres unless a name says otherwise.

;; A stiff string. Its fundamental follows the played note.
;;   id          the object's id ('str)
;;   made-of     a material (mat/steel), or a list of them: a menu param
;;   length      speaking length, m
;;   radius-mm   radius, mm; with material and length it sets the stiffness
;;               (inharmonicity)
;;   modes       how many modes to simulate (fixed when generated)
;;   freq-loss   damping that rises with frequency (panel "HF loss")
;;   const-loss  damping at every frequency (panel "Loss")
;;   mute        extra damping while the note is released
;;   tune        frequency ratio to the played note
;;   planes      1, or 2 for two transverse directions (Modalys' bi-string):
;;               the second is the object (m/plane id 1), with the same
;;               params; connections and listens address either
;;   split       the second direction's frequency ratio to the first (they
;;               beat when it is not 1)
(def string (id &key (made-of mat/steel) (length 0.65) (radius-mm 0.5) (modes 60)
                 (freq-loss 0.4) (const-loss 0.3) (mute 6) (tune 1) (planes 1) (split 1.0007))
  (dict :id id :title "STRING" :kind :string :lumped false :modes modes :stuff made-of :planes planes
        :params (append (material-params id made-of)
                (if (= planes 2) (list (pspec id "split" split 0.99 1.01 :title "Split" :dec 4)) (list))
                (list (pspec id "length" length 0.2 20 :unit 'm :title "Length" :taper :log)
                      (pspec id "radius" radius-mm 0.1 5 :unit 'mm :title "Radius" :taper :log)
                      (pspec id "hf_loss" freq-loss 0 10 :title "HF loss")
                      (pspec id "loss" const-loss 0 20 :title "Loss")
                      (pspec id "mute" mute 0 60 :title "Mute")
                      (pspec id "tune" tune 0.25 4 :title "Tune" :dec 3)))))

;; A simply supported rectangular plate, 8 x 8 modes. Its pitch comes from its
;; material and size, not the note (a soundboard or body).
;;   id        the object's id ('body)
;;   made-of   a material (mat/spruce), or a list of them: a menu param
;;   thick-mm  thickness, mm
;;   width     width, m
;;   height    height, m
;;   mass      its mass relative to one modal mass of a string; heavier
;;             plates move less when a connection pushes them
;;   loss      damping at every frequency
;;   damp      damping that rises with frequency (panel "HF damp")
;;   grid      modes per side: the plate has grid x grid modes (fixed when
;;             generated)
(def plate (id &key (made-of mat/spruce) (thick-mm 3) (width 0.45) (height 0.35)
                (mass 30) (loss 4) (damp 0.3) (grid 8))
  (dict :id id :title "PLATE" :kind :plate :lumped false :grid grid :modes (* grid grid) :stuff made-of
        :fixed-pitch true
        :params (append (material-params id made-of)
                (list (pspec id "thick" thick-mm 0.5 20 :unit 'mm :title "Thick" :taper :log)
                      (pspec id "mass" mass 1 1000 :title "Mass" :taper :log)
                      (pspec id "width" width 0.05 2 :title "Width")
                      (pspec id "height" height 0.05 2 :title "Height")
                      (pspec id "loss" loss 0 40 :title "Loss")
                      (pspec id "damp" damp 0 5 :title "HF damp")))))

;; A circular drum head with 120 Bessel modes. Its fundamental follows the
;; played note.
;;   id      the object's id ('head)
;;   loss    damping at every frequency (panel "Decay")
;;   bright  damping that rises with frequency (panel "Dark")
;;   choke   extra damping at every frequency, for a hand on the head
;;   modes   how many of the 120 modes sound (a param; the rest are masked)
;;   tune    frequency ratio to the played note
(def membrane (id &key (loss 6) (bright 1) (choke 0) (modes 120) (tune 1))
  (dict :id id :title "HEAD" :kind :membrane :lumped false :modes 120
        :params (list (pspec id "loss" loss 0 60 :title "Decay")
                      (pspec id "bright" bright 0 10 :title "Dark")
                      (pspec id "choke" choke 0 60 :title "Choke")
                      (pspec id "tune" tune 0.25 4 :title "Tune" :dec 3)
                      (pspec id "modes" modes 1 120 :title "Modes" :dec 0))))

;; A free bar (Euler-Bernoulli beam, free at both ends): marimba, xylophone,
;; glockenspiel, vibraphone bars. Its fundamental follows the played note;
;; the uppers sit at the free bar's 1 : 2.756 : 5.404 : 8.933 ...
;;   id        the object's id ('bar)
;;   modes     how many modes (fixed when generated)
;;   undercut  0 keeps the natural ratios; 1 tunes modes 2 and 3 to 4 and
;;             10 times the fundamental, as a marimba bar's arch does (a
;;             spectral approximation of undercutting, not its geometry)
;;   loss      damping at every frequency
;;   hf-loss   damping that rises with frequency (panel "HF loss")
;;   mute      extra damping while the note is released
;;   tune      frequency ratio to the played note
;;   mass      relative to one modal mass, as for a plate
(def bar (id &key (modes 12) (undercut 0) (loss 3) (hf-loss 0.6) (mute 0) (tune 1) (mass 1))
  (dict :id id :title "BAR" :kind :bar :lumped false :modes modes
        :params (append (list (pspec id "undercut" undercut 0 1 :title "Undercut")
                              (pspec id "loss" loss 0 40 :title "Loss")
                              (pspec id "hf_loss" hf-loss 0 10 :title "HF loss"))
                        (note-params id tune mute mass))))

;; A rectangular membrane, fixed at its edges (Modalys rect-membrane):
;; grid x grid modes at f0 sqrt((m / aspect)^2 + n^2), scaled so mode (1, 1)
;; follows the played note.
;;   id      the object's id ('head)
;;   grid    modes per side (fixed when generated)
;;   aspect  width over height
;;   loss    damping at every frequency (panel "Decay")
;;   hf-loss damping that rises with frequency (panel "Dark")
;;   mute    extra damping while the note is released
;;   tune    frequency ratio to the played note
;;   mass    relative to one modal mass
(def rect-membrane (id &key (grid 8) (aspect 1.4) (loss 6) (hf-loss 1) (mute 0) (tune 1) (mass 1))
  (dict :id id :title "MEMBRANE" :kind :rect-membrane :lumped false :grid grid :modes (* grid grid)
        :params (append (list (pspec id "aspect" aspect 0.2 5 :title "Aspect" :taper :log)
                              (pspec id "loss" loss 0 60 :title "Decay")
                              (pspec id "hf_loss" hf-loss 0 10 :title "Dark"))
                        (note-params id tune mute mass))))

;; A circular plate (Modalys clamped-circ-plate / free-circ-plate), Kirchhoff
;; bending: cymbals, gongs, bells' crowns, tam-tams, steel discs. Its pitch
;; comes from its material and size (and tune), not the note. The modes are
;; every root of the plate's Bessel-function frequency equation, solved
;; natively (modal-circ-plate-modes, Leissa 1969); a free edge's depend on
;; Poisson's ratio (that of the first material it is made of).
;;   id        the object's id ('cym)
;;   edge      :free (cymbals, gongs) or :clamped
;;   made-of   a material, or a list of them: a menu param
;;   radius    m
;;   thick-mm  thickness, mm
;;   modes     how many modes (fixed when generated)
;;   loss      damping at every frequency
;;   damp      damping that rises with frequency (panel "HF damp")
;;   tune      frequency ratio
;;   mass      relative to one modal mass, as for a plate
(def circ-plate (id &key (edge :free) (made-of mat/bronze) (radius 0.2) (thick-mm 1.5) (modes 48)
                    (loss 2) (damp 0.4) (tune 1) (mass 2))
  (let ((nu (if (made-of? made-of) (get (first (material-choices made-of)) :poisson) 0.3))
        (found (modal-circ-plate-modes edge nu modes (circ-samples))))
  (dict :id id :title (if (= edge :clamped) "CLAMPED PLATE" "FREE PLATE") :kind :circ-plate
        :lumped false :modes (len found) :plate-modes found :edge edge :stuff made-of :fixed-pitch true
        :params (append (material-params id made-of)
                (list (pspec id "radius" radius 0.02 2 :unit 'm :title "Radius" :taper :log)
                      (pspec id "thick" thick-mm 0.1 20 :unit 'mm :title "Thick" :taper :log)
                      (pspec id "mass" mass 0.1 1000 :title "Mass" :taper :log)
                      (pspec id "loss" loss 0 40 :title "Loss")
                      (pspec id "damp" damp 0 5 :title "HF damp")
                      (pspec id "tune" tune 0.25 8 :title "Tune" :dec 3))))))

;; A harmonic oscillator (Modalys harmonic-oscillator): one mode, a mass on
;; a spring, at the played note. The usual reed or lip, or a resonance to
;; couple a string to. It is a one-mode data object.
;;   id    the object's id ('osc)
;;   loss  its decay rate, 1/s
;;   tune  frequency ratio to the played note
;;   mass  relative to one modal mass
(def oscillator (id &key (loss 5) (tune 1) (mass 1))
  (merge (modal-data id :freqs (list 440) :losses (list loss) :root 440 :tune tune :mass mass)
         :title "OSC" :label "oscillator"))

;; A finger (Modalys' two-mass): a hand on a trajectory (from +start to
;; -depth over stroke-ms after each hit) with a soft tip hung off it. Pair it
;; with pluck.
;;   id         the object's id ('fin)
;;   hz         resonance of the flesh between hand and tip (panel "Flesh")
;;   damp       damping of that resonance, 0.05..0.9
;;   stiff      the tip's stiffness: the higher, the less a contact force
;;              moves it
;;   start      where the hand starts, above the string (panel "From")
;;   depth      where it ends, below the string (panel "To")
;;   stroke-ms  how long the stroke takes, ms (panel "Time")
(def finger (id &key (hz 300) (damp 0.5) (stiff 2) (start 0.1) (depth 0.2) (stroke-ms 30))
  (dict :id id :title "FINGER" :kind :two-mass :lumped true
        :params (list (pspec id "hz" hz 20 2000 :unit 'Hz :title "Flesh" :taper :log)
                      (pspec id "damp" damp 0.05 0.9 :title "Damp")
                      (pspec id "stiff" stiff 0.1 20 :title "Stiff" :taper :log)
                      (pspec id "start" start 0.01 0.5 :title "From")
                      (pspec id "depth" depth 0.01 1 :title "To")
                      (pspec id "stroke" stroke-ms 1 1000 :unit 'ms :title "Time" :taper :log))))

;; A mass thrown at an object on each hit. Pair it with felt.
;;   id     the object's id ('mal)
;;   mass   relative to one modal mass of what it strikes; useful values are
;;          0.002..0.5 (a 120-mode head's point is ~60x lighter than a
;;          string's)
;;   speed  throw speed; scaled by the hit's velocity
(def mallet (id &key (mass 0.01) (speed 1))
  (dict :id id :title "MALLET" :kind :mallet :lumped true
        :params (list (pspec id "mass" mass 0.002 0.5 :title "Mass" :taper :log :dec 3)
                      (pspec id "speed" speed 0.1 4 :title "Speed"))))

;; A modal object defined by data (Modalys' single-point / multiple-points):
;; its modes' frequencies and losses, and their shapes over a set of points.
;; A finite-element solid, a measured resonance model, or numbers written by
;; hand. Its fundamental is fixed unless the data names the pitch it was
;; tuned at (root), when the modes follow the played note. The modes are
;; sorted by frequency (with their losses and shape columns).
;;   id          the object's id ('bell)
;;   data        a dict whose :freqs :losses :bandwidths :amps :shapes
;;               :points :root :mass fill the arguments below that are not
;;               given: what (modal-read-data "bell.modes.json") or
;;               (modal-read-data "bell.txt") returns, or your own (an error
;;               message from modal-read-data is reported by check)
;;   freqs       mode frequencies, Hz
;;   losses      per-mode decay rates, 1/s
;;   bandwidths  per-mode -3 dB bandwidths, Hz (decay rate pi bw), when no
;;               losses are given; 1 Hz each when neither is
;;   amps        one gain per mode, for an object heard at a single point
;;   shapes      one list of N values per point: each mode's shape over the
;;               points; default a single point with amps (or all 1)
;;   points      one (x y z) per point, so accesses can name a position
;;   root        the frequency the data was tuned at; nil keeps it fixed
;;   follow      :octave to transpose by the played note's octave (from the
;;               C4 octave, -3..+3), so twelve keys can each play a stroke
;;               of one cymbal (see strokes); nil otherwise
;;   strokes     measured strokes, one dict per key from C: :re and :im (the
;;               complex residue, one value per mode, a hit injects: :im is
;;               heard at once, :re a quarter cycle on), :name, and
;;               :click (dict :rms :decay-s :hz); played by m/strokes
;;   tune        frequency ratio (to the note when root is given)
;;   decay       multiplies every loss (panel "Decay x")
;;   muffle      tape or a resting hand: loss growing with sqrt(f), up to
;;               40/s at 1 kHz (panel "Muffle")
;;   mute        extra damping while the note is released (a choke)
;;   mass        relative to one modal mass, as for a plate (default 1)
;;   air-loss    whether to add modal-loss's air loss; default true unless
;;               the data is :measured (its losses already hold it)
;; An eseq-modes file of an identified model (tools/pm-ride/export_modes.py)
;; also carries :strokes, :wash and :contact-s for m/strokes.
(def modal-data (id &key (data nil) (freqs nil) (losses nil) (bandwidths nil) (amps nil)
                    (shapes nil) (points nil) (root nil) (follow nil) (strokes nil) (tune 1) (decay 1)
                    (muffle 0) (mute 0) (mass nil) (air-loss nil))
  (let ((table (if (map? data) data nil))
        (given (lambda (value key) (if value value (if table (get table key) nil))))
        (fs (given freqs :freqs))
        (n (if (list? fs) (len fs) 0))
        ;; already sorted data (a solver's, a fitter's) is left as it is
        (sorted (or (< n 2) (empty? (filter (lambda (k) (> (nth fs k) (nth fs (+ k 1)))) (range (- n 1))))))
        (order (if sorted (range n) (sort-by-value fs)))
        (reorder (lambda (xs) (if (and (not sorted) (list? xs) (= (len xs) n)) (map (lambda (k) (nth xs k)) order) xs)))
        (ls (given losses :losses))
        (bw (given bandwidths :bandwidths))
        (am (given amps :amps))
        (sh (given shapes :shapes))
        (st (given strokes :strokes))
        (measured (= (given nil :measured) true))
        (root-hz (given root :root))
        ;; its coefficients follow the note: computed when a note starts
        ;; and when an input changes, not every sample (see held-pass)
        (held (or (not (= root-hz nil)) (not (= follow nil))))
        (ones (map (lambda (k) 1) (range n))))
    (pad-one-mode
    (dict :id id :title "MODES" :label "modal-data" :kind :data :lumped false :modes n
          :data-error (if (string? data) data nil)
          :freqs (reorder fs)
          :losses (reorder (if ls ls (map (lambda (b) (* 3.14159265 b)) (if bw bw ones))))
          :shapes (map reorder (if (list? sh) sh (list (if am am ones))))
          :points (given points :points)
          :strokes (if (list? st) (map-residues reorder st) nil)
          :wash (given nil :wash)
          :contact-s (given nil :contact-s)
          :air-loss (if (= air-loss nil) (not measured) air-loss)
          ;; the eseq-modes file it came from, for the panel's mesh view
          :mesh-file (given nil :file)
          :root root-hz
          :follow follow
          :fixed-pitch (not held)
          :held held
          :params (append (list (pspec id "decay" decay 0.05 20 :title "Decay x" :taper :log)
                                (pspec id "muffle" muffle 0 1 :title "Muffle"))
                          (note-params id tune mute (if mass mass (if (given nil :mass) (given nil :mass) 1))))))))

;; A one-mode data object with a second, silent mode (zero shape, so never
;; driven or heard): DGenLisp mishandles one-element tensors, and a one-mode
;; object (an oscillator) would not move at all (eseq-k0m2.32).
;;   obj  a data object
(def pad-one-mode (obj)
  (if (not (= (get obj :modes) 1)) obj
    (merge obj :modes 2
           :freqs (append (get obj :freqs) (list 100000))
           :losses (append (get obj :losses) (list 1))
           :shapes (map (lambda (row) (append row (list 0))) (get obj :shapes))
           :strokes (if (get obj :strokes) (map-residues (lambda (xs) (append xs (list 0))) (get obj :strokes)) nil))))

;; A finite-element solid (Modalys' finite-element objects): any 3-D shape,
;; solved for its modes by modal-solve (the first time; then from a cache)
;; and played as a data object. Its pitch is its own (material and size)
;; unless root makes it follow the note.
;;   id          the object's id ('bell)
;;   shape       the solid, as in a spec file's "solid": e.g.
;;               (dict :revolve (dict :profile (list (list r z wall) ...)))
;;               or (dict :box (dict :size (list 0.4 0.05 0.012))); see
;;               tools/modal/README.md
;;   spec        instead of shape: a whole spec file (relative to the
;;               script), material and all
;;   made-of     its material (one; menus need a re-solve per material)
;;   modes       how many modes to solve for
;;   resolution  the spec's resolution, e.g. (dict :cells 32)
;;   block       a region clamped in place, as in a spec; free when nil
;;   loss        damping at every frequency, 1/s
;;   hf-loss     damping that rises with frequency (as the string's)
;;   root, tune, decay, mute, mass  as for modal-data
(def solid (id &key (shape nil) (spec nil) (made-of mat/steel) (modes 40) (resolution nil) (block nil)
               (loss 2) (hf-loss 0.3) (root nil) (tune 1) (decay 1) (mute 0) (mass nil))
  (let ((m (first (material-choices made-of)))
        ;; :made-of, not :material: a call quotes whatever follows :material
        (base (dict :solid shape :modes modes :made-of m))
        (with-res (if resolution (merge base :resolution resolution) base))
        (data (modal-solve (if spec spec (if block (merge with-res :block block) with-res))))
        (losses (if (map? data) (map (lambda (f) (+ loss (* hf-loss f 0.01))) (get data :freqs)) nil)))
    (merge (modal-data id :data data :losses losses :root root :tune tune :decay decay :mute mute :mass mass)
           :title "SOLID" :label "solid")))

;; The positions of xs's numbers in increasing order (insertion sort; a few
;; hundred modes at most).
;;   xs  a list of numbers
(def sort-by-value (xs)
  (reduce (lambda (sorted k)
            (append (filter (lambda (j) (<= (nth xs j) (nth xs k))) sorted)
                    (list k)
                    (filter (lambda (j) (> (nth xs j) (nth xs k))) sorted)))
          (list) (range (len xs))))

;; An air column (Modalys' closed-closed / closed-open / open-open tubes):
;; its modes are standing pressure waves along the bore, the fundamental on
;; the played note. x = 0 is the mouthpiece end.
;;   closed-open  odd harmonics, pressure shapes cos((2k-1) pi x / 2): a
;;                clarinet's cylinder
;;   open-open    every harmonic, sin(k pi x): a flute
;;   closed-closed (or :cone, a conical bore's complete series, as a
;;                closed-closed approximation) every harmonic, cos(k pi x):
;;                a saxophone, an oboe, brass with lips
;; Losses: a constant, a viscothermal wall term rising with sqrt(f), and
;; radiation rising with f (hf-loss) plus modal-loss's air.
;;   id      the object's id ('bore)
;;   ends    :closed-open, :open-open, :closed-closed or :cone
;;   modes   how many modes (fixed when generated)
;;   loss    damping at every frequency
;;   wall    the sqrt(f) damping of the bore walls
;;   hf-loss damping that rises with frequency (radiation)
;;   tune, mute, mass  as for the other note-following objects
(def tube (id &key (ends :closed-open) (modes 20) (loss 2) (wall 1) (hf-loss 0.3) (mute 30) (tune 1) (mass 1))
  (dict :id id :title "TUBE" :kind :tube :lumped false :modes modes :ends ends
        :params (append (list (pspec id "loss" loss 0 40 :title "Loss")
                              (pspec id "wall" wall 0 10 :title "Wall")
                              (pspec id "hf_loss" hf-loss 0 10 :title "HF loss"))
                        (note-params id tune mute mass))))

;; A form for a tube's pressure mode shapes at x (sin for open-open, else
;; cos, of wave pi x), masked; with gradient, their derivative in x (the
;; air's velocity there, which steers a jet).
;;   obj       a tube
;;   x         the position form
;;   gradient  true for d/dx
(def tube-shape (obj x gradient)
  (let ((open (= (get obj :ends) :open-open)) (arg `(* ,(p obj "wave") 3.14159265 ,x)))
    (if gradient
      `(* ,(p obj "wave") 3.14159265 ,(if open `(cos ,arg) `(* -1 (sin ,arg))) ,(p obj "mask"))
      `(* (,(if open 'sin 'cos) ,arg) ,(p obj "mask")))))

;; A tube's mode k (from 0): its frequency ratio to the fundamental, and its
;; pressure shape's wavenumber over pi (cos for a closed mouthpiece end, sin
;; for an open one).
;;   ends  as for tube
;;   k     the mode
(def tube-ratio (ends k) (if (= ends :closed-open) (+ (* 2 k) 1) (+ k 1)))
(def tube-wave (ends k) (if (= ends :closed-open) (* 0.5 (+ (* 2 k) 1)) (+ k 1)))

;; A melt hybrid (Modalys' melt-hybrid): one object whose modal data is a
;; blend of two objects', so morphing changes what it is (a string into a
;; plate, one material into another) while it rings. Frequencies blend in
;; log frequency, losses, coupling and mode shapes linearly. The parts are
;; objects made for it (not listed in the patch), with the same number of
;; modes; their params stay on the panel. A position on it is (list atA atB).
;;   id     the object's id ('h)
;;   a, b   the two objects (dicts, from m/string, m/plate, ...)
;;   morph  0 is a, 1 is b (a param)
(def melt (id a b &key (morph 0.5))
  (dict :id id :title "MELT" :kind :melt :lumped false :modes (get a :modes) :parts (list a b)
        :params (append (list (pspec id "morph" morph 0 1 :title "Morph")) (get a :params) (get b :params))))

;; A mix hybrid (Modalys' mix-hybrid): two objects kept whole, a connection or
;; listen on it shared between them, (1 - morph) to a and morph to b (both
;; pitches heard). A connection end on it moves (1 - x) ya + x yb and
;; pushes each by its share of the force. A position on it is (list atA atB).
;;   id     the hybrid's id ('h)
;;   a, b   the two objects' ids (modal objects in the patch)
;;   morph  0 is all a, 1 all b (a param)
(def mix-hybrid (id a b &key (morph 0.5))
  (dict :id id :title "MIX" :kind :mix :lumped false :parts (list a b)
        :params (list (pspec id "morph" morph 0 1 :title "Morph"))))

;; True for an object with its own state (not a mix, which only routes).
;;   obj  an object
(def physical? (obj) (not (= (get obj :kind) :mix)))

;; True for a hybrid of either kind.
;;   obj  an object
(def hybrid? (obj) (member? (list :melt :mix) (get obj :kind)))

;; ── connections ─────────────────────────────────────────────────────────────
;; Each constructor returns a connection dict:
;;   :id         the name everything the connection introduces is prefixed with
;;   :title      the panel section's heading
;;   :law        the force law (:pluck :spring :felt :bow)
;;   :a, :b      the two ends' object ids; a is the driver / upper end, b the
;;               driven one: the law's F pushes b by +F and a by -F. A bow
;;               has no a.
;;   :a-at :b-at each end's access point, nil for a lumped end. A position is
;;               a number 0..1 along a string, (list x y) on a plate (each
;;               0..1), or (list r theta) on a membrane (r 0..0.95, theta in
;;               radians). Positions become params. A mount's string end
;;               (0 or 1) is its :end, not a position (its :b-at is nil).
;;   :params     the law's own param specs
;;   :weight     its starting weight (every connection's weight param)
;; resolve-connection later adds :a-lumped / :b-lumped.

;; A finger plucking an object: the tip catches it, holds it as the hand
;; moves on, and lets go once pulled far enough.
;;   id         the connection's id ('pick)
;;   a          the finger's id
;;   b          the plucked object's id
;;   at         where on b it plucks
;;   stiff      how hard the tip grips (panel "Grip")
;;   threshold  how far the tip pulls b before it slips off (panel
;;              "Release"); softer hits release sooner
(def pluck (id a b at &key (stiff 20) (threshold 0.05) (weight 1))
  (dict :id id :title "PLUCK" :law :pluck :a a :b b :a-at nil :b-at at :weight weight
        :params (list (pspec id "stiff" stiff 0.5 200 :title "Grip" :taper :log :dec 1)
                      (pspec id "thresh" threshold 0.002 0.3 :title "Release" :taper :log :dec 3))))

;; A spring joining two objects' access points: a bridge, or glue.
;;   id         the connection's id ('bridge)
;;   a, b       the two objects' ids
;;   a-at       where on a it attaches
;;   b-at       where on b it attaches
;;   stiffness  how stiff it is (panel "Glue"); any value is stable
(def spring (id a a-at b b-at &key (stiffness 2) (weight 1))
  (dict :id id :title "SPRING" :law :spring :a a :b b :a-at a-at :b-at b-at :weight weight
        :params (list (pspec id "stiff" stiffness 0 50 :title "Glue" :dec 1))))

;; A string's end mounted on a point of another object, one way: as a
;; guitar's body carries its strings' bridge and nut, the mount's motion
;; shakes the string and takes nothing back (a solid body is far heavier
;; than its strings). It is a base excitation: each mode is pushed by its
;; share of the end's motion, 2 / (n pi) (alternating in sign for the end
;; at 1), times the mount's acceleration, so the string moves relative to
;; the mount, as a pickup on the body hears it. Off the string's resonances
;; the mount's motion barely reaches it, so a muted string stops a loop
;; through its body.
;;   id      the connection's id ('bridge)
;;   a       the mount's object (a body)
;;   a-at    where on it the end sits
;;   b       the string's id: the direction it names ((m/plane 'str 1) for a
;;           two-direction string's second), as the mount's point moves in one
;;   end     which end is mounted: 0 or 1
;;   gain    the coupling, 1 as rigid
;;   weight  0..1
(def mount (id a a-at b end &key (gain 1) (weight 1))
  (dict :id id :title "MOUNT" :law :mount :a a :b b :a-at a-at :b-at nil :end end :weight weight
        :params (list (pspec id "gain" gain 0 4 :title "Coupling"))))

;; True when c is a mount.
;;   c  a connection
(def mount? (c) (= (get c :law) :mount))

;; True when c moves its object's modes directly (pushes) rather than by a
;; force at a point: strokes and mounts.
;;   c  a connection
(def pusher? (c) (or (strokes? c) (mount? c)))

;; A one-sided felt between a mallet and the object it strikes. By default it
;; hardens with the last sample's squash; given an exponent it is Stulov's
;; hysteretic felt (Modalys' felt): F = Q ((u / u0)^p - eps h), with u the
;; compression, u0 = 1 mm of the mallet's drop and h (u / u0)^p remembered
;; over tau, so it pushes back less on the way out than on the way in.
;;   id          the connection's id ('hit)
;;   a           the mallet's id
;;   b           the struck object's id
;;   b-at        where on b it strikes
;;   felt        the felt's stiffness (Stulov's Q)
;;   hardness    how much it stiffens as it is squashed, 0..1 (panel "Hard";
;;               the default law only)
;;   exponent    Stulov's p (typically 2.2..3.5); nil for the default law
;;   hysteresis  Stulov's eps, 0..0.9
;;   tau-ms      how long the felt remembers, ms
;;   weight      0..1
(def felt (id a b b-at &key (felt 0.005) (hardness 0.5) (exponent nil) (hysteresis 0.2) (tau-ms 0.5) (weight 1))
  (dict :id id :title "FELT" :law :felt :a a :b b :a-at nil :b-at b-at :weight weight :stulov (not (= exponent nil))
        :params (if exponent
                  (list (pspec id "felt" felt 0.0005 5 :title "Felt" :taper :log :dec 4)
                        (pspec id "exponent" exponent 1 5 :title "Exponent")
                        (pspec id "hysteresis" hysteresis 0 0.9 :title "Hysteresis")
                        (pspec id "tau" tau-ms 0.01 20 :unit 'ms :title "Memory" :taper :log))
                  (list (pspec id "felt" felt 0.0005 5 :title "Felt" :taper :log :dec 4)
                        (pspec id "hardness" hardness 0 1 :title "Hard")))))

;; A stiff one-sided contact between two objects' points (Modalys' strike):
;; pushes them apart while a's point is below b's minus offset (offset is the
;; gap between them at rest), and nothing otherwise. Implicit, so any stiffness is stable.
;;   id         the connection's id ('rattle)
;;   a, a-at    the upper object and its point
;;   b, b-at    the lower object and its point
;;   stiffness  relative to the points' give (large is nearly rigid)
;;   offset     the gap at rest
;;   weight     0..1
(def strike (id a a-at b b-at &key (stiffness 200) (offset 0) (weight 1))
  (dict :id id :title "STRIKE" :law :strike :a a :b b :a-at a-at :b-at b-at :weight weight
        :params (list (pspec id "stiff" stiffness 1 5000 :title "Hard" :taper :log :dec 0)
                      (pspec id "offset" offset -0.02 0.02 :title "Gap" :dec 4))))

;; A rigid stop under b's point (Modalys' unilateral strike): the point cannot
;; go below height. A sitar's jawari or a buzzing fret: a string that rings
;; against it buzzes.
;;   id         the connection's id ('jaw)
;;   b, at      the object and its point
;;   height     where the stop is (below rest is negative)
;;   stiffness  relative to the point's give
;;   weight     0..1
(def stop (id b at &key (height -0.002) (stiffness 200) (weight 1))
  (dict :id id :title "STOP" :law :stop :a nil :b b :a-at nil :b-at at :weight weight
        :params (list (pspec id "stiff" stiffness 1 5000 :title "Hard" :taper :log :dec 0)
                      (pspec id "height" height -0.02 0.02 :title "Height" :dec 4))))

;; Measured strokes played on a data object (an identified model, such as
;; the ride in tools/modal/examples/ride-kit.lisp): the key picks a stroke
;; (C, C#, ... in every octave, wrapping onto the strokes the object has),
;; and the hit injects that stroke's measured complex residue into every
;; mode, on top of whatever is still ringing, so close modes keep their
;; recorded phase and beat as they did. The stick: the residues are the
;; modes' response to the measured contact; Hardness shortens or lengthens
;; it (a finite energy budget: a harder stick moves energy up, never adds
;; it), and Vel > timbre softens soft hits. A stroke's click (stick noise
;; above the modes) and, when the data has one, the wash (dense weak modes
;; as band noise, with each stroke's measured level, build-up and decay)
;; sound with it; both follow the object's Decay, Muffle and Mute.
;;   id          the connection's id ('stick)
;;   b           the struck data object (with :strokes)
;;   hardness    stick hardness 0..1 (0.5 is the measured stick)
;;   dynamics    how much softer a soft hit's contact is (Vel > timbre)
;;   click       the click's level, 0..3 (1 as measured)
;;   click-tone  shifts the click's centre by -1.5..+1.5 octaves
;;   wash        the wash's level, 0..2 (1 as measured)
;;   weight      0..1, the strokes' level
(def strokes (id b &key (hardness 0.5) (dynamics 0.5) (click 1) (click-tone 0) (wash 1) (weight 1))
  (dict :id id :title "STROKES" :law :strokes :a nil :b b :a-at nil :b-at nil :weight weight
        :params (list (pspec id "hardness" hardness 0 1 :title "Hardness")
                      (pspec id "dynamics" dynamics 0 1 :title "Vel > timbre")
                      (pspec id "click" click 0 3 :title "Click")
                      (pspec id "click_tone" click-tone -1 1 :title "Click tone")
                      (pspec id "wash" wash 0 2 :title "Wash"))))

;; The point of a data object (or the data it was read from) where its
;; stroke called name struck, for a physical connection there (an identified
;; model's point k + 1 is stroke k's spot; see tools/pm-ride/export_modes.py);
;; 0, the microphone, when no stroke has that name.
;;   data  a data dict or data object, with :strokes
;;   name  the stroke's name ("Ride", "Bell")
(def stroke-point (data name)
  (let ((found (filter (lambda (st) (= (get st :name) name)) (if (list? (get data :strokes)) (get data :strokes) (list)))))
    (if (empty? found) 0 (if (get (first found) :point) (get (first found) :point) 0))))

;; True when c plays measured strokes.
;;   c  a connection
(def strokes? (c) (= (get c :law) :strokes))

;; A bow driving b's access point at a steady speed while the note is held,
;; by stick-slip friction (see content/defmacros/bow-contact). It has no
;; object of its own: Modalys drives the bow through a position connection;
;; here its speed follows the gate through attack / release.
;;   id          the connection's id ('bow)
;;   b           the bowed object's id
;;   at          where on b it bows
;;   pressure    bow force relative to the point's give; lifts as the bow
;;               slows
;;   speed       bow speed at full velocity
;;   slip        how wide the stick region is, relative to the speed
;;   attack-ms   time for the bow to come up to speed, ms
;;   release-ms  time for it to stop after the note is released, ms
;;   hair        on a string with :planes 2: the hair's depth into the
;;               second direction; the bow then presses that direction down,
;;               and its friction pressure is the pressure param times the
;;               contact force over its 50 ms mean (nil: a constant pressure)
(def bow (id b at &key (pressure 0.2) (speed 0.2) (slip 1) (attack-ms 60) (release-ms 120) (hair nil) (weight 1))
  (dict :id id :title "BOW" :law :bow :a nil :b b :a-at nil :b-at at :weight weight :hair hair
        :params (append (list (pspec id "pressure" pressure 0.01 2 :title "Pressure" :taper :log)
                              (pspec id "speed" speed 0.01 1 :title "Speed" :taper :log)
                              (pspec id "slip" slip 0.1 3 :title "Slip")
                              (pspec id "attack" attack-ms 1 1000 :unit 'ms :title "Attack" :taper :log)
                              (pspec id "release" release-ms 1 2000 :unit 'ms :title "Release" :taper :log))
                        (if hair (list (pspec id "hair" hair 0.0001 0.02 :title "Hair" :dec 4)) (list)))))

;; Every connection's weight: 0..1 of its force (of its pressure for a bow),
;; for fading an interaction in or out or coupling partly (Modalys' weight).
;; Scaling an implicit force down never makes it unstable.
;;   id      the connection's id
;;   weight  the starting weight
(def weight-param (id weight) (pspec id "weight" weight 0 1 :title "Weight"))

;; A finger pressing a string onto a fingerboard (Modalys' mono-fingerboard):
;; while press is up the string is clamped at its point (a stiff pull to
;; rest, scaled by the press signal), and a rigid board under that point lets
;; the string buzz against it. Moving the point slides the fret.
;;   id         the connection's id ('fret)
;;   b, at      the string and where the finger stops it
;;   press      a signal 0..1 for how hard the finger presses (default an
;;              envelope on the note)
;;   stiffness  how hard the press holds, relative to the point's give
;;   board      the board's height under the string (negative)
;;   weight     0..1
(def fingerboard (id b at &key (press (envelope :attack-ms 3 :sustain 1 :release-ms 30)) (stiffness 300)
                       (board -0.0015) (weight 1))
  (list (dict :id id :title "FINGERBOARD" :law :fingerboard :a nil :b b :a-at nil :b-at at :signal press :weight weight
              :params (append (list (pspec id "stiff" stiffness 0 2000 :title "Hold" :taper :log :dec 0))
                              (signal-params id press)))
        (merge (stop (nm id "board") b at :height board) :coords-of id)))

;; ── air ─────────────────────────────────────────────────────────────────────
;; Blown connections on a tube's mouthpiece point. Each takes a breath
;; signal (default an envelope on the note) scaling its blowing pressure.

;; A tone hole: the tube's pressure released at a point while it is open
;; (a stiff pull of the pressure toward 0, weighted by open), which shortens
;; the column acoustically and raises its pitch.
;;   id    the connection's id ('h1)
;;   tube  the tube's id
;;   at    where along it
;;   open  0 closed .. 1 open (its weight param)
;;   size  how fully it releases, relative to the point's give
(def tone-hole (id tube at &key (open 1) (size 300))
  (merge (position id tube at :stiffness size :weight open) :title "TONE HOLE"))

;; A single reed (the normalised clarinet model, Modalys' normalised-valve):
;; with blowing pressure gamma and the mouthpiece pressure p (both relative
;; to the pressure that shuts the reed), the flow into the tube is
;; zeta (1 - gamma + p) sqrt|gamma - p| (signed), and 0 while p is below
;; gamma - 1 (the reed beats shut). p is solved with the flow it causes,
;; inside the sample (12 bisection steps).
;;   id        the connection's id ('reed)
;;   tube      the tube's id
;;   at        the mouthpiece point (0, a closed end)
;;   blow      gamma at full breath (it sounds above about 1/3)
;;   zeta      the reed's opening (brightness)
;;   coupling  how strongly the flow drives the tube
;;   scale     the tube pressure that counts as the reed's closing pressure
;;   breath    a signal 0..1
;;   weight    0..1
(def reed (id tube at &key (blow 0.6) (zeta 0.3) (coupling 10) (scale 1)
                           (breath (envelope :attack-ms 30 :sustain 1 :release-ms 40)) (weight 1))
  (dict :id id :title "REED" :law :reed :a nil :b tube :a-at nil :b-at at :signal breath :weight weight
        :params (append (list (pspec id "blow" blow 0 1.5 :title "Blow")
                              (pspec id "zeta" zeta 0.05 1.5 :title "Reed")
                              (pspec id "coupling" coupling 0.001 100 :title "Couple" :taper :log :dec 3)
                              (pspec id "scale" scale 0.001 10 :title "Scale" :taper :log :dec 3))
                        (signal-params id breath))))

;; Lips on a brass mouthpiece (Modalys' valve): an outward-striking lip,
;; pushed open by the pressure difference gamma - p, whose opening
;; max(0, 1 + y / rest) lets a flow zeta h sqrt|gamma - p| (signed) into the
;; tube. The flow is solved with the pressure it causes; the lip moves by
;; beta (gamma - p) from the mouthpiece's pressure before this sample's flow.
;;   id        the connection's id ('lips)
;;   lip       the lip object's id (m/oscillator, near the note)
;;   tube      the tube's id
;;   at        the mouthpiece point
;;   blow      gamma at full breath
;;   zeta      the flow's scale
;;   beta      how hard the pressure pushes the lip
;;   rest      the lip displacement that closes it
;;   coupling, scale, breath, weight  as for reed
;;   lip-at    the lip's point (0 for an oscillator)
(def lips (id lip tube at &key (blow 0.7) (zeta 0.5) (beta 0.1) (rest 0.05) (coupling 1) (scale 1)
                               (breath (envelope :attack-ms 40 :sustain 1 :release-ms 50)) (lip-at 0) (weight 1))
  (dict :id id :title "LIPS" :law :lips :a lip :b tube :a-at lip-at :b-at at :signal breath :weight weight :split true
        :params (append (list (pspec id "blow" blow 0 1.5 :title "Blow")
                              (pspec id "zeta" zeta 0.05 2 :title "Flow")
                              (pspec id "beta" beta 0.0001 1 :title "Push" :taper :log :dec 4)
                              (pspec id "rest" rest 0.001 1 :title "Rest" :taper :log :dec 3)
                              (pspec id "coupling" coupling 0.001 100 :title "Couple" :taper :log :dec 3)
                              (pspec id "scale" scale 0.001 10 :title "Scale" :taper :log :dec 3))
                        (signal-params id breath))))

;; An air jet across a flute's embouchure (Modalys' jet + labium): the
;; breath plus turbulence plus the mouth's own acoustic velocity (last
;; sample), delayed by the jet's travel to the edge and saturated (tanh),
;; drives the tube. The delay is ratio periods of the note: about half a
;; period sounds the fundamental, less overblows.
;;   id        the connection's id ('jet)
;;   tube      the tube's id
;;   at        the embouchure point (near an open end)
;;   blow      breath level
;;   ratio     the jet delay in periods of the note
;;   feedback  how much the mouth's velocity steers the jet
;;   noise     turbulence, relative to the breath
;;   gain      how hard the jet drives the tube
;;   breath    a signal 0..1
;;   weight    0..1
(def jet (id tube at &key (blow 0.5) (ratio 0.5) (feedback 1) (noise 0.05) (gain 0.02)
                          (breath (envelope :attack-ms 60 :sustain 1 :release-ms 60)) (weight 1))
  (dict :id id :title "JET" :law :jet :a nil :b tube :a-at nil :b-at at :signal breath :weight weight
        :params (append (list (pspec id "blow" blow 0 2 :title "Blow")
                              (pspec id "ratio" ratio 0.05 2 :title "Jet" :dec 3)
                              (pspec id "feedback" feedback 0 100 :title "Steer" :taper :log)
                              (pspec id "noise" noise 0 1 :title "Noise")
                              (pspec id "gain" gain 0.000001 2 :title "Drive" :taper :log :dec 6))
                        (signal-params id breath))))

;; ── drivers and signals ─────────────────────────────────────────────────────
;; Drivers move an access point from outside the patch (Modalys' force,
;; position and speed connections). What drives them is a signal: plain data
;; naming a little DGenLisp circuit, or a list of signals multiplied
;; together:
;;   (m/force 'buzz 'str 0.2 :signal (list (m/envelope :attack-ms 50) (m/sine :ratio 2)))
;; Each signal kind is a row of signal-kinds: its params, and the forms and
;; value it emits.

;; DGenLisp's adsr on the gate, restarted by each note and scaled by its
;; velocity.
;;   attack-ms, decay-ms, release-ms  stage times, ms
;;   sustain                          the level held while the gate is up
(def envelope (&key (attack-ms 5) (decay-ms 300) (sustain 0) (release-ms 100))
  (dict :kind :signal :name :env :args (list attack-ms decay-ms sustain release-ms)))

;; White noise, -1..1.
(def noise () (dict :kind :signal :name :noise :args (list)))

;; A sine at ratio times the note.
;;   ratio  frequency ratio to the note
(def sine (&key (ratio 1)) (dict :kind :signal :name :sine :args (list ratio)))

;; A constant (a knob).
;;   value  its starting value, -1..1
(def constant (value) (dict :kind :signal :name :const :args (list value)))

;; Any DGenLisp expression as a signal; it may use gate, velocity, f0, hit
;; (1 on a note's first sample), age (seconds since) and hit_velocity.
;;   form  the expression, quoted
(def signal (form) (dict :kind :signal :name :form :args (list form)))

;; Feedback taps (Modalys' access-position / -speed / -force and
;; connection-state controllers): what the patch did last sample, as a
;; signal for a driver, so one object can drive another (sympathetic loops)
;; or itself (self-oscillation; a force's :saturate keeps that bounded).

;; The displacement of obj's point at at, last sample.
;;   obj  an object id
;;   at   a position on it
(def tap-position (obj at) (dict :kind :signal :name :tap-position :short "tap" :args (list obj at)))

;; The velocity of obj's point at at, last sample, scaled as a listen hears it.
;;   obj  an object id
;;   at   a position on it
(def tap-speed (obj at) (dict :kind :signal :name :tap-speed :short "tap" :args (list obj at)))

;; A connection's force, last sample.
;;   conn  a connection id
(def tap-force (conn) (dict :kind :signal :name :tap-force :short "tap" :args (list conn)))

;; 1 while a connection pushes (in contact), last sample, else 0.
;;   conn  a connection id
(def tap-contact (conn) (dict :kind :signal :name :tap-contact :short "tap" :args (list conn)))

;; What the patch's amp (its :amp) sounds like distance metres from its
;; speaker and angle degrees off its axis: the speaker's output last sample,
;; delayed by the air's travel (distance / 343 m/s), falling as 1 / distance,
;; and losing its top off axis: a 12-inch cone beams, so off axis it is a
;; low-pass at about 580 Hz / sin angle (a piston of radius 0.15 m). A force
;; on the guitar driven by it closes the acoustic feedback loop; the force's
;; gain is how much of the sound the guitar catches. The distance moves which
;; partial the loop favours (and Doppler-bends it while it moves); the angle
;; how far up the harmonics it can squeal. Room adds the sound's reflections
;; off the floor (the cabinet 0.4 m up, the guitar 1.2 m), the wall 1.5 m
;; behind the player and one 2 m to the side, each arriving later along its
;; own path: their delays comb the loop, so more spots feed back, and moving
;; makes the feedback hunt between partials.
;;   distance  metres from the speaker
;;   angle     degrees off its axis, 0 .. 90
;;   room      the reflections' strength, 0 (outdoors) .. 1
(def speaker (&key (distance 1.5) (angle 45) (room 0.5))
  (dict :kind :signal :name :speaker :args (list distance angle room)))

;; The paths m/speaker's sound takes besides the direct one, given the
;; distance d (a form, metres): off the floor (the cabinet 0.4 m up, the
;; guitar 1.2 m), the wall 1.5 m behind the player, the wall 2 m to the
;; side. Each is (dict :length form :extra-max samples): its length, and a
;; delay long enough for how much longer than d it is, at 192 kHz.
;;   d  a form for the distance
(def room-paths (d)
  ;; (0.4 + 1.2)^2 = 2.56; twice 1.5 m = 3; twice 2 m, squared = 16
  (list (dict :length `(sqrt (+ (* ,d ,d) 2.56)) :extra-max 1024)
        (dict :length `(+ ,d 3) :extra-max 2048)
        (dict :length `(sqrt (+ (* ,d ,d) 16)) :extra-max 4096)))

;; How much of a reflection comes back from the floor or a wall.
(def reflection-gain () 0.6)

;; The history holding a patch amp's speaker output, which amp-forms writes
;; and m/speaker reads a sample later.
(def speaker-history () 'amp_speaker_h)

;; True for the taps that read an object's point.
;;   sig  a signal
(def point-tap? (sig) (member? (list :tap-position :tap-speed) (get sig :name)))

;; True for the taps that read a connection's force.
;;   sig  a signal
(def force-tap? (sig) (member? (list :tap-force :tap-contact) (get sig :name)))

;; Forms defining a point tap's mode shape at its point, cid_tap_phi.
;;   n      the tap's symbol function
;;   args   (obj at)
;;   patch  the patch
(def tap-shape-forms (n args patch)
  (let ((o (find-object patch (first args))))
    (shape-defs o (access-syms (n "") "tap" o) (n "_phi"))))

;; One row per signal kind:
;;   :params (lambda (cid n args) specs)  its params
;;   :forms  (lambda (n args) forms)      what it defines first
;;   :value  (lambda (n args) form)       its value
;; where n maps a suffix to the signal's symbol (cid_env_suffix).
(def signal-kinds ()
  (dict
    :env (dict :params (lambda (cid n args)
                         (list (pspec cid (str n "_attack") (nth args 0) 0.1 2000 :unit 'ms :title "Attack" :taper :log)
                               (pspec cid (str n "_decay") (nth args 1) 1 10000 :unit 'ms :title "Decay" :taper :log)
                               (pspec cid (str n "_sustain") (nth args 2) 0 1 :title "Sustain")
                               (pspec cid (str n "_release") (nth args 3) 1 10000 :unit 'ms :title "Release" :taper :log)))
               :forms (lambda (n args patch) nil)
               :value (lambda (n args patch)
                        `(* (adsr gate hit ,(n "_attack") ,(n "_decay") ,(n "_sustain") ,(n "_release")) hit_velocity)))
    :noise (dict :params (lambda (cid n args) nil) :forms (lambda (n args patch) nil)
                 :value (lambda (n args patch) (white-noise-form)))
    :sine (dict :params (lambda (cid n args)
                          (list (pspec cid (str n "_ratio") (nth args 0) 0.125 16 :title "Ratio" :taper :log :dec 3)))
                :forms (lambda (n args patch) nil)
                :value (lambda (n args patch) `(sin (* twopi (phasor (* f0 ,(n "_ratio")))))))
    :const (dict :params (lambda (cid n args) (list (pspec cid (str n "_value") (nth args 0) -1 1 :title "Value")))
                 :forms (lambda (n args patch) nil)
                 :value (lambda (n args patch) (n "_value")))
    :form (dict :params (lambda (cid n args) nil) :forms (lambda (n args patch) nil)
                :value (lambda (n args patch) (first args)))
    ;; taps read last sample's state; their point params come from
    ;; tap-params (they need the patch)
    :tap-position (dict :params (lambda (cid n args) nil)
                        :forms (lambda (n args patch) (tap-shape-forms n args patch))
                        :value (lambda (n args patch)
                                 (let ((o (find-object patch (first args))))
                                   `(sum (* (read-history ,(p o "s1")) ,(n "_phi"))))))
    :tap-speed (dict :params (lambda (cid n args) nil)
                     :forms (lambda (n args patch) (tap-shape-forms n args patch))
                     :value (lambda (n args patch)
                              (let ((o (find-object patch (first args))))
                                `(/ (* (sum (* (- (read-history ,(p o "s1")) (read-history ,(p o "s2"))) ,(n "_phi")))
                                       ,(p o "mass"))
                                    ,(p o "wref")))))
    :tap-force (dict :params (lambda (cid n args) nil) :forms (lambda (n args patch) nil)
                     :value (lambda (n args patch) `(read-history ,(nm (first args) "F_h"))))
    :tap-contact (dict :params (lambda (cid n args) nil) :forms (lambda (n args patch) nil)
                       :value (lambda (n args patch) `(gt (abs (read-history ,(nm (first args) "F_h"))) 0.0000001)))
    ;; the air's travel, at least the sample the history already delays
    :speaker (dict :params (lambda (cid n args)
                             (list (pspec cid (str n "_distance") (nth args 0) 0.05 10 :unit 'm :title "Distance" :taper :log)
                                   (pspec cid (str n "_angle") (nth args 1) 0 90 :unit 'deg :title "Angle" :dec 0)
                                   (pspec cid (str n "_room") (nth args 2) 0 1 :title "Room")))
                   ;; the direct sound, delayed by the air (8192 samples hold
                   ;; 10 m at 192 kHz); each reflection delays it further by
                   ;; its extra path, so one long buffer serves them all
                   :forms (lambda (n args patch)
                            (list `(def ,(n "_direct")
                                     (delay (read-history ,(speaker-history)) (max 0 (- (/ (* ,(n "_distance") samplerate) 343) 1))
                                            @max-delay 8192))))
                   ;; each path falling as 1 / its length, then off axis
                   :value (lambda (n args patch)
                            `(svf (+ (* (/ 1 (max ,(n "_distance") 0.05)) ,(n "_direct"))
                                     ,@(map (lambda (path)
                                              `(* (/ (* ,(reflection-gain) ,(n "_room")) ,(get path :length))
                                                  (delay ,(n "_direct") (/ (* (- ,(get path :length) ,(n "_distance")) samplerate) 343)
                                                         @max-delay ,(get path :extra-max))))
                                            (room-paths (n "_distance"))))
                                  ,(below-nyquist `(/ 580 (max (sin (* ,(n "_angle") 0.017453292519943295)) 0.01)))
                                  0.6 0)))))

;; A driver's signals as a list (one signal, or a list of them).
;;   sig  a signal or a list of signals
(def signals-of (sig) (if (map? sig) (list sig) sig))

;; The name a driver's k-th signal's params and forms share: env, env1, ...
;; (a tap's is tap: its kind's name has a hyphen)
;;   sig  the signal
;;   k    its position among the driver's signals
(def signal-name (sig k)
  (str (if (get sig :short) (get sig :short) (bare (get sig :name))) (if (= k 0) "" k)))

;; Calls one column of a driver's signals' rows, with each signal's name
;; function and args.
;;   c       a driver connection
;;   column  :params, :forms or :value
;;   call    (lambda (f n args) ...) applying that column's function
(def each-signal (c column call)
  (let ((cid (get c :id)) (sigs (signals-of (get c :signal))))
    (map (lambda (k)
           (let ((sig (nth sigs k)))
             (call (get (get (signal-kinds) (get sig :name)) column)
                   (lambda (suffix) (nm cid (str (signal-name sig k) suffix)))
                   (get sig :args)
                   (signal-name sig k))))
         (range (len sigs)))))

;; What is wrong with a driver's signals: anything that is not a signal of a
;; known kind (m/tap-* included).
;;   c  a driver connection
(def signal-errors (c)
  (concat-all
    (map (lambda (sig)
           (if (and (map? sig) (= (get sig :kind) :signal) (map? (get (signal-kinds) (get sig :name))))
             nil
             (list (str (what c) ": " (source sig) " is not a signal (m/envelope, m/noise, m/sine, m/constant, m/signal, m/tap-...)"))))
         (signals-of (get c :signal)))))

;; The params of a driver's signals.
;;   id   the driver's id
;;   sig  a signal or a list of them
(def signal-params (id sig)
  (concat-all (each-signal (dict :id id :signal sig) :params
                           (lambda (f n args name) (f id name args)))))

;; Forms defining a driver's signal, cid_sig: each signal's circuit, then
;; their product.
;;   c      a driver connection
;;   patch  the patch (taps read its objects)
(def signal-forms (c patch)
  (append (concat-all (each-signal c :forms (lambda (f n args name) (f n args patch))))
          (list `(def ,(nm (get c :id) "sig")
                   ,(product-form (each-signal c :value (lambda (f n args name) (f n args patch))))))))

;; The point params of a driver's point taps: each tap's point, named
;; cid_tap_obj_at (apart from the driver's own point).
;;   patch  the patch
;;   c      a driver connection
(def tap-params (patch c)
  (let ((sigs (signals-of (get c :signal))))
    (concat-all
      (map (lambda (k)
             (let ((sig (nth sigs k)) (o (find-object patch (first (get (nth sigs k) :args)))))
               (if (and (point-tap? sig) o)
                 (access-params (nm (get c :id) (signal-name sig k)) "tap" o (nth (get sig :args) 1)
                                :label (str (signal-name sig k) " " (source (get o :id))))
                 nil)))
           (range (len sigs))))))

;; What is wrong with a driver's taps: a point tap on a missing or lumped
;; object (or a bad position), a force tap on a missing connection.
;;   patch  the patch
;;   c      a driver connection
(def tap-errors (patch c)
  (concat-all
    (map (lambda (sig)
           (let ((target (first (get sig :args))))
             (if (point-tap? sig)
               (let ((o (find-object patch target)))
                 (if (= o nil) (list (str (what c) ": tap on no object " (source target)))
                   (if (stateless? o)
                     (list (str (what c) ": a tap needs a modal object, not " (source target)))
                     (at-errors (str (what c) " tap") o (nth (get sig :args) 1)))))
               (if (force-tap? sig)
                 (if (empty? (filter (lambda (other) (= (get other :id) target)) (get patch :connections)))
                   (list (str (what c) ": tap on no connection " (source target)))
                   nil)
                 (if (and (= (get sig :name) :speaker) (not (get patch :amp)))
                   (list (str (what c) ": m/speaker needs the patch's :amp"))
                   nil)))))
         (filter map? (signals-of (get c :signal))))))

;; True for an object with no modal state to read at a point: lumped, or a
;; mix (which only routes).
;;   o  an object
(def stateless? (o) (or (get o :lumped) (not (physical? o))))

;; The connections some driver's force tap reads, by id.
;;   patch  the patch
(def tapped-connections (patch)
  (let ((ids (concat-all
               (map (lambda (c) (map (lambda (sig) (first (get sig :args))) (filter force-tap? (signals-of (get c :signal)))))
                    (filter (lambda (c) (get c :signal)) (get patch :connections))))))
    (map (lambda (k) (nth ids k)) (filter (lambda (k) (first-occurrence? ids k)) (range (len ids))))))

;; A form multiplying forms: 1 for none, the form for one, else (* ...).
;;   forms  a list of forms
(def product-form (forms)
  (if (empty? forms) 1 (if (= (len forms) 1) (first forms) (cons '* forms))))

;; A force on b's access point: gain times the signal (a force of 1 moves the
;; object's fundamental by about one unit, whatever its pitch).
;;   id      the connection's id ('buzz)
;;   b       the driven object's id
;;   at      where on b
;;   signal  a signal, or a list of them multiplied (default an envelope)
;;   gain    the force's scale
;;   saturate  true for gain tanh(signal): a bounded push, for feedback
;;   weight  0..1, as for every connection
(def force (id b at &key (signal (envelope)) (gain 0.05) (saturate false) (weight 1))
  (dict :id id :title "FORCE" :law :force :a nil :b b :a-at nil :b-at at :signal signal :weight weight
        :saturate saturate
        :params (append (list (pspec id "gain" gain 0.000001 2 :title "Gain" :taper :log :dec 6))
                        (signal-params id signal))))

;; Holds b's access point at the signal's value (a displacement) through a
;; stiff implicit spring: a constant 0 clamps the point (Modalys' position).
;;   id         the connection's id ('clamp)
;;   b, at      the driven object and where
;;   signal     a signal (default (m/constant 0): a clamp)
;;   stiffness  how hard it holds, relative to the point's give
;;   weight     0..1
(def position (id b at &key (signal (constant 0)) (stiffness 50) (weight 1))
  (dict :id id :title "POSITION" :law :position :a nil :b b :a-at nil :b-at at :signal signal :weight weight
        :params (append (list (pspec id "stiff" stiffness 0 1000 :title "Hold" :taper :log :dec 1))
                        (signal-params id signal))))

;; Moves b's access point at gain times the signal (units per second): the
;; point follows a target the signal's integral drags along, which leaks back
;; to rest with a 2 s time constant so it never drifts away (Modalys' speed).
;;   id         the connection's id ('push)
;;   b, at      the driven object and where
;;   signal     a signal (default an envelope)
;;   gain       speed scale
;;   stiffness  how closely the point follows, relative to its give
;;   weight     0..1
(def speed (id b at &key (signal (envelope)) (gain 0.01) (stiffness 50) (weight 1))
  (dict :id id :title "SPEED" :law :speed :a nil :b b :a-at nil :b-at at :signal signal :weight weight
        :params (append (list (pspec id "gain" gain 0 1 :title "Gain" :dec 3)
                              (pspec id "stiff" stiffness 0 1000 :title "Hold" :taper :log :dec 1))
                        (signal-params id signal))))

;; A listening point (a mic): the output hears the velocity of obj at at
;; (Modalys' speed output), or its position.
;;   obj   the object's id
;;   at    where on it to listen, a position as for a connection
;;   gain  this mic's level in the mix
;;   modes  which modes it hears, as mode numbers from 0 (Modalys'
;;          point-output selector); nil hears all
;;   hear   :speed (normalised so objects of any pitch and mass mix at
;;          similar levels), or :position, the point's displacement as it
;;          is: what measured strokes were heard as
(def listen (obj at &key (gain 1) (modes nil) (hear :speed))
  (dict :obj obj :at at :gain gain :modes modes :hear hear))

;; A magnetic pickup: a listen that hears obj the way a guitar's coil does.
;; The coil's voltage is the string's speed times how fast the magnet's flux
;; changes with its distance, which grows as the string nears the pole
;; (about 1 / gap^3): a pickup set close is louder, and asymmetric, adding
;; even harmonics. The coil and the cable's capacitance make a resonant
;; low-pass. Where it sits along the string is its comb filter, as for any
;; listen (0.1 by the bridge, 0.25 at the neck).
;;   obj      the string's id
;;   at       where the pole sits, a position as for a listen
;;   gain     its level in the mix
;;   height   the pole's distance from the string at rest, in units of a
;;            firm pluck's swing: lower is closer
;;   coil-hz  the coil's resonance (a bright single coil ~5 kHz, a
;;            humbucker ~2.5 kHz)
;;   coil-q   how pronounced that resonance is (0.5 none)
(def pickup (obj at &key (gain 1) (height 3) (coil-hz 3500) (coil-q 2))
  (dict :obj obj :at at :gain gain :modes nil :hear :pickup :height height :coil-hz coil-hz :coil-q coil-q))

;; True for a listen made by pickup.
;;   l  a listen
(def pickup? (l) (= (get l :hear) :pickup))

;; obj with Modalys' pseudo-physical controls: its spectrum warped without
;; recomputing it. Every mode's frequency is multiplied by freq-scales (one
;; per mode, the rest 1), bent by the bend param (cents), made to follow the
;; note (follow-note: the lowest mode at the played note, for objects with a
;; pitch of their own, such as plates), then spread about the lowest mode by
;; the diffraction param rho: f' = (f - f1) ln(rho (e - 1) + 1) + f1, so 1
;; leaves it, 0 folds every mode onto f1, above 1 stretches it. Losses are
;; multiplied by loss-scales and what its listens hear by gains (one per mode:
;; a mode equalizer).
;;   obj          an object
;;   bend         starting pitch bend, cents (a param)
;;   diffraction  starting rho (a param)
;;   follow-note  true to put the lowest mode on the played note
;;   freq-scales  per-mode frequency factors
;;   loss-scales  per-mode loss factors
;;   gains        per-mode gains for what listens hear
(def spectral (obj &key (bend 0) (diffraction 1) (follow-note false) (freq-scales nil) (loss-scales nil) (gains nil))
  (let ((id (get obj :id)))
    (merge obj
           :spectral (dict :follow follow-note :freq-scales freq-scales :loss-scales loss-scales :gains gains)
           :params (append (get obj :params)
                           (list (pspec id "bend" bend -2400 2400 :unit 'ct :title "Bend" :dec 0)
                                 (pspec id "diffraction" diffraction 0 3 :title "Spread"))))))

;; The id of an object's k-th direction: the object itself for 0, its second
;; direction (id_v) for 1 (see string's :planes).
;;   id  the object's id
;;   k   0 or 1
(def plane (id k) (if (= k 0) id (nm id "v")))

;; The symbol of one of obj's params: obj_name, or its first direction's
;; for a second-direction copy, which has no params of its own.
;;   obj   an object
;;   name  the param's name
(def pp (obj name) (nm (if (get obj :plane-of) (get obj :plane-of) (get obj :id)) name))

;; An object and, for one with :planes 2, its second direction: a copy named
;; id_v that reads the first's params.
;;   o  an object
(def expand-planes (o)
  (if (= (get o :planes) 2)
    (list o (merge o :id (plane (get o :id) 1) :plane-of (get o :id) :planes 1 :params (list)
                   :shared-params (get o :params) :title (str (get o :title) " v")))
    (list o)))

;; The hidden contact a bow with :hair presses its string's second direction
;; down with: a stop from above at the hair depth, faded with the bow, its
;; point the bow's.
;;   c  a bow connection with :hair
(def hair-connection (c)
  (dict :id (nm (get c :id) "hair") :title "HAIR" :law :stop :above true :a nil :b (plane (get c :b) 1)
        :a-at nil :b-at (get c :b-at) :internal true :owner (get c :id) :coords-of (get c :id)
        :weight-form `(* ,(nm (get c :id) "weight") ,(nm (get c :id) "env")) :params (list)))

;; A patch's connections: lists (fingerboard's pair) flattened, and each
;; bow with :hair followed by its hair contact.
;;   connections  as given
(def expand-connections (connections)
  (concat-all (map (lambda (c) (if (and (bow? c) (get c :hair)) (list c (hair-connection c)) (list c)))
                   (concat-all (map (lambda (c) (if (list? c) c (list c))) connections)))))

;; A whole patch.
;;   name         the instrument's name; what Save offers, and the folder
;;                eseq modal emit writes by default
;;   objects      a list of objects
;;   connections  a list of connections, solved in this order
;;   listens      a list of listens, mixed into the output
;;   level        an overall output trim
;;   voicing      nil, or :one (every hit on one ringing instrument, as a
;;                cymbal is played: the host's mono voicing) or :poly; either
;;                adds the host's Voicing param (0 poly, 1 one voice, ...)
;;   tone         nil, or a starting cutoff (Hz) for an output Tone param: a
;;                gentle low-pass, 1 kHz .. 20 kHz
;;   amp          nil, or an amplifier (m/amp) the mix plays through before
;;                the output: inside the instrument, so m/speaker can feed
;;                its sound back into the objects
(def patch (&key (name "modal") (objects nil) (connections nil) (listens nil) (level 1) (voicing nil) (tone nil)
                (amp nil))
  (let ((conns (expand-connections connections)))
    (let ((planed (concat-all (map expand-planes objects)))
          (compiled (map (lambda (o) (compile-points o conns listens planed)) planed)))
      (dict :name name
            ;; a mix hybrid carries its part objects, for its ends and listens
            :objects (map (lambda (o) (if (= (get o :kind) :mix)
                                        (merge o :part-objs
                                               (map (lambda (id) (first (filter (lambda (q) (= (get q :id) id)) compiled)))
                                                    (get o :parts)))
                                        o))
                          compiled)
            :connections conns :listens listens :level level :voicing voicing :tone tone :amp amp))))

;; ── data objects' points ────────────────────────────────────────────────────
;; A data object's accesses address its points by index or by (x y z) (the
;; nearest point). The DSP holds the shape rows of its compiled points: all of
;; them up to 64, else only those its connections and listens use (Modalys'
;; compiled accesses), so a 2000-point solid stays a small table.

;; How many points a data object has.
;;   obj  a data object
(def point-count (obj) (len (get obj :shapes)))

;; The point an access position names on a data object: a number is an
;; index, (x y z) the nearest of its :points; 0 when neither fits.
;;   obj  a data object
;;   at   the position
(def point-index (obj at)
  (if (number? at) (clamp (round at) 0 (- (point-count obj) 1))
    (let ((pts (get obj :points)))
      (if (and (list? at) (= (len at) 3) (list? pts) (not (empty? pts)))
        ;; native: a solid has thousands of points
        (modal-nearest-point pts at)
        0))))

;; obj with :compiled, the points its DSP holds (in order), when obj is a
;; data object; any other object unchanged.
;;   obj          an object
;;   connections  the patch's connections
;;   listens      the patch's listens
(def compile-points (obj connections listens objects)
  (if (= (get obj :kind) :melt)
    (merge obj :parts (map (lambda (part) (compile-points part connections listens objects)) (get obj :parts)))
  (if (not (= (get obj :kind) :data)) obj
    (if (<= (point-count obj) 64) (merge obj :compiled (range (point-count obj)))
      (let ((used (map (lambda (at) (point-index obj at)) (positions-on (get obj :id) connections listens objects)))
            (unique (filter (lambda (k) (first-occurrence? used k)) (range (len used)))))
        (merge obj :compiled (if (empty? unique) (list 0) (map (lambda (k) (nth used k)) unique))))))))

;; Every position the patch puts on object id: its connections' ends and
;; listens, the matching half of positions on a hybrid it is part of, and
;; point taps on it.
;;   id           an object id
;;   connections  the patch's connections
;;   listens      the patch's listens
;;   objects      the patch's objects (for hybrids)
(def positions-on (id connections listens objects)
  (let ((on (lambda (target)
              (append (map (lambda (c) (if (= (get c :a) target) (get c :a-at) (get c :b-at)))
                           ;; strokes play every mode, at no point; a mount's
                           ;; string end is not one either
                           (filter (lambda (c) (and (not (strokes? c))
                                                    (or (= (get c :a) target) (and (= (get c :b) target) (not (mount? c))))))
                                   connections))
                      (map (lambda (l) (get l :at)) (filter (lambda (l) (= (get l :obj) target)) listens))))))
    (append (on id)
            (concat-all
              (map (lambda (h)
                     (let ((k (index-of (map (lambda (part) (if (map? part) (get part :id) part)) (get h :parts)) id)))
                       (if (= k nil) nil
                         (map (lambda (at) (if (list? at) (nth at k) at)) (on (get h :id))))))
                   (filter hybrid? objects)))
            (concat-all
              (map (lambda (c) (map (lambda (sig) (nth (get sig :args) 1))
                                    (filter (lambda (sig) (and (map? sig) (point-tap? sig) (= (first (get sig :args)) id)))
                                            (signals-of (get c :signal)))))
                   (filter (lambda (c) (get c :signal)) connections))))))

;; Where in a data object's compiled points the point at sits (0 if absent).
;;   obj  a data object (after compile-points)
;;   at   an access position
(def compiled-slot (obj at)
  (let ((k (point-index obj at)) (cs (get obj :compiled)))
    (reduce (lambda (found j) (if (= (nth cs j) k) j found)) 0 (range (len cs)))))

;; ── check ───────────────────────────────────────────────────────────────────

;; True for objects whose points are one position 0..1 (strings, bars).
;;   obj  an object
(def one-d? (obj) (member? (list :string :bar :tube) (get obj :kind)))

;; True for rectangular objects, whose points are (x y) in 0..1.
;;   obj  an object
(def rect? (obj) (member? (list :plate :rect-membrane) (get obj :kind)))

;; The object in patch whose id is id, or nil.
;;   patch  a patch
;;   id     an object id
(def find-object (patch id)
  (first (filter (lambda (o) (= (get o :id) id)) (get patch :objects))))

;; A keyword's name without its colon: (bare :steel) -> "steel".
;;   k  a keyword
(def bare (k) (let ((s (source k))) (substring s 1 (len s))))

;; A connection's law as a word: "spring".
;;   c  a connection
(def law-name (c) (bare (get c :law)))

;; How messages name a connection: "spring bridge".
;;   c  a connection
(def what (c) (str (law-name c) " " (source (get c :id))))

;; What is wrong with access point at on obj, as a list of messages (empty
;; when it fits: lumped objects take anything, strings one number, plates and
;; membranes a list).
;;   who  the message prefix naming who uses the point ("spring bridge a")
;;   obj  the object
;;   at   the position given for it
(def at-errors (who obj at)
  (if (hybrid? obj)
    (if (and (list? at) (= (len at) 2))
      (concat-all (map (lambda (k) (let ((part (nth (hybrid-part-objects obj) k)))
                                     (if (map? part) (at-errors who part (nth at k)) nil)))
                       (range 2)))
      (list (str who ": hybrid " (source (get obj :id)) " takes (list at-a at-b), got " (source at))))
  (if (get obj :lumped) nil
    (if (= (get obj :kind) :data) (data-at-errors who obj at)
    (if (one-d? obj)
      (if (number? at) nil
        (list (str who ": " (bare (get obj :kind)) " " (source (get obj :id))
                   " takes one position 0..1, got " (source at))))
      (if (number? at)
        (list (str who ": " (bare (get obj :kind)) " " (source (get obj :id)) " takes a position "
                   (if (rect? obj) "(list x y)" "(list r theta)") ", got " (source at)))
        nil))))))

;; What is wrong with access point at on data object obj.
;;   who  the message prefix
;;   obj  a data object
;;   at   a point index, or (x y z) when obj has :points
(def data-at-errors (who obj at)
  (let ((name (str (get obj :label) " " (source (get obj :id)))))
    (if (number? at)
      (if (and (>= at 0) (< at (point-count obj))) nil
        (list (str who ": " name " has points 0.." (- (point-count obj) 1) ", got " (source at))))
      (if (and (list? at) (= (len at) 3))
        (if (list? (get obj :points)) nil
          (list (str who ": " name " has no :points to find " (source at) " among")))
        (list (str who ": " name " takes a point index or (list x y z), got " (source at)))))))

;; What is wrong with a data object's numbers: no modes, or a losses or
;; shape row that does not have one value per mode.
;;   obj  a data object
(def data-errors (obj)
  (let ((name (str (get obj :label) " " (source (get obj :id))))
        (n (get obj :modes))
        (points (get obj :points)))
    (if (get obj :data-error) (list (str name ": " (get obj :data-error)))
    (if (not (member? (list nil :octave) (get obj :follow)))
      (list (str name ": :follow is :octave, got " (source (get obj :follow))))
    (if (and (get obj :follow) (get obj :root))
      (list (str name ": give :root (follow the note) or :follow :octave, not both"))
    (if (= n 0)
      (list (str name ": needs :freqs, a list of mode frequencies"))
      (append (if (or (= points nil) (and (list? points) (= (len points) (point-count obj)))) nil
                (list (str name ": " (len points) " points for " (point-count obj) " shape rows")))
              (if (= (len (get obj :losses)) n) nil
                (list (str name ": " (len (get obj :losses)) " losses for " n " modes")))
              (concat-all (map (lambda (k) (if (= (len (nth (get obj :shapes) k)) n) nil
                                             (list (str name ": shape row " k " has "
                                                        (len (nth (get obj :shapes) k)) " values for " n " modes"))))
                               (range (point-count obj)))))))))))

;; What is wrong with a hybrid: a melt's parts with different mode counts, a
;; mix's parts missing or lumped.
;;   patch  the patch
;;   obj    a hybrid
(def hybrid-errors (patch obj)
  (let ((name (str (bare (get obj :kind)) " " (source (get obj :id)))))
    (if (= (get obj :kind) :melt)
      (let ((na (get (first (get obj :parts)) :modes)) (nb (get (nth (get obj :parts) 1) :modes)))
        (append (concat-all (map (lambda (part) (if (or (get part :lumped) (hybrid? part))
                                                  (list (str name ": " (source (get part :id)) " is not a modal object"))
                                                  nil))
                                 (get obj :parts)))
                (if (= na nb) nil (list (str name ": its parts have " na " and " nb " modes; give them the same :modes")))))
      (concat-all
        (map (lambda (id)
               (let ((part (find-object patch id)))
                 (if (= part nil) (list (str name ": no object " (source id)))
                   (if (or (get part :lumped) (hybrid? part))
                     (list (str name ": " (source id) " is not a modal object a mix can share"))
                     nil))))
             (get obj :parts))))))

;; What is wrong with an object's spectral controls: a lumped object (a
;; finger, a mallet) has no spectrum to warp.
;;   obj  an object
(def spectral-errors (obj)
  (if (and (get obj :spectral) (get obj :lumped))
    (list (str (bare (get obj :kind)) " " (source (get obj :id)) ": spectral needs a modal object"))
    nil))

;; What is wrong with one end of a connection: a missing object, or a
;; position that does not fit it.
;;   patch  the patch
;;   c      the connection
;;   side   "a" or "b", for the message
;;   id     that end's object id
;;   at     that end's position
(def end-errors (patch c side id at)
  (let ((obj (find-object patch id)))
    (if (= obj nil)
      (list (str (what c) ": no object " (source id)))
      (at-errors (str (what c) " " side) obj at))))

;; What is wrong with a connection: both ends on one object, or either end
;; (strokes: see strokes-errors).
;;   patch  the patch
;;   c      the connection
(def connection-errors (patch c)
  (append
  (if (and (get c :a) (= (get c :a) (get c :b))) (list (str (what c) ": both ends on " (source (get c :b)))) nil)
  (if (strokes? c) (strokes-errors patch c)
  (if (mount? c) (mount-errors patch c)
  (append
          ;; these read their struck object's own wref / state, which a mix has none of
          (if (and (member? (list :bow :felt) (get c :law)) (find-object patch (get c :b))
                   (= (get (find-object patch (get c :b)) :kind) :mix))
            (list (str (what c) ": cannot end on a mix; connect the parts, or use a melt"))
            nil)
          (air-errors patch c)
          (if (get c :a) (end-errors patch c "a" (get c :a) (get c :a-at)) nil)
          (end-errors patch c "b" (get c :b) (get c :b-at)))))))

;; What is wrong with a mount: its string must be a string and its end 0 or
;; 1; the mount's object one whose state is a displacement to read (not
;; lumped, a mix or a tube, whose state is the air's) at a fitting point.
;;   patch  the patch
;;   c      a mount
(def mount-errors (patch c)
  (let ((strung (find-object patch (get c :b))) (body (find-object patch (get c :a))))
    (append (if (= strung nil) (list (str (what c) ": no object " (source (get c :b))))
              (if (= (get strung :kind) :string) nil
                (list (str (what c) ": " (source (get c :b)) " is not a string"))))
            (if (member? (list 0 1) (get c :end)) nil
              (list (str (what c) ": end is 0 or 1, got " (source (get c :end)))))
            (if (and body (or (stateless? body) (= (get body :kind) :tube)))
              (list (str (what c) ": the mount needs a modal object, not " (source (get c :a))))
              (end-errors patch c "a" (get c :a) (get c :a-at))))))

;; What is wrong with a blown connection: it must blow into a tube, at a
;; point where the tube has pressure (not an open end), and lips need a
;; modal-data lip (an oscillator).
;;   patch  the patch
;;   c      a connection
(def air-errors (patch c)
  (if (not (member? (list :reed :lips :jet) (get c :law))) nil
    (let ((tube (find-object patch (get c :b))) (lip (if (get c :a) (find-object patch (get c :a)) nil)))
      (append
        (if (and tube (not (= (get tube :kind) :tube)))
          (list (str (what c) ": blows into a tube (m/tube), not " (source (get c :b))))
          (if (and tube (not (= (get c :law) :jet)) (= (get tube :ends) :open-open)
                   (number? (get c :b-at)) (or (< (get c :b-at) 0.01) (> (get c :b-at) 0.99)))
            (list (str (what c) ": " (source (get c :b)) " is open there (no pressure); use a closed end"))
            nil))
        (if (and (= (get c :law) :lips) lip (not (= (get lip :kind) :data)))
          (list (str (what c) ": the lip is an m/oscillator (or modal-data), not " (source (get c :a))))
          nil)))))

;; What is wrong with a listen: a missing object, or a bad position.
;;   patch  the patch
;;   l      the listen
(def listen-errors (patch l)
  (let ((obj (find-object patch (get l :obj))))
    (if (= obj nil)
      (list (str "listen: no object " (source (get l :obj))))
      (append (at-errors "listen" obj (get l :at))
              (if (or (member? (list :speed :position) (get l :hear)) (and (pickup? l) (get l :height)))
                (if (and (member? (list :position :pickup) (get l :hear)) (= (get obj :kind) :mix))
                  ;; its parts' displacements are in different units
                  (list (str "listen " (source (get l :obj)) ": :hear " (source (get l :hear)) " is not for a mix"))
                  nil)
                (list (str "listen " (source (get l :obj)) ": :hear is :speed or :position (m/pickup makes a pickup), got "
                           (source (get l :hear)))))
              (if (= (get l :modes) nil) nil
              (if (= (get obj :kind) :mix) (list (str "listen " (source (get l :obj)) ": :modes is not for a mix"))
                (map (lambda (k) (str "listen " (source (get l :obj)) ": no mode " k " (it has "
                                      (get obj :modes) ", from 0)"))
                     (filter (lambda (k) (or (< k 0) (>= k (get obj :modes)))) (get l :modes)))))))))

;; One message per id used twice among objects and connections (their
;; generated names would collide).
;;   patch  the patch
(def duplicate-ids (patch)
  (let ((ids (append (map (lambda (o) (get o :id)) (all-objects patch))
                     (map (lambda (c) (get c :id)) (get patch :connections)))))
    (map (lambda (k) (str "two things are called " (source (nth ids k))))
         (filter (lambda (k) (not (empty? (filter (lambda (j) (= (nth ids j) (nth ids k))) (range k)))))
                 (range (len ids))))))

;; True when made-of is a material or a non-empty list of materials.
;;   made-of  what an object's :made-of was given
(def made-of? (made-of)
  (let ((choices (material-choices made-of)))
    (if (list? choices)
      (if (empty? choices) false (empty? (filter (lambda (m) (not (mat/material? m))) choices)))
      false)))

;; True when obj is made of a material (strings and plates).
;;   obj  an object
(def has-material? (obj) (member? (list :string :plate :circ-plate) (get obj :kind)))

;; What is wrong with one material, as messages naming who uses it.
;;   who  the message prefix ("string str")
;;   m    a material
(def material-field-errors (who m)
  (let ((label (if (string? (get m :name)) (get m :name) "a material")))
    (append (if (string? (get m :name)) nil (list (str who ": a material needs a :name string")))
            (concat-all
              (map (lambda (field)
                     (if (number? (get m field)) nil
                       (list (str who ": material " label " needs a number " (source field)))))
                   (list :density :young :poisson))))))

;; What is wrong with what obj is made of: not a material or a list of them
;; (an old keyword material such as :steel lands here), a material missing a
;; field, or a menu with two materials of one name.
;;   obj  a string or plate
(def made-of-errors (obj)
  (let ((who (str (bare (get obj :kind)) " " (source (get obj :id))))
        (made-of (get obj :stuff)))
    (if (not (made-of? made-of))
      (list (str who ": :made-of takes a material such as mat/steel, or a list of them; got "
                 (source made-of)))
      (let ((choices (material-choices made-of))
            (names (map (lambda (m) (get m :name)) (material-choices made-of))))
        (append (concat-all (map (lambda (m) (material-field-errors who m)) choices))
                (map (lambda (k) (str who ": two materials are called " (nth names k)))
                     (filter (lambda (k) (not (empty? (filter (lambda (j) (= (nth names j) (nth names k))) (range k)))))
                             (range (len names)))))))))

;; Every material problem in a patch.
;;   patch  the patch
(def material-errors (patch)
  (concat-all (map (lambda (o) (if (has-material? o) (made-of-errors o) nil)) (all-objects patch))))

;; The patch's objects and its melts' parts (which are not listed itself),
;; for the checks that look at every object.
;;   patch  the patch
(def all-objects (patch)
  (append (get patch :objects)
          (concat-all (map (lambda (o) (if (= (get o :kind) :melt) (get o :parts) nil)) (get patch :objects)))))

;; Everything wrong with a patch, as a list of messages; empty when it is
;; fine. dsp and ui assume a patch this finds nothing wrong with.
;;   patch  the patch
(def check (patch)
  (concat-all
    (list (if (empty? (get patch :objects)) (list "a patch needs at least one object") nil)
          (if (empty? (get patch :listens)) (list "a patch needs at least one listen point") nil)
          (duplicate-ids patch)
          (material-errors patch)
          (concat-all (map (lambda (o) (if (= (get o :kind) :data) (data-errors o) nil)) (all-objects patch)))
          (concat-all (map spectral-errors (all-objects patch)))
          (concat-all (map (lambda (o) (hybrid-errors patch o)) (filter hybrid? (get patch :objects))))
          (concat-all (map (lambda (c) (if (and (bow? c) (get c :hair) (= (find-object patch (plane (get c :b) 1)) nil))
                                         (list (str (what c) ": :hair needs a string with :planes 2"))
                                         nil))
                           (get patch :connections)))
          (concat-all (map (lambda (c) (if (get c :signal) (append (signal-errors c) (tap-errors patch c)) nil))
                           (get patch :connections)))
          (concat-all (map (lambda (c) (connection-errors patch c)) (get patch :connections)))
          (concat-all (map (lambda (l) (listen-errors patch l)) (get patch :listens)))
          (if (or (= (get patch :amp) nil) (and (map? (get patch :amp)) (= (get (get patch :amp) :kind) :amp))) nil
            (list (str "patch: :amp is an m/amp, got " (source (get patch :amp)))))
          (if (member? (list nil :one :poly) (get patch :voicing)) nil
            (list (str "patch: :voicing is :one or :poly, got " (source (get patch :voicing))))))))

;; ── access-point params ─────────────────────────────────────────────────────

;; The param specs for where a connection or mic touches an object, one per
;; coordinate: none for a lumped object, <obj>_at for a string, <obj>_x and
;; <obj>_y for a plate, <obj>_r and <obj>_angle for a membrane. They belong
;; to the connection (cid_str_at), so they sit in its panel section.
;;   cid    the connection's (or mic's) id, which owns the params
;;   side   "a", "b" or "mic"; not part of the names, since the object id
;;          already tells a connection's two ends apart
;;   obj    the object touched
;;   at     the position, giving the params' defaults
;;   label  what the panel titles call the point (default: the object's id)
(def access-params (cid side obj at &key (label nil))
  (let ((on (source (get obj :id)))
        (lb (if label label (source (get obj :id)))))
    (if (get obj :lumped) nil
      (if (hybrid? obj)
        (concat-all (map (lambda (k) (access-params cid side (nth (hybrid-part-objects obj) k) (nth at k) :label label))
                         (range 2)))
      (if (= (get obj :kind) :data)
        (if (<= (len (get obj :compiled)) 1) nil
          (list (pspec cid (str on "_pt") (compiled-slot obj at) 0 (- (len (get obj :compiled)) 1)
                       :title (str lb " point") :dec 0)))
      (if (one-d? obj)
        (list (pspec cid (str on "_at") at (if (member? (list :bar :tube) (get obj :kind)) 0 0.01)
                     (if (member? (list :bar :tube) (get obj :kind)) 1 0.99)
                     :title (str lb " at")))
        (if (rect? obj)
          (list (pspec cid (str on "_x") (nth at 0) 0.01 0.99 :title (str lb " x"))
                (pspec cid (str on "_y") (nth at 1) 0.01 0.99 :title (str lb " y")))
          (list (pspec cid (str on "_r") (nth at 0) 0 (if (= (get obj :kind) :circ-plate) 1 0.95) :title (str lb " r"))
                (pspec cid (str on "_angle") (nth at 1) 0 3.14159 :title (str lb " angle"))))))))))

;; A hybrid's two part objects: a melt's own, a mix's from the patch.
;;   obj  a hybrid (a mix after the patch gave it :part-objs)
(def hybrid-part-objects (obj) (if (= (get obj :kind) :melt) (get obj :parts) (get obj :part-objs)))

;; The symbols of those params, in coordinate order: what shape-defs reads
;; the point from at run time.
;;   cid, side, obj  as for access-params
(def access-syms (cid side obj)
  (map (lambda (p) (get p :sym)) (access-params cid side obj (list 0 0))))

;; Forms defining phi-sym, obj's mode-shape vector at an access point
;; (masked to the modes that sound): sin(n pi x) on a string, the plate's
;; product of sines, the membrane's Bessel shapes.
;;   obj      a modal (not lumped) object
;;   coords   forms for the point's coordinates, as access-syms gives
;;   phi-sym  the symbol to define
(def shape-defs (obj coords phi-sym)
  (let ((id (get obj :id)) (kind (get obj :kind)))
    (if (= kind :melt) (melt-shape-defs obj coords phi-sym)
    (if (= kind :data) (data-shape-defs obj coords phi-sym)
    (if (= kind :bar) (bar-shape-defs obj (first coords) phi-sym)
    (if (= kind :tube) (list `(def ,phi-sym ,(tube-shape obj (first coords) false)))
    (if (= kind :circ-plate) (circ-plate-shape-defs obj coords phi-sym)
    (if (= kind :string)
      (list `(def ,phi-sym (* (sin (* ,(nm id "n") 3.14159265 ,(first coords))) ,(nm id "mask"))))
      (if (rect? obj)
        (list `(def ,phi-sym (* (rect-plate-shape ,(nm id "pm") ,(nm id "pn") ,(nth coords 0) ,(nth coords 1))
                                ,(nm id "mask"))))
        (list `(def (,(nm phi-sym "ratios") ,(nm phi-sym "raw")) (circ-membrane ,(nth coords 0) ,(nth coords 1)))
              `(def ,phi-sym (* ,(nm phi-sym "raw") ,(nm id "mask")))))))))))))

;; shape-defs for a melt: each part's shape at its own point, blended.
;;   obj      a melt
;;   coords   the parts' coordinates, a's then b's
;;   phi-sym  the symbol to define
(def melt-shape-defs (obj coords phi-sym)
  (let ((a (first (get obj :parts))) (b (nth (get obj :parts) 1)) (x (p obj "morph"))
        (na (len (access-syms 'probe "b" (first (get obj :parts))))))
    (append (shape-defs a (map (lambda (k) (nth coords k)) (range na)) (nm phi-sym "a"))
            (shape-defs b (map (lambda (k) (nth coords (+ na k))) (range (- (len coords) na))) (nm phi-sym "b"))
            (list `(def ,phi-sym (+ (* (- 1 ,x) ,(nm phi-sym "a")) (* ,x ,(nm phi-sym "b"))))))))

;; How many radial samples a circular plate's shape table holds.
(def circ-samples () 64)

;; shape-defs for a circular plate: R(r) from the radial table (linear
;; between samples) times cos(m theta).
;;   obj      a circular plate
;;   coords   (r theta)
;;   phi-sym  the symbol to define
(def circ-plate-shape-defs (obj coords phi-sym)
  (let ((top (- (circ-samples) 1)))
    (append (table-row-defs (p obj "radial") (get obj :modes) `(* (clip ,(first coords) 0 1) ,top) top
                            (nm phi-sym "row") true)
            (list `(def ,phi-sym (* ,(nm phi-sym "row") (cos (* ,(p obj "order") ,(nth coords 1))) ,(p obj "mask")))))))

;; Forms defining sym, a row of a [rows n] table at a fractional row index:
;; blended linearly between the rows either side, or the nearest row when the
;; rows are not neighbours of each other.
;;   table  the table's symbol
;;   n      its row length
;;   index  a form for the row index (0..top)
;;   top    the last row
;;   sym    the symbol to define
;;   blend  true to blend, false for the nearest row
(def table-row-defs (table n index top sym blend)
  (let ((row (lambda (k) (table-row-form table n k)))
        (x (nm sym "x")) (i (nm sym "i")))
    (if blend
      (list `(def ,x (clip ,index 0 ,top))
            `(def ,i (floor ,x))
            `(def ,sym (+ (* ,(row i) (- 1 (- ,x ,i))) (* ,(row `(min (+ ,i 1) ,top)) (- ,x ,i)))))
      (list `(def ,i (clip (round ,index) 0 ,top))
            `(def ,sym ,(row i))))))

;; The free-free beam's modes, computed when generating (in double
;; precision) so the DSP never forms cosh / sinh, whose size would swamp a
;; float. With b = beta L, mode n's shape at x in 0..1 is
;;   cosh bx + cos bx - s (sinh bx + sin bx)
;;   = cos bx - s sin bx + A e^(b (x - 1)) + B e^(-bx)
;; with s = (cosh b - cos b) / (sinh b - sin b),
;; A = (1 - s) e^b / 2 = (cos b - sin b - e^-b) / (1 - e^-2b - 2 e^-b sin b),
;; B = (1 + s) / 2; every term stays within a few units.
;;   modes  how many
(def bar-betas (modes)
  (map (lambda (k) (if (< k 4) (nth (list 4.73004074 7.85320462 10.9956078 14.1371655) k)
                     (* (+ (* 2 (+ k 1)) 1) 1.5707963268)))
       (range modes)))
(def bar-s (b) (let ((e (exp (* -1 b))))
                 ;; (cosh b - cos b) / (sinh b - sin b), times 2 e^-b top and bottom
                 (/ (- (+ 1 (* e e)) (* 2 e (cos b))) (- (- 1 (* e e)) (* 2 e (sin b))))))
(def bar-a (b) (let ((e (exp (* -1 b))))
                 (/ (- (- (cos b) (sin b)) e) (- (- 1 (* e e)) (* 2 e (sin b))))))

;; Each mode's frequency ratio to the fundamental: natural (b_n / b_1)^2,
;; and the undercut targets (modes 2 and 3 at 4 and 10).
;;   modes  how many
(def bar-ratios (modes)
  (let ((bs (bar-betas modes)))
    (map (lambda (b) (pow (/ b (first bs)) 2)) bs)))
(def bar-targets (modes)
  (let ((natural (bar-ratios modes)))
    ;; modes above 3 move with mode 3 (by 10 / 5.404), so they stay in order
    (map (lambda (k) (if (= k 1) 4 (if (= k 0) 1 (* (nth natural k) (/ 10 (nth natural 2))))))
         (range modes))))

;; shape-defs for a bar: the mode shapes above at x, scaled by 1/sqrt 2 to
;; the string's unit modal mass (mean square 1/2).
;;   obj      a bar
;;   x        the form for the position 0..1
;;   phi-sym  the symbol to define
(def bar-shape-defs (obj x phi-sym)
  (let ((bx `(* ,(p obj "beta") ,x)))
    (list `(def ,phi-sym
             (* (+ (cos ,bx) (* -1 ,(p obj "s") (sin ,bx))
                   (* ,(p obj "A") (exp (* ,(p obj "beta") (- ,x 1))))
                   (* ,(p obj "B") (exp (* -1 ,bx))))
                0.70710678 ,(p obj "mask"))))))

;; shape-defs for a data object: its compiled shape row at the point param,
;; blended linearly between neighbouring rows for a fractional index.
;;   obj      a data object
;;   coords   (list point-param), or none when it has one compiled point
;;   phi-sym  the symbol to define
(def data-shape-defs (obj coords phi-sym)
  (let ((top (- (len (get obj :compiled)) 1)) (row (nm phi-sym "row")))
    (append (table-row-defs (p obj "shapes") (get obj :modes) (if (empty? coords) 0 (first coords)) top
                            row
                            ;; a subset of a big object's points: neighbours in the
                            ;; table are not neighbours on the object
                            (= (len (get obj :compiled)) (point-count obj)))
            (list `(def ,phi-sym (* ,row ,(p obj "mask")))))))

;; ── per-object DSP ──────────────────────────────────────────────────────────

;; The symbol obj_name for a name an object introduces: (p obj "freqs").
;;   obj   the object
;;   name  the name's suffix
(def p (obj name) (nm (get obj :id) name))

;; The Young's modulus a plate's macros take for material m. rect-plate
;; computes with Poisson's ratio 0.3 (sqrt(E / (12 rho (1 - 0.09)))), so
;; scaling E by 0.91 / (1 - nu^2) gives the plate formula with m's own ratio.
;;   obj  a string or plate
;;   m    a material
(def young-for (obj m)
  (if (= (get obj :kind) :plate)
    (/ (* (get m :young) 0.91) (- 1 (* (get m :poisson) (get m :poisson))))
    (get m :young)))

;; Forms defining obj_density and obj_young from what obj is made of: the
;; numbers themselves for one material, picked by the material menu param
;; for several.
;;   obj  a string or plate
(def material-defs (obj)
  (let ((choices (material-choices (get obj :stuff))) (idx (pp obj "material")))
    (list `(def ,(p obj "density") ,(pick-form idx (map (lambda (m) (get m :density)) choices)))
          `(def ,(p obj "young") ,(pick-form idx (map (lambda (m) (young-for obj m)) choices))))))

;; Forms run every sample before predicting obj: its mode frequencies, the
;; mask of modes that sound, damping, then setup-tail's coefficients and
;; state. A finger computes its hand's position on the stroke; a mallet its
;; throw.
;;   obj  the object
(def object-setup (obj)
  (let ((kind (get obj :kind)))
    (if (= kind :melt) (melt-setup obj)
    (if (= kind :mix) (list)
    (if (= kind :data) (data-setup obj)
    (if (= kind :bar) (bar-setup obj)
    (if (= kind :tube) (tube-setup obj)
    (if (= kind :rect-membrane) (rect-membrane-setup obj)
    (if (= kind :circ-plate) (circ-plate-setup obj)
    (if (= kind :string) (string-setup obj)
    (if (= kind :plate) (plate-setup obj)
    (if (= kind :membrane) (membrane-setup obj)
    (if (= kind :two-mass) (finger-setup obj)
      (mallet-setup obj))))))))))))))

;; object-setup for a tube: its ratio and wavenumber tables, frequencies on
;; the note, and losses with the wall's sqrt(f) term.
;;   obj  a tube
(def tube-setup (obj)
  (let ((m (get obj :modes)) (ends (get obj :ends)))
    (append
      (list `(def ,(p obj "wave") ,(tensor-form (map (lambda (k) (tube-wave ends k)) (range m))))
            `(def ,(p obj "f0") (* f0 ,(p obj "tune")))
            `(def ,(p obj "freqs") (* ,(tensor-form (map (lambda (k) (tube-ratio ends k)) (range m))) ,(p obj "f0")))
            (mask-def obj m)
            `(def ,(p obj "sigma") (+ (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "hf_loss"))
                                     (* ,(p obj "wall") 0.1 (sqrt ,(p obj "freqs"))))))
      (setup-tail obj (p obj "f0")))))

;; object-setup for a melt: both parts' setups (their coefficients are what
;; it blends), then its own: log-blended frequencies, blended mask, losses,
;; wref and mass, and its own state.
;;   obj  a melt
(def melt-setup (obj)
  (let ((a (first (get obj :parts))) (b (nth (get obj :parts) 1)) (x (p obj "morph"))
        (lerp (lambda (fa fb) `(+ (* (- 1 ,(p obj "morph")) ,fa) (* ,(p obj "morph") ,fb))))
        (blend (lambda (name) (lerp (p a name) (p b name))))
        ;; a part's release-mute factor (setup-tail's g), 1 when it has none
        (mute-of (lambda (part) (if (muted? part) (p part "g") 1))))
    (append (held-pass a (spectral-pass a (object-setup a))) (held-pass b (spectral-pass b (object-setup b)))
            (list `(def ,(p obj "freqs") (exp (+ (* (- 1 ,x) (log ,(p a "freqs"))) (* ,x (log ,(p b "freqs"))))))
                  `(def ,(p obj "mask") ,(blend "mask"))
                  `(def ,(p obj "sigma") ,(blend "sigma"))
                  `(def ,(p obj "mass") ,(blend "mass"))
                  `(def (,(p obj "a0") ,(p obj "b0")) (modal-coefs ,(p obj "freqs") ,(p obj "sigma")))
                  `(def ,(p obj "g") ,(lerp (mute-of a) (mute-of b)))
                  `(def ,(p obj "a") (* ,(p obj "a0") ,(p obj "g")))
                  `(def ,(p obj "b") (* ,(p obj "b0") ,(p obj "g") ,(p obj "g")))
                  `(def ,(p obj "wref") ,(blend "wref"))
                  `(def ,(p obj "G") (/ (* ,(p obj "wref") ,(p obj "wref")) ,(p obj "mass")))
                  `(make-history ,(p obj "s1") @shape [ ,(get obj :modes) ])
                  `(make-history ,(p obj "s2") @shape [ ,(get obj :modes) ])))))

;; The coefficients and state every modal object ends with, from its
;; obj_freqs and obj_sigma: modal-coefs, wref (its reference angular frequency
;; per sample) from pitch, G (force -> mode gain) from its mass, and the
;; histories of its last two states. An object with a mute param decays faster
;; while the note is released through one scalar factor g on the coefficients
;; (a = 2 r cos w and b = -r^2 take g and g^2), so the coefficients
;; themselves only change when params do.
;;   obj    the object
;;   pitch  a form for the pitch wref is taken at (Hz)
(def setup-tail (obj pitch)
  (let ((n (get obj :modes)) (muted (muted? obj)))
    (append
      (if muted
        (list `(def (,(p obj "a0") ,(p obj "b0")) (modal-coefs ,(p obj "freqs") ,(p obj "sigma")))
              `(def ,(p obj "g") (exp (/ (* -1 ,(pp obj "mute") (- 1 gate)) samplerate)))
              `(def ,(p obj "a") (* ,(p obj "a0") ,(p obj "g")))
              `(def ,(p obj "b") (* ,(p obj "b0") ,(p obj "g") ,(p obj "g"))))
        (list `(def (,(p obj "a") ,(p obj "b")) (modal-coefs ,(p obj "freqs") ,(p obj "sigma")))))
      (list `(def ,(p obj "wref") (min (* twopi (/ ,pitch samplerate)) 2.8))
            `(def ,(p obj "G") (/ (* ,(p obj "wref") ,(p obj "wref")) ,(p obj "mass")))
            `(make-history ,(p obj "s1") @shape [ ,n ])
            `(make-history ,(p obj "s2") @shape [ ,n ])))))

;; True when obj has a mute param (its own, or its first direction's).
;;   obj  an object
(def muted? (obj)
  (member? (map (lambda (spec) (get spec :sym)) (if (get obj :plane-of) (get obj :shared-params) (get obj :params)))
           (pp obj "mute")))

;; The mask of modes that sound: the first count modes, faded below Nyquist.
;;   obj    the object
;;   count  a form or number for how many sound
(def mask-def (obj count)
  `(def ,(p obj "mask") (modal-band (+ (iota ,(get obj :modes)) 1) ,(p obj "freqs") ,count)))

;; object-setup for a stiff string: stiff-string-freqs at the note, from its
;; material, radius and length.
;;   obj  a string
(def string-setup (obj)
  (append (material-defs obj)
          (list `(def ,(p obj "f0") (* f0 ,(pp obj "tune") ,@(if (get obj :plane-of) (list (pp obj "split")) (list))))
                `(def ,(p obj "n") (+ (iota ,(get obj :modes)) 1))
                `(def ,(p obj "freqs") (stiff-string-freqs ,(p obj "n") ,(p obj "f0") ,(p obj "young") ,(p obj "density")
                                         (* ,(pp obj "radius") 0.001) ,(pp obj "length")))
                (mask-def obj (get obj :modes))
                `(def ,(p obj "sigma") (modal-loss ,(p obj "freqs") ,(pp obj "loss") ,(pp obj "hf_loss")))
                `(def ,(p obj "mass") 1))
          (setup-tail obj (p obj "f0"))))

;; object-setup for a rectangular plate: rect-plate over its mode grid, from
;; its material and size; wref at mode (1, 1).
;;   obj  a plate
(def plate-setup (obj)
  (append (material-defs obj) (grid-defs obj)
          (list `(def ,(p obj "thick_m") (* ,(p obj "thick") 0.001))
                `(def ,(p obj "freqs") (rect-plate ,(p obj "pm") ,(p obj "pn") ,(p obj "young") ,(p obj "density")
                                         ,(p obj "thick_m") ,(p obj "width") ,(p obj "height")))
                (mask-def obj (get obj :modes))
                `(def ,(p obj "sigma") (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "damp")))
                `(def ,(p obj "f11") (rect-plate 1 1 ,(p obj "young") ,(p obj "density")
                                       ,(p obj "thick_m") ,(p obj "width") ,(p obj "height"))))
          (setup-tail obj (p obj "f11"))))

;; object-setup for a circular membrane: circ-membrane's Bessel ratios at the
;; note; its modes param masks the rest.
;;   obj  a membrane
(def membrane-setup (obj)
  (append (list `(def (,(p obj "ratios") ,(p obj "phi0")) (circ-membrane 0 0))
                `(def ,(p obj "f0") (* f0 ,(p obj "tune")))
                `(def ,(p obj "freqs") (* ,(p obj "ratios") ,(p obj "f0")))
                (mask-def obj (p obj "modes"))
                `(def ,(p obj "sigma") (+ (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "bright")) ,(p obj "choke")))
                `(def ,(p obj "mass") 1))
          (setup-tail obj (p obj "f0"))))

;; object-setup for a finger: the hand on its stroke and two-mass-coefs.
;;   obj  a finger
(def finger-setup (obj)
  (list `(def ,(p obj "frac") (min (/ (* age 1000) ,(p obj "stroke")) 1))
        `(def ,(p obj "hand") (- ,(p obj "start") (* (+ ,(p obj "start") ,(p obj "depth")) ,(p obj "frac"))))
        `(def (,(p obj "a") ,(p obj "b") ,(p obj "follow") ,(p obj "C"))
           (two-mass-coefs ,(p obj "hz") ,(p obj "damp") ,(p obj "stiff")))
        `(make-history ,(p obj "s1"))
        `(make-history ,(p obj "s2"))))

;; object-setup for a mallet: its drop height, throw speed and compliance.
;;   obj  a mallet
(def mallet-setup (obj)
  (list `(def ,(p obj "drop") 0.001)
        `(def ,(p obj "throw") (/ (* ,(p obj "speed") (+ 0.2 (* 0.8 hit_velocity))) samplerate))
        `(def ,(p obj "wref") (min (* twopi (/ f0 samplerate)) 2.8))
        `(def ,(p obj "C") (/ (* ,(p obj "wref") ,(p obj "wref")) ,(p obj "mass")))
        `(make-history ,(p obj "hx"))
        `(make-history ,(p obj "hv"))))

;; object-setup for a circular plate: lambda^2, orders and the radial shape
;; table as tensors; f = lambda^2 c h / (2 pi a^2) with
;; c = sqrt(E / (12 rho (1 - nu^2))) per material; wref at its lowest mode.
;;   obj  a circular plate
(def circ-plate-setup (obj)
  (let ((modes (get obj :plate-modes))
        (c-of (lambda (m) (sqrt (/ (get m :young) (* 12 (get m :density) (- 1 (* (get m :poisson) (get m :poisson))))))))
        (l2 (lambda (mode) (* (get mode :lambda) (get mode :lambda)))))
    (append
      (list `(def ,(p obj "c") ,(pick-form (p obj "material") (map c-of (material-choices (get obj :stuff)))))
            `(def ,(p obj "l2") ,(tensor-form (map l2 modes)))
            `(def ,(p obj "order") ,(tensor-form (map (lambda (mode) (get mode :m)) modes)))
            `(def ,(p obj "radial")
               ,(table-form (map (lambda (r) (map (lambda (mode) (nth (get mode :radial) r)) modes)) (range (circ-samples)))))
            `(def ,(p obj "scale") (/ (* ,(p obj "c") ,(p obj "thick") 0.001 ,(p obj "tune"))
                                      (* twopi ,(p obj "radius") ,(p obj "radius"))))
            `(def ,(p obj "freqs") (* ,(p obj "l2") ,(p obj "scale")))
            (mask-def obj (len modes))
            `(def ,(p obj "sigma") (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "damp"))))
      (setup-tail obj `(* ,(l2 (first modes)) ,(p obj "scale"))))))

;; The grid of mode indices of a rectangular object: obj_grid 0..g^2-1 and
;; its 1-based (m, n), obj_pm and obj_pn.
;;   obj  a plate or rectangular membrane
(def grid-defs (obj)
  (let ((g (get obj :grid)))
    (list `(def ,(p obj "grid") (iota ,(* g g)))
          `(def ,(p obj "pm") (+ (floor (/ ,(p obj "grid") ,g)) 1))
          `(def ,(p obj "pn") (+ (- ,(p obj "grid") (* ,g (floor (/ ,(p obj "grid") ,g)))) 1)))))

;; object-setup for a bar: the beam tables as tensors, frequencies from the
;; note through the undercut blend (in log frequency), losses.
;;   obj  a bar
(def bar-setup (obj)
  (let ((m (get obj :modes)) (bs (bar-betas (get obj :modes))))
    (append
      (list `(def ,(p obj "beta") ,(tensor-form bs))
            `(def ,(p obj "s") ,(tensor-form (map bar-s bs)))
            `(def ,(p obj "A") ,(tensor-form (map bar-a bs)))
            `(def ,(p obj "B") ,(tensor-form (map (lambda (b) (/ (+ 1 (bar-s b)) 2)) bs)))
            `(def ,(p obj "f0") (* f0 ,(p obj "tune")))
            `(def ,(p obj "ratio") (exp (+ (* (- 1 ,(p obj "undercut")) (log ,(tensor-form (bar-ratios m))))
                                           (* ,(p obj "undercut") (log ,(tensor-form (bar-targets m)))))))
            `(def ,(p obj "freqs") (* ,(p obj "ratio") ,(p obj "f0")))
            (mask-def obj m)
            `(def ,(p obj "sigma") (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "hf_loss"))))
      (setup-tail obj (p obj "f0")))))

;; object-setup for a rectangular membrane: mode (m, n) at
;; sqrt((m / aspect)^2 + n^2) / sqrt(1 / aspect^2 + 1) times the note.
;;   obj  a rectangular membrane
(def rect-membrane-setup (obj)
  (let ((aspect (p obj "aspect")))
    (append
      (grid-defs obj)
      (list `(def ,(p obj "f0") (* f0 ,(p obj "tune")))
            `(def ,(p obj "freqs")
               (* ,(p obj "f0")
                  (/ (sqrt (+ (pow (/ ,(p obj "pm") ,aspect) 2) (pow ,(p obj "pn") 2)))
                     (sqrt (+ (pow (/ 1 ,aspect) 2) 1)))))
            (mask-def obj (get obj :modes))
            `(def ,(p obj "sigma") (modal-loss ,(p obj "freqs") ,(p obj "loss") ,(p obj "hf_loss"))))
      (setup-tail obj (p obj "f0")))))

;; object-setup for a data object: its numbers as tensors (the shape rows of
;; its compiled points only), then the same coefficients as any modal object.
;; Losses add muffle's sqrt(f) term (of the data's own frequencies), and the always-on air loss of
;; modal-loss unless the data is measured. wref is at the pitch heard: the
;; root when the data names one (a bell's hum sits an octave under it), else
;; the lowest mode. Its ratio to the data's own pitch is the tune param,
;; times the note over the root, or the note's octave (:follow :octave);
;; then it is held on coef_tick (see held-pass).
;;   obj  a data object (after compile-points)
(def data-setup (obj)
  (let ((rows (map (lambda (k) (nth (get obj :shapes) k)) (get obj :compiled)))
        (ratio (if (get obj :root) `(* ,(p obj "tune") (/ f0 ,(get obj :root)))
                 (if (= (get obj :follow) :octave) `(* ,(p obj "tune") key_transpose) (p obj "tune"))))
        (pitch (if (get obj :root) (get obj :root) (first (get obj :freqs)))))
    (append
      (list `(def ,(p obj "hz") ,(tensor-form (get obj :freqs)))
            `(def ,(p obj "loss0") ,(tensor-form (get obj :losses)))
            `(def ,(p obj "shapes") ,(table-form rows))
            `(def ,(p obj "ratio") ,(if (get obj :held) `(event-hold ,ratio coef_tick) ratio))
            `(def ,(p obj "freqs") (* ,(p obj "hz") ,(p obj "ratio")))
            (mask-def obj (get obj :modes))
            `(def ,(p obj "sigma")
               (+ (* ,(p obj "loss0") ,(p obj "decay"))
                  ,(muffle-loss-form obj (p obj "hz"))
                  ,@(if (get obj :air-loss) (list `(modal-loss ,(p obj "freqs") 0 0)) (list)))))
      (setup-tail obj `(* ,pitch ,(p obj "ratio"))))))

;; The setup forms of an object whose coefficients are held (:held, a data
;; object following the note: :root or :follow), rewritten so their work runs
;; on coef_tick only: what the per-sample code reads (the coefficients, with the release
;; mute's factor g, the mask and wref) is defined as obj_<name>_e, computed
;; on the tick's event clock, and obj_<name> latches it back to audio rate.
;; Every input of that work is event-held (the ratio, params, g), so DGen
;; schedules it on the tick; reading the latched value every sample costs
;; nothing. Any other object's forms pass through.
;;   obj    an object
;;   forms  its setup forms, after spectral-pass
(def held-pass (obj forms)
  (if (not (get obj :held)) forms
    (let ((names (map (lambda (n) (p obj n)) (list "a" "b" "mask" "wref")))
          (held (lambda (sym) (member? names sym)))
          (event (lambda (sym) (if (held sym) (symbol sym "_e") sym)))
          (g (p obj "g"))
          (latches (lambda (syms) (map (lambda (sym) `(def ,sym (latch ,(symbol sym "_e") coef_tick))) (filter held syms)))))
      (concat-all
        (map (lambda (f)
               (if (and (list? f) (= (first f) 'def))
                 (let ((target (nth f 1)))
                   (if (list? target)
                     (if (not (any? held target)) (list f)
                       (cons `(def ,(map event target) ,(nth f 2)) (latches target)))
                     (if (held target)
                       (cons `(def ,(event target) ,(subst-form (nth f 2) g `(event-hold ,g coef_tick))) (latches (list target)))
                       (list f))))
                 (list f)))
             forms)))))

;; form with every occurrence of the symbol from replaced by to.
;;   form  a form
;;   from  a symbol
;;   to    its replacement
(def subst-form (form from to)
  (if (= form from) to
    (if (list? form) (map (lambda (x) (subst-form x from to)) form) form)))

;; True when a patch has an object holding its coefficients (see held-pass).
;;   patch  the patch
(def any-held? (patch) (any? (lambda (o) (get o :held)) (all-objects patch)))

;; True when a patch reads the played key (an object following its octave,
;; or strokes picked by it).
;;   patch  the patch
(def keyed? (patch)
  (or (any? (lambda (o) (= (get o :follow) :octave)) (all-objects patch))
      (any? strokes? (get patch :connections))))

;; Forms for the played key, latched when a note starts: key_offset
;; (semitones from C4), key_row (0..11 within its octave) and key_transpose
;; (2 to the octave from C4's, -3..+3 as tools/pm-ride/build.py's
;; OCTAVE_RANGE).
(def key-forms ()
  (list '(def key_offset (latch (round (+ (* 12 (/ (log (/ f0 440)) (log 2))) 9)) hit))
        '(def key_octave (floor (/ key_offset 12)))
        '(def key_row (- key_offset (* 12 key_octave)))
        '(def key_transpose (pow 2 (clip key_octave -3 3)))))

;; What coef_tick's coefficients are computed from, besides the key: the
;; params of each held object and of each object strokes play (whose wash
;; bank is latched on it), the gate (for a release mute), and the note's
;; pitch for an object following it continuously (:root, or spectral
;; :follow-note).
;;   patch  the patch
(def coef-inputs (patch)
  (let ((struck (map (lambda (c) (get c :b)) (filter strokes? (get patch :connections)))))
    (let ((objs (filter (lambda (o) (or (get o :held) (member? struck (get o :id)))) (all-objects patch))))
      (append (list 'gate)
              (if (any? (lambda (o) (and (get o :held) (or (get o :root) (and (get o :spectral) (get (get o :spectral) :follow)))))
                        objs)
                (list 'f0) (list))
              (concat-all (map (lambda (o) (map (lambda (spec) (get spec :sym)) (get o :params))) objs))))))

;; Forms defining coef_tick, when held coefficients are computed (on
;; DGenLisp's event clock): a note's start, the first sample, and any sample
;; on which one of their inputs changes (exact change detection, as the
;; factory Modal Snare's). A param moving continuously recomputes them every
;; sample while it moves; a still one costs nothing.
;;   patch  the patch
(def coef-tick-forms (patch)
  (let ((inputs (coef-inputs patch))
        (h (lambda (k) (symbol "coef_in" k))))
    (append (list '(make-history coef_started))
            (map (lambda (k) `(make-history ,(h k))) (range (len inputs)))
            (list `(def coef_tick (max hit (lte (read-history coef_started) 0)
                                       ,@(map (lambda (k) `(gt (abs (- ,(nth inputs k) (read-history ,(h k)))) 0))
                                              (range (len inputs)))))
                  '(write-history coef_started 1))
            (map (lambda (k) `(write-history ,(h k) ,(nth inputs k))) (range (len inputs))))))

;; Protocol step 1: forms defining obj_free, where obj would be this sample
;; with no force on it (a finger's tip also follows its hand; a mallet also
;; defines obj_v, its velocity).
;;   obj  the object
(def object-predict (obj)
  (let ((kind (get obj :kind))
        (free `(modal-predict (read-history ,(p obj "s1")) (read-history ,(p obj "s2")) ,(p obj "a") ,(p obj "b"))))
    (if (= kind :two-mass)
      (list `(def ,(p obj "free") (+ ,free (* ,(p obj "follow") ,(p obj "hand")))))
      (if (= kind :mallet)
        (list `(def (,(p obj "free") ,(p obj "v"))
                 (mallet-predict (read-history ,(p obj "hx")) (read-history ,(p obj "hv")) hit ,(p obj "drop") ,(p obj "throw"))))
        (list `(def ,(p obj "free") ,free))))))

;; A form for obj's state after forces already solved this sample: its free
;; state plus G (or C, lumped) times their sum.
;;   obj    the object
;;   terms  signed (F phi) products already applied to it, as forms
(def displaced (obj terms)
  (if (empty? terms) (p obj "free")
    (if (get obj :lumped)
      `(+ ,(p obj "free") (* ,(p obj "C") ,(sum-form terms)))
      `(+ ,(p obj "free") (* ,(p obj "G") ,(sum-form terms))))))

;; Protocol step 4: forms defining obj_next, obj's state with every force
;; applied, and writing it to its histories. The library macros take and
;; return values; the patch owns the histories.
;;   obj     the object
;;   terms   every connection's force term on it, as terms-of gives
;;   pushes  what strokes and mounts add to its modes' state directly
;;           (pushes-on)
(def object-commit (obj terms pushes)
  (let ((kind (get obj :kind)) (total (sum-form terms)))
    (if (= kind :mallet)
      (list `(def (,(p obj "next") ,(p obj "next_v"))
               (mallet-advance ,(p obj "free") ,(p obj "v") (* -1 ,total) ,(p obj "C") ,(p obj "drop")))
            `(write-history ,(p obj "hx") ,(p obj "next"))
            `(write-history ,(p obj "hv") ,(p obj "next_v")))
      (list `(def ,(p obj "next") (modal-advance ,(p obj "free")
                                    ;; even an empty force term (G 0): without it DGen
                                    ;; fuses the strokes' push worse (15% more CPU)
                                    ,(sum-form (cons (if (= kind :two-mass) `(* ,(p obj "C") ,total) `(* ,(p obj "G") ,total))
                                                     pushes))))
            `(write-history ,(p obj "s2") (read-history ,(p obj "s1")))
            `(write-history ,(p obj "s1") ,(p obj "next"))))))

;; ── connections ─────────────────────────────────────────────────────────────

;; The force terms connection c applies to object id when its force is the
;; symbol f: (+f phi_b) on b, (-f phi_a) on a, phi left out for a lumped end;
;; none when id is neither end.
;;   c   a resolved connection (see resolve-connection)
;;   id  an object id
;;   f   the symbol holding c's force (cid_F, or one sweep's cid_F2)
(def terms-with (c id f)
  (let ((cid (get c :id)))
    ;; a :split connection (lips) pushes its a end by its own force
    (let ((fa (if (get c :split) (symbol f "_a") f)))
    (append
      (if (= (get c :b) id) (list (if (get c :b-lumped) f `(* ,f ,(nm cid "phi_b")))) nil)
      (if (= (get c :a) id) (list (if (get c :a-lumped) `(* -1 ,fa) `(* -1 ,fa ,(nm cid "phi_a")))) nil)
      ;; a part of a mix hybrid at either end: its share of the force
      (concat-all
        (map (lambda (side)
               (let ((mix (get c (if (= side "a") :a-mix :b-mix))))
                 (if (and mix (member? (get mix :parts) id))
                   (let ((k (index-of (get mix :parts) id)) (x (nm (get mix :id) "morph")))
                     (list `(* ,(if (= side "a") -1 1) ,(if (= k 0) `(- 1 ,x) x) ,(if (= side "a") fa f)
                              ,(nm cid (str "phi_" side "_" k)))))
                   nil)))
             (list "a" "b")))))))

;; terms-with c's final force, cid_F (none for strokes and mounts, which
;; push the modes' state directly: see pushes-on).
;;   c   a resolved connection
;;   id  an object id
(def terms-on (c id) (if (pusher? c) nil (terms-with c id (nm (get c :id) "F"))))

;; Every final force term on object id from the connections at indices.
;;   conns    the resolved connections
;;   indices  which of them, as positions in conns
;;   id       an object id
(def terms-of (conns indices id)
  (concat-all (map (lambda (j) (terms-on (nth conns j) id)) indices)))

;; Protocol step 2 for one end of c: forms defining cid_y_<side><tag>, where
;; the access point stands given terms, and cid_C_<side><tag>, how far it
;; moves per unit force. A modal end first defines cid_phi_<side>.
;;   c         a resolved connection
;;   side      "a" or "b"
;;   obj       that end's object
;;   terms     forces already applied to obj, as forms
;;   tag       "" for exciters, the sweep number for springs and bows
;;   with-phi  true the first time this end is read (phi is defined once)
(def end-forms (c side obj terms tag with-phi)
  (let ((cid (get c :id)) (y (nm cid (str "y_" side tag))) (cs (nm cid (str "C_" side tag)))
        (coord-id (if (get c :coords-of) (get c :coords-of) cid))
        ;; placed at another connection's point (on its object, maybe in the
        ;; other direction): that point's params, named by its object
        (named (if (and (get c :coords-of) (get obj :plane-of)) (merge obj :id (get obj :plane-of)) obj)))
    (if (= (get obj :kind) :mix) (mix-end-forms c side obj terms tag with-phi)
    (if (get obj :lumped)
      (list `(def ,y ,(displaced obj (terms (get obj :id))))
            `(def ,cs ,(p obj "C")))
      (append (if with-phi (shape-defs obj (access-syms coord-id side named) (nm cid (str "phi_" side))) nil)
              (list `(def (,y ,cs) (modal-point ,(displaced obj (terms (get obj :id))) ,(nm cid (str "phi_" side))
                                                ,(p obj "G")))))))))

;; end-forms for an end on a mix hybrid: each part's point (phi, y, C) as for
;; any end, then the lever: y = (1 - x) ya + x yb, C = (1 - x)^2 Ca + x^2 Cb.
;;   c, side, obj, terms, tag, with-phi  as for end-forms (obj the hybrid,
;;                                       with its :part-objs)
(def mix-end-forms (c side obj terms tag with-phi)
  (let ((cid (get c :id)) (x (p obj "morph")) (parts (get obj :part-objs))
        (coord-id (if (get c :coords-of) (get c :coords-of) (get c :id)))
        (y (nm cid (str "y_" side tag))) (cs (nm cid (str "C_" side tag))))
    (append
      (concat-all
        (map (lambda (k)
               (let ((part (nth parts k)) (sfx (str side "_" k)))
                 (append (if with-phi (shape-defs part (access-syms coord-id side part) (nm cid (str "phi_" sfx))) nil)
                         (list `(def (,(nm cid (str "y_" sfx tag)) ,(nm cid (str "C_" sfx tag)))
                                  (modal-point ,(displaced part (terms (get part :id))) ,(nm cid (str "phi_" sfx))
                                               ,(p part "G")))))))
             (range 2)))
      (list `(def ,y (+ (* (- 1 ,x) ,(nm cid (str "y_" side "_0" tag))) (* ,x ,(nm cid (str "y_" side "_1" tag)))))
            `(def ,cs (+ (* (- 1 ,x) (- 1 ,x) ,(nm cid (str "C_" side "_0" tag)))
                         (* ,x ,x ,(nm cid (str "C_" side "_1" tag)))))))))

;; Protocol step 3 for an exciter (pluck, felt or force): forms defining cid_F from
;; the ends end-forms read (tag ""). Exciters keep state, so each is solved
;; once per sample. A pluck's release threshold scales with the hit's
;; velocity; a felt stiffens with last sample's squash.
;;   c  a resolved pluck or felt connection
(def exciter-forms (c patch)
  (let ((cid (get c :id)) (gap `(- ,(nm cid "y_a") ,(nm cid "y_b"))) (cc `(+ ,(nm cid "C_a") ,(nm cid "C_b"))))
    (if (= (get c :law) :pluck)
      (list `(def ,(nm cid "thr") (* ,(nm cid "thresh") (+ 0.25 (* 0.75 hit_velocity))))
            `(def ,(nm cid "F") (* ,(nm cid "weight") (pluck-contact ,gap ,cc hit ,(nm cid "stiff") ,(nm cid "thr")))))
    (if (= (get c :law) :jet) (jet-forms c patch)
    (if (= (get c :law) :force)
      (append (signal-forms c patch)
              (list `(def ,(nm cid "F") (* ,(nm cid "weight") ,(nm cid "gain")
                                           ,(if (get c :saturate) `(tanh ,(nm cid "sig")) (nm cid "sig"))))))
    (if (get c :stulov) (stulov-forms c gap cc)
      ;; felt: stiffens with last sample's squash, scaled by a full-speed
      ;; sample of mallet travel
      (list `(make-history ,(nm cid "squash"))
            `(def ,(nm cid "gap") ,gap)
            `(def ,(nm cid "Cs") ,cc)
            `(def ,(nm cid "k") (* ,(nm cid "felt") (+ 1 (* ,(nm cid "hardness")
                                   (/ (read-history ,(nm cid "squash")) (/ ,(nm (get c :a) "speed") samplerate))))))
            `(def ,(nm cid "F") (* ,(nm cid "weight") (felt-contact ,(nm cid "gap") ,(nm cid "Cs") ,(nm cid "k"))))
            `(write-history ,(nm cid "squash") (max 0 (* -1 (- ,(nm cid "gap") (* ,(nm cid "Cs") ,(nm cid "F")))))))))))))

;; The jet: breath (with turbulence) plus the air's velocity at the mouth
;; last sample (for a tube: its modes' pressure gradient there),
;; delayed by ratio periods of the note, through tanh.
;;   c      a jet connection
;;   patch  the patch
(def jet-forms (c patch)
  (let ((n (lambda (suffix) (nm (get c :id) suffix))) (tube (find-object patch (get c :b))))
    (append (signal-forms c patch)
            (list `(def ,(n "breath") (* ,(n "blow") (max 0 ,(n "sig"))))
                  ;; the air's velocity at the embouchure, which steers the
                  ;; jet: each mode's amplitude times its pressure gradient
                  ;; shape there (largest at an open end)
                  `(def ,(n "grad") ,(tube-shape tube (first (access-syms (get c :id) "b" tube)) true))
                  `(def ,(n "mouth") (sum (* (read-history ,(p tube "s1")) ,(n "grad"))))
                  `(def ,(n "in") (+ (* ,(n "breath") (+ 1 (* ,(n "noise") ,(white-noise-form))))
                                     (* ,(n "feedback") ,(n "mouth"))))
                  ;; ratio periods of the tube's own pitch (ratio <= 2, f0 >= 20 Hz)
                  `(def ,(n "delayed") (delay ,(n "in") (/ (* ,(n "ratio") samplerate) ,(p tube "f0")) @max-delay 9600))
                  `(make-history ,(n "u_h"))
                  ;; the jet's flow into the tube; the tube takes its change
                  ;; (see blown-forms)
                  `(def ,(n "u") (tanh (- ,(n "delayed") ,(n "breath"))))
                  `(def ,(n "F") (* ,(weight-of c) ,(n "gain") (- ,(n "u") (read-history ,(n "u_h")))))
                  `(write-history ,(n "u_h") ,(n "u"))))))

;; Stulov's felt, solved inside the sample: the force magnitude f that
;; equals law(f) = Q / C u0 (r^p - eps h) with r = u / u0 the compression it
;; leaves, u = max(0, -gap - C f). g(f) = law(f) - f is convex and falling
;; on [0, -gap / C], so Newton from f = 0 rises to the root without
;; overshooting: 5 steps of f += g / (Q p r^(p-1) + 1), each one pow, clamped
;; to the bracket. Then h follows r^p over tau. F = -f (felt-contact's sign).
;;   c    a felt connection with :stulov
;;   gap  the free gap form
;;   cc   the summed compliance form
(def stulov-forms (c gap cc)
  (let ((n (lambda (suffix) (nm (get c :id) suffix)))
        (steps 5)
        ;; r^(p-1) and r at force f, as forms
        (ratio (lambda (f) `(/ (max 0 (- (* -1 ,(nm (get c :id) "gap")) (* ,(nm (get c :id) "Cs") ,f))) 0.001))))
    (append
      (list `(make-history ,(n "h"))
            `(def ,(n "gap") ,gap)
            `(def ,(n "Cs") ,cc)
            `(def ,(n "hi") (/ (max 0 (* -1 ,(n "gap"))) ,(n "Cs")))
            `(def ,(n "f0") 0))
      (concat-all
        (map (lambda (k)
               (let ((f (n (str "f" k))) (r (n (str "r" k))) (q (n (str "q" k))))
                 (list `(def ,r ,(ratio f))
                       ;; q = r^(p-1); r^p = q r
                       `(def ,q (pow ,r (- ,(n "exponent") 1)))
                       `(def ,(n (str "f" (+ k 1)))
                          (clip (+ ,f (/ (- (* (/ ,(n "felt") ,(n "Cs")) 0.001
                                                 (- (* ,q ,r) (* ,(n "hysteresis") (read-history ,(n "h")))))
                                              ,f)
                                           (+ (* ,(n "felt") ,(n "exponent") ,q) 1)))
                                0 ,(n "hi"))))))
             (range steps)))
      (list `(def ,(n "f") ,(n (str "f" steps)))
            `(def ,(n "F") (* -1 ,(n "weight") ,(n "f")))
            `(def ,(n "rf") ,(ratio (n "f")))
            `(write-history ,(n "h")
               (+ (read-history ,(n "h"))
                  (* (- (pow ,(n "rf") ,(n "exponent")) (read-history ,(n "h")))
                     (- 1 (exp (/ -1000 (* (max ,(n "tau") 0.001) samplerate)))))))))))

;; Forms emitted once before the sweeps for a bow: its speed envelope
;; (cid_env, rising over attack while the gate is up and falling over
;; release), its velocity cid_vb this sample, cid_vmax, and the histories of
;; its point's last position and the friction solver's last root.
;;   c  a resolved bow connection
(def bow-pre-forms (c)
  (let ((cid (get c :id)))
    (append
      (list `(make-history ,(nm cid "env_h"))
            `(def ,(nm cid "env") (max (min (+ (read-history ,(nm cid "env_h"))
                                               (gswitch (gt gate 0.5) (/ 1000 (* ,(nm cid "attack") samplerate))
                                                                      (/ -1000 (* ,(nm cid "release") samplerate))))
                                            1) 0))
            `(write-history ,(nm cid "env_h") ,(nm cid "env"))
            `(def ,(nm cid "vmax") (/ ,(nm cid "speed") samplerate))
            `(def ,(nm cid "vb") (* ,(nm cid "vmax") ,(nm cid "env") (+ 0.5 (* 0.5 velocity))))
            `(make-history ,(nm cid "y_prev"))
            `(make-history ,(nm cid "x_prev")))
      ;; the hair contact's stiffness and depth (see hair-connection)
      (if (get c :hair)
        (list `(make-history ,(nm cid "press_h"))
              `(make-history ,(nm cid "avg_h"))
              `(def ,(nm cid "hair_stiff") 300)
              `(def ,(nm cid "hair_height") (* -1 ,(nm cid "hair"))))
        (list)))))

;; Forms emitted once before the sweeps for a position or speed driver: its
;; signal and cid_anchor, the target its spring pulls the point toward (the
;; signal itself, or for speed its leaky integral).
;;   c  a position or speed driver
(def driver-pre-forms (c patch)
  (let ((cid (get c :id)))
    (append (signal-forms c patch)
            (if (member? (list :reed :lips) (get c :law))
              (list `(make-history ,(nm cid "u_h"))
                    ;; the blowing pressure, once per sample
                    `(def ,(nm cid "gamma") (* ,(nm cid "blow") (max 0 ,(nm cid "sig")))))
            (if (= (get c :law) :fingerboard) (list)
            (if (= (get c :law) :position)
              (list `(def ,(nm cid "anchor") ,(nm cid "sig")))
              (list `(make-history ,(nm cid "anchor_h"))
                    `(def ,(nm cid "anchor") (+ (* (exp (/ -0.5 samplerate)) (read-history ,(nm cid "anchor_h")))
                                                (/ (* ,(nm cid "gain") ,(nm cid "sig")) samplerate)))
                    `(write-history ,(nm cid "anchor_h") ,(nm cid "anchor")))))))))

;; Connection predicates. Springs, bows and position / speed drivers are
;; solved by the sweeps (sweeps?); plucks, felts and forces are exciters,
;; solved once beforehand.
;;   c  a connection
(def spring? (c) (= (get c :law) :spring))
(def bow? (c) (= (get c :law) :bow))
(def driver? (c) (member? (list :position :speed :fingerboard :reed :lips) (get c :law)))
(def sweeps? (c) (or (spring? c) (bow? c) (driver? c) (member? (list :strike :stop) (get c :law))))
(def exciter? (c) (not (sweeps? c)))

;; The positions in conns of the connections pred accepts.
;;   conns  a list of connections
;;   pred   a predicate on one connection
(def indices-where (conns pred) (filter (lambda (k) (pred (nth conns k))) (range (len conns))))

;; How many items of xs are less than x: x's position in the sorted list xs.
;;   xs  a sorted list of numbers
;;   x   a number
(def position-of (xs x) (len (filter (lambda (k) (< (nth xs k) x)) (range (len xs)))))

;; Protocol step 3 for springs and bows. They are stateless per call, so
;; when several share objects they are solved by Gauss-Seidel sweeps: in
;; sweep s each one defines cid_F<s>, reading its ends with every exciter's
;; force, the connections before it at this sweep's force, and those after
;; it at the last sweep's. One sequential pass is not exact when they share
;; an object: stiff springs on a light shared body grew instead of settling,
;; and a bow solved before a bridge read the bridge's push as string velocity
;; and pumped energy in after release. 4 sweeps converge. The last sweep's
;; force becomes cid_F (and a bow keeps its solver root in x_prev).
;;   patch     the patch, to find each end's object
;;   conns     the resolved connections
;;   exciters  positions in conns of the exciters
;;   springs   positions in conns of the springs and bows, in order
;;   sweeps    how many sweeps to emit
(def spring-sweep-forms (patch conns exciters springs sweeps)
  (concat-all
    (map (lambda (s)
           (concat-all
             (map (lambda (k)
                    (let ((c (nth conns k)) (cid (get (nth conns k) :id)) (tag (str s))
                          (others (lambda (id)
                                    (append (terms-of conns exciters id)
                                            (concat-all
                                              (map (lambda (j)
                                                     (if (= j k) nil
                                                       (if (< (position-of springs j) (position-of springs k))
                                                         (terms-with (nth conns j) id (nm (get (nth conns j) :id) (str "F" s)))
                                                         (if (> s 0)
                                                           (terms-with (nth conns j) id (nm (get (nth conns j) :id) (str "F" (- s 1))))
                                                           nil))))
                                                   springs))))))
                      (append (if (get c :a) (end-forms c "a" (find-object patch (get c :a)) others tag (= s 0)) nil)
                              (end-forms c "b" (find-object patch (get c :b)) others tag (= s 0))
                              (sweep-law-forms c s tag)
                              (if (= s (- sweeps 1))
                                (append (list `(def ,(nm cid "F") ,(nm cid (str "F" s))))
                                        ;; a blown law's flow, for next sample's change; lips' own lip force
                                        (if (member? (list :reed :lips) (get c :law))
                                          (list `(write-history ,(nm cid "u_h") ,(blown-sym c (nm cid (str "F" s)) "u")))
                                          nil)
                                        (if (get c :split)
                                          (list `(def ,(symbol (nm cid "F") "_a") ,(symbol (nm cid (str "F" s)) "_a")))
                                          nil)
                                        (if (bow? c) (list `(write-history ,(nm cid "x_prev") ,(nm cid (str "x" s)))) nil))
                                nil))))
                  springs)))
         (range sweeps))))

;; One sweep of one swept connection: forms defining cid_F<s> from its ends
;; as they stand this sweep (cid_y_a<tag> / C_a, y_b / C_b). A bow solves its
;; friction; a position or speed driver pulls toward its anchor; a strike or
;; stop pushes only while pressed in; a spring pulls the two points together.
;;   c    a swept connection
;;   s    the sweep number
;;   tag  the sweep's name suffix
(def sweep-law-forms (c s tag)
  (let ((n (lambda (suffix) (nm (get c :id) suffix)))
        (w (weight-of c))
        (F (nm (get c :id) (str "F" s)))
        (ya (nm (get c :id) (str "y_a" tag))) (yb (nm (get c :id) (str "y_b" tag)))
        (ca (nm (get c :id) (str "C_a" tag))) (cb (nm (get c :id) (str "C_b" tag)))
        (law (get c :law)))
    (if (bow? c)
      (list `(def (,F ,(n (str "x" s)))
               (bow-contact ,yb (read-history ,(n "y_prev")) (read-history ,(n "x_prev")) ,cb ,(n "vb")
                            (* ,(n "slip") ,(n "vmax"))
                            ;; the bow lifts as it slows; with hair, the
                            ;; pressure follows the hair contact's force
                            (max 0.000001 (* ,(n "pressure") ,(n "env") ,w
                                             ,@(if (get c :hair) (list `(read-history ,(n "press_h"))) (list)))))))
    (if (= law :reed) (reed-law-forms c F yb cb w)
    (if (= law :lips) (lips-law-forms c s F ya yb cb w)
    (if (= law :fingerboard)
      (list `(def ,F (* ,w (max 0 ,(n "sig")) (spring-connection (- 0 ,yb) ,cb ,(n "stiff")))))
    (if (driver? c)
      (list `(def ,F (* ,w (spring-connection (- ,(n "anchor") ,yb) ,cb ,(n "stiff")))))
    (if (= law :strike)
      (list `(def ,F (* ,w (felt-contact (+ (- ,ya ,yb) ,(n "offset")) (+ ,ca ,cb) ,(n "stiff")))))
    (if (= law :stop)
      (if (get c :above)
        ;; a stop above b: b is pushed down while above height
        (list `(def ,F (* ,w (felt-contact (- ,(n "height") ,yb) ,cb ,(n "stiff")))))
        ;; the stop is the lower end, fixed: b is pushed up by -felt-contact
        (list `(def ,F (* -1 ,w (felt-contact (- ,yb ,(n "height")) ,cb ,(n "stiff"))))))
      (list `(def ,F (* ,w (spring-connection (- ,ya ,yb) (+ ,ca ,cb) ,(n "stiff"))))))))))))))

;; A blown flow law solved for the mouthpiece pressure. An air column's
;; modes are driven by the rate of change of the flow into it (its input
;; impedance is j w / (w_n^2 - w^2 + ...)), so the force on the tube is
;; coupling times the flow's change this sample, u(p) - u_prev. Forms
;; defining F where p = y / scale is the pressure that leaves:
;; p = p0 + K (u(p) - u_prev), p0 = yb / scale, K = w C coupling / scale
;; (capped at 0.9 / zeta so h(p) = p - p0 - K (u(p) - u_prev) rises: one
;; root); by 12 bisection steps on a bracket covering the flow there.
;;   c       the connection
;;   F       the force symbol to define
;;   yb, cb  the tube point's free pressure and give, as symbols
;;   w       the weight form
;;   flow    (lambda (p) form) for the flow at pressure p
;;   umax    a bound on |flow|
(def blown-forms (c F yb cb w flow umax)
  (let ((n (lambda (suffix) (blown-sym c F suffix)))
        (prev `(read-history ,(nm (get c :id) "u_h")))
        (steps 12))
    (let ((p0 (n "p0")) (k (n "K")) (half (n "half")))
      (append
        (list `(def ,p0 (/ ,yb ,(nm (get c :id) "scale")))
              ;; with the weight, so the solved pressure is the applied
              ;; force's; capped so h stays monotone (K u' < 1, u' <= zeta)
              `(def ,k (min (/ (* ,w ,cb ,(nm (get c :id) "coupling")) ,(nm (get c :id) "scale"))
                            (/ 0.9 (max ,(nm (get c :id) "zeta") 0.000001))))
              ;; wide enough for the flow near p0 and the bound on it
              `(def ,half (+ (* 2 ,k (+ ,umax (abs ,(flow p0)) (abs ,prev))) 0.000001))
              `(def ,(n "lo0") (- ,p0 ,half))
              `(def ,(n "hi0") (+ ,p0 ,half)))
        (concat-all
          (map (lambda (j)
                 (let ((lo (n (str "lo" j))) (hi (n (str "hi" j))) (mid (n (str "mid" j))) (up (n (str "up" j))))
                   (list `(def ,mid (* 0.5 (+ ,lo ,hi)))
                         ;; h(mid) > 0: the root is below mid
                         `(def ,up (gt (- ,mid ,p0 (* ,k (- ,(flow mid) ,prev))) 0))
                         `(def ,(n (str "lo" (+ j 1))) (gswitch ,up ,lo ,mid))
                         `(def ,(n (str "hi" (+ j 1))) (gswitch ,up ,mid ,hi)))))
               (range steps)))
        (list `(def ,(n "p") (* 0.5 (+ ,(n (str "lo" steps)) ,(n (str "hi" steps)))))
              `(def ,(n "u") ,(flow (n "p")))
              `(def ,F (* ,w ,(nm (get c :id) "coupling") (- ,(n "u") ,prev))))))))

;; The symbol of one of a blown law's per-sweep values: cid_<suffix>_<F>.
;;   c       the connection
;;   F       that sweep's force symbol
;;   suffix  the value's name
(def blown-sym (c F suffix) (nm (get c :id) (str suffix "_" (source F))))

;; The reed law (see reed): gamma from the breath, the flow
;; zeta (1 - gamma + p) sqrt|gamma - p| sign(gamma - p) above p = gamma - 1.
;;   c, F, yb, cb, w  as for blown-forms
(def reed-law-forms (c F yb cb w)
  (let ((n (lambda (suffix) (nm (get c :id) suffix))))
    (append
      (blown-forms c F yb cb w
                   (lambda (pp) `(* (gt ,pp (- ,(n "gamma") 1)) ,(n "zeta") (+ 1 (- ,(n "gamma")) ,pp)
                                    (sqrt (abs (- ,(n "gamma") ,pp))) (sign (- ,(n "gamma") ,pp))))
                   `(* ,(n "zeta") 3 (sqrt (+ 2 ,(n "gamma"))))))))

;; The lips law (see lips): the opening from the lip's position, the flow
;; solved as for the reed, and the lip's own force beta (gamma - p0) from
;; the pressure before this sample's flow, as F_a (terms-with's a-end force
;; for a :split connection; it pushes the lip by -F_a).
;;   c, s, F, ya, yb, cb, w  as for sweep-law-forms
(def lips-law-forms (c s F ya yb cb w)
  (let ((n (lambda (suffix) (nm (get c :id) suffix))) (h (nm (get c :id) (str "open" s))))
    (append
      (list `(def ,h (max 0 (+ 1 (/ ,ya ,(n "rest"))))))
      (blown-forms c F yb cb w
                   (lambda (pp) `(* ,(n "zeta") ,h (sqrt (abs (- ,(n "gamma") ,pp))) (sign (- ,(n "gamma") ,pp))))
                   `(* ,(n "zeta") ,h (sqrt (+ 2 ,(n "gamma")))))
      (list `(def ,(symbol F "_a") (* -1 ,w ,(n "beta") (- ,(n "gamma") (/ ,yb ,(n "scale")))))))))

;; A connection's weight form: its weight param, or the form a hidden
;; connection is weighted by.
;;   c  a connection
(def weight-of (c) (if (get c :weight-form) (get c :weight-form) (nm (get c :id) "weight")))

;; Protocol steps 2 and 3 for every connection: each exciter in turn (reading
;; its ends with the earlier exciters' forces), then every bow's setup, then
;; the springs and bows by sweeps (4 when there are several, else 1).
;;   patch  the patch
;;   conns  its connections, resolved
(def connection-forms (patch conns)
  (let ((exciters (indices-where conns exciter?))
        (springs (indices-where conns sweeps?)))
    (append
      (concat-all
        (map (lambda (k)
               (let ((c (nth conns k)) (before (filter (lambda (j) (< j k)) exciters)))
                 (if (strokes? c) (strokes-forms c patch)
                 (if (mount? c) (mount-forms c patch)
                   (append (if (get c :a) (end-forms c "a" (find-object patch (get c :a)) (lambda (id) (terms-of conns before id)) "" true) nil)
                           (end-forms c "b" (find-object patch (get c :b)) (lambda (id) (terms-of conns before id)) "" true)
                           (exciter-forms c patch))))))
             exciters))
      (concat-all (map (lambda (k) (bow-pre-forms (nth conns k))) (indices-where conns bow?)))
      (concat-all (map (lambda (k) (driver-pre-forms (nth conns k) patch)) (indices-where conns driver?)))
      (spring-sweep-forms patch conns exciters springs (if (> (len springs) 1) 4 1))
      ;; a bow with hair: its pressure next sample is the pressure param times
      ;; the hair force over its 50 ms mean (clamped to 0..3), so the second
      ;; direction's motion modulates the friction around the set pressure
      (concat-all
        (map (lambda (c)
               (if (and (bow? c) (get c :hair))
                 (let ((n (lambda (suffix) (nm (get c :id) suffix))))
                   (list `(def ,(n "normal") (max 0 (* -1 ,(n "hair_F"))))
                         ;; seeded with the force itself at first contact, so an
                         ;; attack is not bowed at the clip
                         `(def ,(n "normal_avg")
                            (gswitch (lt (read-history ,(n "avg_h")) 0.0000000001) ,(n "normal")
                                     (+ (read-history ,(n "avg_h"))
                                        (* (- ,(n "normal") (read-history ,(n "avg_h")))
                                           (- 1 (exp (/ -20 samplerate)))))))
                         `(write-history ,(n "avg_h") ,(n "normal_avg"))
                         `(write-history ,(n "press_h")
                            (clip (/ ,(n "normal") (max ,(n "normal_avg") (* 0.5 ,(n "normal")) 0.0000000001)) 0 3))))
                 (list)))
             conns)))))

;; c with :a-lumped and :b-lumped added, whether each end's object is lumped
;; (a missing a end counts as not lumped).
;;   patch  the patch
;;   c      a connection
(def resolve-connection (patch c)
  (let ((end (lambda (id) (if id (find-object patch id) nil))))
    (merge c :a-lumped (if (get c :a) (get (end (get c :a)) :lumped) false)
             :b-lumped (get (end (get c :b)) :lumped)
             :a-mix (if (and (end (get c :a)) (= (get (end (get c :a)) :kind) :mix)) (end (get c :a)) nil)
             :b-mix (if (= (get (end (get c :b)) :kind) :mix) (end (get c :b)) nil))))

;; Every param spec of a connection: its law's own, then its ends' access
;; points.
;;   patch  the patch
;;   c      a connection
(def connection-params (patch c)
  (if (get c :internal) (list)
    (append (get c :params)
            (list (weight-param (get c :id) (get c :weight)))
            ;; a connection placed at another's point has no point params,
            ;; nor do strokes (they play every mode); a mount's string end
            ;; is fixed
            (if (or (get c :coords-of) (strokes? c)) (list)
              (append (if (get c :a) (access-params (get c :id) "a" (find-object patch (get c :a)) (get c :a-at)) nil)
                      (if (mount? c) nil (access-params (get c :id) "b" (find-object patch (get c :b)) (get c :b-at)))))
            (if (get c :signal) (tap-params patch c) (list)))))

;; A mount's forms, ending in cid_push on its string: the second difference
;; of the mount point's displacement (its acceleration over a sample
;; squared, from the last three samples' state) times each mode's share of
;; the end's motion, faded where the string's modes do not sound.
;;   c      a mount
;;   patch  the patch
(def mount-forms (c patch)
  (let ((n (lambda (suffix) (nm (get c :id) suffix)))
        (body (find-object patch (get c :a)))
        (strung (find-object patch (get c :b))))
    (append (shape-defs body (access-syms (get c :id) "a" body) (n "phi_a"))
            ;; the point's displacement the last two samples (the body's own
            ;; state), and the one before
            (list `(def ,(n "u") (sum (* (read-history ,(p body "s1")) ,(n "phi_a"))))
                  `(def ,(n "u1") (sum (* (read-history ,(p body "s2")) ,(n "phi_a"))))
                  `(make-history ,(n "u2"))
                  `(def ,(n "d2") (+ (- ,(n "u") (* 2 ,(n "u1"))) (read-history ,(n "u2"))))
                  `(write-history ,(n "u2") ,(n "u1"))
                  `(def ,(n "share") ,(tensor-form (map (lambda (k) (/ (* 2 (if (and (= (get c :end) 1) (= (mod k 2) 1)) -1 1))
                                                                       (* 3.14159265 (+ k 1))))
                                                        (range (get strung :modes)))))
                  ;; the per-mode factors together, so only d2 is per sample
                  `(def ,(n "push") (* ,(n "d2") (* -1 ,(weight-of c) ,(n "gain") ,(n "share") ,(p strung "mask"))))))))

;; ── measured strokes ────────────────────────────────────────────────────────

;; A tensor literal of values padded to two (DGenLisp mishandles one-element
;; tensors, eseq-k0m2.32), for tables read with peek.
;;   values  a list of numbers
(def peek-table-form (values) (tensor-form (if (= (len values) 1) (append values (list 0)) values)))

;; Forms for a strokes connection (see strokes), defining cid_push (what the
;; hit adds to the struck object's modes this sample, applied at its commit)
;; and cid_out (its click and wash, added to the mix). The hit's work runs on
;; the hit's event clock (event-hold), latched back to audio rate.
;;
;; A stroke's residue (re, im) is a mode's state as a phasor (im heard, re a
;; quarter cycle on). The object's two-sample state takes it as two pushes:
;; im on the hit, then s re - c im the next sample, where c and s are the
;; mode's radius times the cosine and sine of its frequency (from its
;; coefficients: c = a/2, s = sqrt(-b - c^2)).
;; The fallback contact time, click Q and octave range follow
;; tools/pm-ride/build.py (CONTACT_SECONDS, CLICK_Q, OCTAVE_RANGE).
;;   c      a strokes connection
;;   patch  the patch
(def strokes-forms (c patch)
  (let ((obj (find-object patch (get c :b)))
        (n (lambda (suffix) (nm (get c :id) suffix)))
        (o (lambda (name) (p obj name)))
        (sts (get obj :strokes))
        (modes (get obj :modes))
        (contact (if (get obj :contact-s) (get obj :contact-s) 0.00025))
        (clicks (lambda (field) (map (lambda (st) (let ((v (if (get st :click) (get (get st :click) field) nil)))
                                                    (if (number? v) v 0)))
                                     sts))))
    (append
      (list `(def ,(n "re_t") ,(table-form (map (lambda (st) (get st :re)) sts)))
            `(def ,(n "im_t") ,(table-form (map (lambda (st) (get st :im)) sts)))
            ;; the key's stroke, wrapped onto the strokes there are
            `(def ,(n "row") (- key_row (* ,(len sts) (floor (/ key_row ,(len sts))))))
            `(def ,(n "row_e") (event-hold ,(n "row") hit))
            `(def ,(n "ratio") (latch ,(o "ratio") hit))
            ;; the stick: residues are the modes' response to a two-pole
            ;; contact force of the measured time; the live one multiplies
            ;; each by the exact complex ratio, live / measured, squared for
            ;; the two poles
            `(def ,(n "hz") (* ,(o "hz") (event-hold ,(n "ratio") hit)))
            `(def ,(n "ref_pole") ,(pole-form contact))
            `(def ,(n "tau") (* ,contact (pow 2 (* 3 (- 0.5 ,(n "hardness"))))
                                (+ 1 (* 1.5 ,(n "dynamics") (- 1 hit_velocity)))))
            `(def ,(n "live_pole") ,(pole-form `(event-hold ,(n "tau") hit)))
            `(def ,(n "w") (* twopi (/ (min ,(n "hz") (* 0.48 samplerate)) samplerate))))
      (cos-sin-defs (n "w"))
      (pole-response-defs (n "ref") (n "ref_pole") (n "w"))
      (pole-response-defs (n "live") (n "live_pole") (n "w"))
      (list
            `(def ,(n "live_norm") ,(guarded (norm2-form (n "live_re") (n "live_im"))))
            `(def ,(n "q_re") (/ (+ (* ,(n "ref_re") ,(n "live_re")) (* ,(n "ref_im") ,(n "live_im"))) ,(n "live_norm")))
            `(def ,(n "q_im") (/ (- (* ,(n "ref_im") ,(n "live_re")) (* ,(n "ref_re") ,(n "live_im"))) ,(n "live_norm")))
            `(def ,(n "dc") (/ (* (- 1 ,(n "live_pole")) (- 1 ,(n "live_pole"))) (* (- 1 ,(n "ref_pole")) (- 1 ,(n "ref_pole")))))
            `(def ,(n "c_re") (* ,(n "dc") (- (* ,(n "q_re") ,(n "q_re")) (* ,(n "q_im") ,(n "q_im")))))
            `(def ,(n "c_im") (* ,(n "dc") 2 ,(n "q_re") ,(n "q_im")))
            ;; modes the transposition puts near Nyquist fade out
            `(def ,(n "band") (clip (/ (- (* samplerate 0.47) ,(n "hz")) (* samplerate 0.07)) 0 1))
            `(def ,(n "re") (* ,(table-row-form (n "re_t") modes (n "row_e")) ,(n "band")))
            `(def ,(n "im") (* ,(table-row-form (n "im_t") modes (n "row_e")) ,(n "band")))
            ;; a finite energy budget: a harder stick redistributes, never
            ;; manufactures, modal energy
            `(def ,(n "e") ,(norm2-form (n "re") (n "im")))
            `(def ,(n "energy") (min 1 (sqrt (/ (sum ,(n "e"))
                                                ,(guarded `(sum (* ,(n "e") ,(norm2-form (n "c_re") (n "c_im")))))))))
            `(def ,(n "scale") (* (event-hold hit_velocity hit) ,(n "weight") ,(n "energy")))
            ;; only the two pushes run per sample (latched); the rest stays
            ;; on the hit's clock
            `(def ,(n "dr") (event-hold (* ,(n "scale") (- (* ,(n "re") ,(n "c_re")) (* ,(n "im") ,(n "c_im")))) hit))
            `(def ,(n "di_e") (event-hold (* ,(n "scale") (+ (* ,(n "re") ,(n "c_im")) (* ,(n "im") ,(n "c_re")))) hit))
            `(def ,(n "di") (latch ,(n "di_e") hit))
            `(def ,(n "cos") (* 0.5 (event-hold ,(o "a") hit)))
            `(def ,(n "sin") (sqrt (max 0 (- (* -1 (event-hold ,(o "b") hit)) (* ,(n "cos") ,(n "cos"))))))
            `(def ,(n "p1") (latch (event-hold (- (* ,(n "sin") ,(n "dr")) (* ,(n "cos") ,(n "di_e"))) hit) hit))
            `(make-history ,(n "pend") @shape [ ,modes ])
            `(def ,(n "push") (+ (* hit ,(n "di")) (read-history ,(n "pend"))))
            `(write-history ,(n "pend") (* hit ,(n "p1")))
            ;; the click: band-passed noise falling in a millisecond or two
            `(def ,(n "click_t") ,(peek-table-form (clicks :rms)))
            `(def ,(n "click_decay_t") ,(peek-table-form (clicks :decay-s)))
            `(def ,(n "click_hz_t") ,(peek-table-form (clicks :hz)))
            `(make-history ,(n "click_h"))
            `(def ,(n "click_pole") (latch (event-hold ,(pole-form `(max (peek ,(n "click_decay_t") ,(n "row_e")) 0.0001)) hit) hit))
            `(def ,(n "click_env") (+ (* (read-history ,(n "click_h")) ,(n "click_pole"))
                                      (* hit (peek ,(n "click_t") ,(n "row")) hit_velocity ,(n "click") ,(n "weight"))))
            `(write-history ,(n "click_h") ,(n "click_env"))
            `(def ,(n "click_hz") (latch (* (max (peek ,(n "click_hz_t") ,(n "row")) 20) ,(n "ratio") (pow 2 (* 1.5 ,(n "click_tone")))) hit))
            `(def ,(n "click_out") (svf (* ,(white-noise-form) ,(n "click_env")) ,(below-nyquist (n "click_hz")) 1.5 1)))
      (if (get obj :wash) (wash-forms c obj) (list `(def ,(n "wash_out") 0)))
      (list `(def ,(n "out") (+ ,(n "click_out") ,(n "wash_out")))))))

;; Forms defining w_cos and w_sin, once for every pole-response-defs at w.
;;   w  the frequency's symbol
(def cos-sin-defs (w) (list `(def ,(symbol w "_cos") (cos ,w)) `(def ,(symbol w "_sin") (sin ,w))))

;; Forms defining name_re and name_im, the response 1 - pole e^(-iw) of a
;; one-pole contact force at w (two of them make the stick's contact).
;;   name  the symbol prefix
;;   pole  a form for the pole
;;   w     the frequency's symbol, with cos-sin-defs
(def pole-response-defs (name pole w)
  (list `(def ,(symbol name "_re") (- 1 (* ,pole ,(symbol w "_cos"))))
        `(def ,(symbol name "_im") (* ,pole ,(symbol w "_sin")))))

;; A form for re^2 + im^2.
;;   re, im  forms
(def norm2-form (re im) `(+ (* ,re ,re) (* ,im ,im)))

;; A divisor kept away from zero: event inputs read zero between events.
;;   form  a form
(def guarded (form) `(max 0.00000000000000000001 ,form))

;; A form for muffle's loss at frequencies hz: up to 40/s at 1 kHz, rising
;; with sqrt(f) (tools/pm-ride/build.py's LOSS_REFERENCE_HZ). It takes the
;; data's own frequencies, untransposed, as PM Ride Kit does, so it stays
;; param-rate work.
;;   obj  a data object
;;   hz   a form for the frequencies
(def muffle-loss-form (obj hz) `(* 40 ,(p obj "muffle") ,(p obj "muffle") (sqrt (/ ,hz 1000))))

;; Forms for a strokes connection's wash, cid_wash_out: white noise through
;; a bank of band-passes (two cascaded TPT state-variable stages per band of
;; Q 3, build.py's WASH_Q, normalised to unit output variance), each band's
;; power envelope charged by the stroke's measured power, filling in over the
;; band's measured build-up time and decaying at its measured rate (a fast
;; and a slow component). Powers add, as incoherent dense modes do. Hardness
;; reshapes it with the modes' contact ratio; Decay, Muffle and Mute act on
;; it as on the modes. The Wash param scales its power (squared, as PM Ride
;; Kit does: 2 is +12 dB).
;;   c    a strokes connection
;;   obj  its data object, with :wash
(def wash-forms (c obj)
  (let ((n (lambda (suffix) (nm (get c :id) (str "wash_" suffix))))
        (k (lambda (suffix) (nm (get c :id) suffix)))
        (o (lambda (name) (p obj name)))
        (wash (get obj :wash))
        (bands (len (get wash :hz)))
        (power (lambda (name table rates)
                 (let ((m (lambda (suffix) (n (str name "_" suffix)))))
                   (list `(def ,(m "t") ,(table-form table))
                         `(def ,(m "inject") (latch (event-hold (* ,(table-row-form (m "t") bands (k "row_e")) ,(n "vel_power")) hit) hit))
                         `(def ,(m "rate") (+ (* ,(tensor-form rates) ,(o "decay")) ,(muffle-loss-form obj (n "hz_t"))))
                         `(def ,(m "fall") (* (exp (/ (* -2 ,(m "rate")) samplerate))
                                              ,@(if (muted? obj) (list (o "g") (o "g")) (list))))
                         `(make-history ,(m "charge") @shape [ ,bands ])
                         `(def ,(m "charged") (+ (* (read-history ,(m "charge")) ,(n "settle")) (* hit ,(m "inject"))))
                         `(write-history ,(m "charge") ,(m "charged"))
                         `(make-history ,(m "power") @shape [ ,bands ])
                         `(def ,(m "now") (+ (* (read-history ,(m "power")) ,(m "fall")) (* ,(m "charged") (- 1 ,(n "settle")))))
                         `(write-history ,(m "power") ,(m "now"))))))
        (stage (lambda (j source)
                 (let ((m (lambda (suffix) (n (str "s" j "_" suffix)))))
                   (list `(make-history ,(m "1") @shape [ ,bands ])
                         `(make-history ,(m "2") @shape [ ,bands ])
                         `(def ,(m "v3") (- ,source (read-history ,(m "2"))))
                         `(def ,(m "v1") (+ (* ,(n "a1") (read-history ,(m "1"))) (* ,(n "a2") ,(m "v3"))))
                         `(def ,(m "v2") (+ (read-history ,(m "2")) (* ,(n "a2") (read-history ,(m "1"))) (* ,(n "a3") ,(m "v3"))))
                         `(write-history ,(m "1") (- (* 2 ,(m "v1")) (read-history ,(m "1"))))
                         `(write-history ,(m "2") (- (* 2 ,(m "v2")) (read-history ,(m "2"))))
                         `(def ,(m "out") (/ ,(m "v1") 3.0)))))))
    (append
      (list `(def ,(n "hz_t") ,(tensor-form (get wash :hz)))
            ;; the bank, at the object's transposition
            `(def ,(n "hz") (* ,(n "hz_t") ,(o "ratio")))
            `(def ,(n "open") (gt (* 0.45 samplerate) ,(n "hz")))
            `(def ,(n "g") (tan (* pi (/ ,(below-nyquist (n "hz")) samplerate))))
            `(def ,(n "d") (/ 1 (+ 1 (* ,(n "g") (+ ,(n "g") (/ 1 3.0))))))
            `(def ,(n "a1") (latch ,(n "d") coef_tick))
            `(def ,(n "a2") (latch (* ,(n "g") ,(n "d")) coef_tick))
            `(def ,(n "a3") (latch (* ,(n "g") ,(n "g") ,(n "d")) coef_tick))
            ;; two unit-peak band-passes have noise bandwidth (pi/4)(f/Q);
            ;; uniform noise in [-1, 1] has variance 1/3
            `(def ,(n "norm") (latch (* ,(n "open") (sqrt (/ (* 1.5 samplerate) (* 0.25 pi (/ (max ,(n "hz") 1) 3.0))))) coef_tick))
            ;; the stick's contact ratio at each band
            `(def ,(n "w") (* twopi (/ (min (* ,(n "hz_t") (event-hold ,(k "ratio") hit)) (* 0.48 samplerate)) samplerate))))
      (cos-sin-defs (n "w"))
      (pole-response-defs (n "live") (k "live_pole") (n "w"))
      (pole-response-defs (n "ref") (k "ref_pole") (n "w"))
      (list `(def ,(n "ratio") (/ ,(norm2-form (n "ref_re") (n "ref_im")) ,(guarded (norm2-form (n "live_re") (n "live_im")))))
            `(def ,(n "vel") (* (event-hold hit_velocity hit) ,(k "energy") ,(k "weight")))
            `(def ,(n "vel_power") (* ,(n "ratio") ,(n "ratio") ,(k "dc") ,(k "dc") ,(n "vel") ,(n "vel")))
            `(def ,(n "settle") (exp (/ (* -1 ,(tensor-form (get wash :buildup))) samplerate))))
      (power "fast" (get wash :fast-power) (get wash :fast-rate))
      (power "slow" (get wash :slow-power) (get wash :slow-rate))
      (list `(def ,(n "gain") (* (sqrt (+ ,(n "fast_now") ,(n "slow_now"))) ,(n "norm") ,(k "wash") ,(k "wash"))))
      (stage 1 (white-noise-form))
      (stage 2 (n "s1_out"))
      (list `(def ,(n "out") (sum (* ,(n "s2_out") ,(n "gain"))))))))

;; Every strokes or mount connection's push on object id, as forms (none
;; when nothing pushes it).
;;   conns  the resolved connections
;;   id     an object id
(def pushes-on (conns id)
  (map (lambda (c) (nm (get c :id) "push")) (filter (lambda (c) (and (pusher? c) (= (get c :b) id))) conns)))

;; What is wrong with a strokes connection: its object must be a data object
;; with :strokes whose residues have one value per mode.
;;   patch  the patch
;;   c      a strokes connection
(def strokes-errors (patch c)
  (let ((obj (find-object patch (get c :b))))
    (if (= obj nil) (list (str (what c) ": no object " (source (get c :b))))
      (if (or (not (= (get obj :kind) :data)) (not (list? (get obj :strokes))) (empty? (get obj :strokes)))
        (list (str (what c) ": " (source (get c :b)) " has no :strokes (a modal-data object with measured strokes)"))
        (append
          (concat-all
            (map (lambda (k)
                   (let ((st (nth (get obj :strokes) k)))
                     (if (and (list? (get st :re)) (list? (get st :im))
                              (= (len (get st :re)) (get obj :modes)) (= (len (get st :im)) (get obj :modes)))
                       nil
                       (list (str (what c) ": stroke " k " needs :re and :im with one value per mode ("
                                  (get obj :modes) ")")))))
                 (range (len (get obj :strokes)))))
          (if (get obj :wash) (wash-errors c (get obj :wash) (len (get obj :strokes))) nil))))))

;; What is wrong with a wash: :hz, :buildup, :fast-rate and :slow-rate need
;; one value per band, :fast-power and :slow-power one row of them per
;; stroke.
;;   c        the strokes connection
;;   wash     the wash dict
;;   strokes  how many strokes
(def wash-errors (c wash strokes)
  (let ((bands (if (list? (get wash :hz)) (len (get wash :hz)) 0))
        (sized (lambda (xs n) (and (list? xs) (= (len xs) n)))))
    (if (= bands 0) (list (str (what c) ": the wash needs :hz, one centre per band"))
      (append
        (concat-all (map (lambda (key) (if (sized (get wash key) bands) nil
                                          (list (str (what c) ": wash " (source key) " needs " bands " values"))))
                         (list :buildup :fast-rate :slow-rate)))
        (concat-all (map (lambda (key)
                           (let ((rows (get wash key)))
                             (if (and (sized rows strokes) (empty? (filter (lambda (row) (not (sized row bands))) rows))) nil
                               (list (str (what c) ": wash " (source key) " needs " strokes " rows of " bands)))))
                         (list :fast-power :slow-power)))))))

;; ── listening ───────────────────────────────────────────────────────────────

;; The id of the k-th listen point: mic1, mic2, ...
;;   k  the listen's position in the patch's listens, from 0
(def mic (k) (symbol "mic" (+ k 1)))

;; Forms defining mic<k>_v, the velocity at a listen point after this
;; sample's forces, normalised by the object's reference frequency and mass
;; so that a light string and a heavy plate mix at similar levels.
;;   patch  the patch
;;   l      the listen
;;   k      its position in the patch's listens
(def listen-forms (patch l k)
  (let ((obj (find-object patch (get l :obj))) (lid (mic k)))
    (if (= (get obj :kind) :mix) (mix-listen-forms patch obj l k)
      (hearing-forms obj l lid lid (excite-norm patch (get patch :connections))))))

;; Forms defining <s>_v, the velocity obj is heard with at listen l's point
;; (normalised by its reference frequency and mass so that a light string
;; and a heavy plate mix at similar levels).
;;   obj   the heard object
;;   l     the listen
;;   lid   the listen's id (its point params' owner)
;;   s     the symbol prefix of what is defined
;;   norm  a form for the patch's output normalisation (excite-norm), which
;;         a pickup measures its distance in
(def hearing-forms (obj l lid s norm)
  (append (shape-defs obj (access-syms lid "mic" obj) (nm s "phi"))
          (list `(make-history ,(nm s "prev"))
                ;; the param-rate factors first, so DGen can fold them once per block
                `(def ,(nm s "y") (sum (* ,(nm s "phi") ,@(hearing-gains obj l) ,(p obj "next")))))
          (if (= (get l :hear) :position) (list `(def ,(nm s "v") ,(nm s "y")))
            (if (pickup? l) (pickup-forms obj lid s norm)
              (list `(def ,(nm s "v") ,(speed-form obj s)))))
          (list `(write-history ,(nm s "prev") ,(nm s "y")))))

;; A form for the velocity heard at <s>_y, normalised by the object's
;; reference frequency and mass.
;;   obj  the heard object
;;   s    the symbol prefix of the listen's defs
(def speed-form (obj s) `(/ (* (- ,(nm s "y") (read-history ,(nm s "prev"))) ,(p obj "mass")) ,(p obj "wref")))

;; What a pickup hears, <s>_v: the normalised speed times the flux gradient
;; at the string's distance from the pole, 1 / (1 - u)^3 with u its
;; displacement over the height (normalised as the output is, so a firm
;; pluck swings about 1; kept short of the pole), through the coil's
;; resonance.
;;   obj   the heard object
;;   lid   the pickup's listen id (its params' owner)
;;   s     the symbol prefix of what is defined
;;   norm  a form for the output normalisation
(def pickup-forms (obj lid s norm)
  (list `(def ,(nm s "near") (/ 1 (- 1 (clip (* (/ (* ,(p obj "mass") ,norm) ,(nm lid "height")) ,(nm s "y")) -0.9 0.6))))
        `(def ,(nm s "v") (svf (* ,(speed-form obj s) ,(nm s "near") ,(nm s "near") ,(nm s "near"))
                               ,(below-nyquist (nm lid "coil_hz")) ,(nm lid "coil_q") 0))))

;; listen-forms on a mix hybrid: each part heard at its own point, mixed
;; (1 - morph) and morph into mic<k>_v.
;;   patch  the patch
;;   obj    the mix hybrid
;;   l      the listen
;;   k      its position in the patch's listens
(def mix-listen-forms (patch obj l k)
  (let ((lid (mic k)) (x (p obj "morph")) (parts (get obj :part-objs)))
    (append
      ;; (a mix has no pickup, so no normalisation)
      (concat-all (map (lambda (j) (hearing-forms (nth parts j) l lid (nm lid j) nil)) (range 2)))
      (list `(def ,(nm lid "v") (+ (* (- 1 ,x) ,(nm (nm lid 0) "v")) (* ,x ,(nm (nm lid 1) "v"))))))))

;; Per-mode factors on what a listen hears: the object's spectral gains and
;; the listen's own mode selection, as tensors (none when neither is given).
;;   obj  the heard object
;;   l    the listen
(def hearing-gains (obj l)
  (let ((n (get obj :modes))
        (gains (if (get obj :spectral) (get (get obj :spectral) :gains) nil))
        (picked (get l :modes)))
    (append (if gains (list (per-mode-tensor gains n 1)) (list))
            (if picked (list (indicator-tensor n picked)) (list)))))

;; A tensor of n modes, 1 at the modes in picked and 0 elsewhere.
;;   n       the mode count
;;   picked  mode numbers from 0
(def indicator-tensor (n picked)
  (tensor-form (map (lambda (k) (if (member? picked k) 1 0)) (range n))))

;; A tensor of one value per mode from a list, padded with fill.
;;   values  a list (shorter is fine)
;;   n       the mode count
;;   fill    the value for modes the list does not reach
(def per-mode-tensor (values n fill)
  (tensor-form (map (lambda (k) (if (< k (len values)) (nth values k) fill)) (range n))))

;; A form for mode 0 of a computed n-mode tensor (peek takes only stored
;; tensors): the sum against a one-hot tensor.
;;   form  the tensor form
;;   n     its mode count
(def mode0 (form n) `(sum (* ,form ,(indicator-tensor n (list 0)))))

;; The spectral pass over an object's setup forms (see spectral): its
;; frequency def becomes the raw spectrum obj_freqs0 and the warped
;; obj_freqs; its sigma takes the loss scales; its wref moves with the lowest
;; mode. Objects without spectral pass through.
;;   obj    an object
;;   forms  its setup forms
(def spectral-pass (obj forms)
  (let ((sp (get obj :spectral)) (n (get obj :modes)))
    (if (not sp) forms
      (let ((defines (lambda (f name) (and (list? f) (= (first f) 'def) (= (nth f 1) (p obj name)))))
            (f0s (p obj "freqs0"))
            (bent (p obj "freqs_b")))
        (concat-all
          (map (lambda (f)
                 (if (defines f "freqs")
                   (list `(def ,f0s ,(if (get sp :freq-scales) `(* ,(nth f 2) ,(per-mode-tensor (get sp :freq-scales) n 1)) (nth f 2)))
                         `(def ,(p obj "f1_0") ,(mode0 f0s n))
                         `(def ,bent (* ,f0s (pow 2 (/ ,(pp obj "bend") 1200))
                                        ;; ,@nil fails, so an empty splice is (list)
                                        ,@(if (get sp :follow)
                                            ;; a held object's inputs are event-held (see held-pass)
                                            (list `(/ ,(if (get obj :held) '(event-hold f0 coef_tick) 'f0) ,(p obj "f1_0")))
                                            (list))))
                         `(def ,(p obj "f1_b") ,(mode0 bent n))
                         `(def ,(p obj "freqs")
                            (max 1 (+ (* (- ,bent ,(p obj "f1_b")) (log (+ (* ,(pp obj "diffraction") 1.718281828) 1)))
                                      ,(p obj "f1_b")))))
                   (if (defines f "sigma")
                     (list (if (get sp :loss-scales)
                             `(def ,(p obj "sigma") (* ,(nth f 2) ,(per-mode-tensor (get sp :loss-scales) n 1)))
                             f))
                     (if (defines f "wref")
                       (list `(def ,(p obj "wref0") ,(nth f 2))
                             `(def ,(p obj "wref") (min (* ,(p obj "wref0") (/ ,(p obj "f1_b") ,(p obj "f1_0"))) 2.8)))
                       (list f)))))
               forms))))))

;; The k-th mic's gain param spec, mic<k>_gain.
;;   patch  the patch
;;   k      the listen's position, from 0
(def mic-gain-param (patch k)
  (pspec (mic k) "gain" (get (nth (get patch :listens) k) :gain) 0 4 :title (str (source (mic k)) " gain")))

;; The k-th mic's position param specs (see access-params).
;;   patch  the patch
;;   k      the listen's position, from 0
;;   label  what the panel titles call the point (default: the object's id)
(def mic-at-params (patch k &key (label nil))
  (let ((l (nth (get patch :listens) k)))
    (access-params (mic k) "mic" (find-object patch (get l :obj)) (get l :at) :label label)))

;; The output gain param spec, out_gain.
(def out-gain-param () (pspec 'out "gain" 0.5 0 2 :title "Gain"))

;; The host's Voicing param, out_voice_mode (bound in instrument.json; the
;; host finds it by that name, modal_patch.rs's VOICE_MODE_PARAM), when
;; the patch has :voicing: 0 poly, 1 one voice (retriggered), as the host's
;; voice modes; none otherwise.
;;   patch  the patch
(def voicing-params (patch)
  (let ((v (get patch :voicing)))
    (if (= v nil) (list) (list (pspec 'out "voice_mode" (if (= v :poly) 0 1) 0 3 :title "Voicing" :dec 0)))))

;; The output Tone param, out_tone, when the patch has :tone; none otherwise.
;;   patch  the patch
(def tone-params (patch)
  (if (get patch :tone) (list (pspec 'out "tone" (get patch :tone) 1000 20000 :unit 'Hz :title "Tone" :taper :log)) (list)))

;; A pickup's own params: its height and its coil's resonance (none for
;; other listens).
;;   patch  the patch
;;   k      the listen's position, from 0
(def pickup-params (patch k)
  (let ((l (nth (get patch :listens) k)) (id (mic k)))
    (if (pickup? l)
      (list (pspec id "height" (get l :height) 0.3 30 :title (str (source id) " height") :taper :log)
            (pspec id "coil_hz" (get l :coil-hz) 1000 10000 :unit 'Hz :title (str (source id) " coil") :taper :log)
            (pspec id "coil_q" (get l :coil-q) 0.5 6 :title (str (source id) " peak")))
      (list))))

;; Every param spec of the k-th mic: its position, its gain, then a
;; pickup's own.
;;   patch  the patch
;;   k      the listen's position, from 0
(def listen-params (patch k)
  (append (mic-at-params patch k) (list (mic-gain-param patch k)) (pickup-params patch k)))

;; A form for the output's level normalisation by what drives the patch (its
;; first pluck, felt or bow), so the output sits near the same level whatever
;; the exciter's settings: a pluck's displacement scales with its release
;; force, a mallet's with its throw speed, (sub-linearly) its mass, and falls
;; with the struck pitch; a bowed point moves at about the bow's speed.
;; 1 when nothing drives the patch.
;;   patch  the patch
;;   conns  its connections, resolved
(def excite-norm (patch conns)
  (let ((drivers (filter (lambda (c) (if (bow? c) true (if (= (get c :law) :pluck) true (= (get c :law) :felt)))) conns)))
    (if (empty? drivers) 1
      (let ((c (first drivers)))
        (if (bow? c)
          ;; velocity relative to the bow's speed
          `(* 0.3 (/ ,(nm (get c :b) "wref") ,(nm (get c :id) "vmax")))
        (if (= (get c :law) :pluck)
          `(/ 0.5 ,(nm (get c :id) "thresh"))
          ;; (wref / C3)^1.6 flattens the mallet's -9.6 dB/octave across
          ;; notes; on an object with a pitch of its own (a plate, a cymbal)
          ;; it never cuts below C3, so a low one is not muted
          (let ((b (find-object patch (get c :b))) (wref (nm (get c :b) "wref")))
            `(* (/ samplerate (* ,(nm (get c :a) "speed") 10))
                (pow (/ 0.01 ,(nm (get c :a) "mass")) 0.45)
                (pow (/ ,(if (fixed-pitch? b) `(max ,wref 0.0171) wref) 0.0171) 1.6)))))))))

;; True when obj's pitch is its own rather than the played note's (a plate
;; not made to follow the note).
;;   obj  an object
(def fixed-pitch? (obj)
  (and (get obj :fixed-pitch) (not (and (get obj :spectral) (get (get obj :spectral) :follow)))))

;; ── the amp ─────────────────────────────────────────────────────────────────
;; A guitar amplifier inside the instrument (a patch's :amp), so its sound
;; can reach back into the objects (m/speaker). Its stages, in order:
;;   input     a coupling capacitor's high-pass (80 Hz)
;;   fuzz      drive into tanh about a bias point, which makes it asymmetric
;;             (even harmonics); the input's peaks charge the stage's
;;             coupling capacitor, drifting the bias by a share of the
;;             swing (bloom), so the clipping stays lopsided however hard it
;;             is driven (a fixed offset vanishes against a big swing): the
;;             swell, sputter and octave grit of a hard-driven fuzz; near 1
;;             it gates. Antiderivative
;;             antialiased: tanh's integral, log cosh, differenced over the
;;             step, so its harmonics fold back far less at 48 kHz
;;   tone      bass, mid and treble bands added to or taken from the signal
;;   power     a second, softer tanh whose drive sags as the supply is
;;             loaded (the power-amp compression of a cranked amp)
;;   speaker   the cone's resonance (a high-pass peaking at 100 Hz), its
;;             presence peak (around 2.8 kHz, where a guitar cone's
;;             break-up rings) and the cabinet's falling top: two low-passes
;;             from cab Hz
;;   level     the speaker's loudness, the feedback's too

;; An amplifier for a patch's :amp.
;;   drive   the preamp's gain into the fuzz
;;   bias    the fuzz's asymmetry (0 symmetric)
;;   bloom   how far loud playing drifts the bias
;;   bass, mid, treble  the tone stack, 1 flat (0 .. 2)
;;   power   the power stage's drive
;;   sag     how much the supply sags under load
;;   presence  the cone's presence peak, 0 none
;;   cab     where the cabinet's top falls away, Hz
;;   level   the speaker's level
(def amp (&key (drive 20) (bias 0.2) (bloom 0.5) (bass 1) (mid 1) (treble 1) (power 2) (sag 0.3) (presence 0.5)
               (cab 5000) (level 0.5))
  (dict :kind :amp
        :params (list (pspec 'amp "drive" drive 1 500 :title "Drive" :taper :log :dec 1)
                      (pspec 'amp "bias" bias 0 0.8 :title "Bias")
                      (pspec 'amp "bloom" bloom 0 1 :title "Bloom")
                      (pspec 'amp "bass" bass 0 2 :title "Bass")
                      (pspec 'amp "mid" mid 0 2 :title "Mid")
                      (pspec 'amp "treble" treble 0 2 :title "Treble")
                      (pspec 'amp "power" power 0.5 10 :title "Power" :taper :log)
                      (pspec 'amp "sag" sag 0 1 :title "Sag")
                      (pspec 'amp "presence" presence 0 2 :title "Presence")
                      (pspec 'amp "cab" cab 1500 10000 :unit 'Hz :title "Cab" :taper :log)
                      (pspec 'amp "level" level 0 2 :title "Level"))))

;; The amp's params (none without one).
;;   patch  the patch
(def amp-params (patch) (if (get patch :amp) (get (get patch :amp) :params) (list)))

;; log cosh z without overflow: |z| + log(1 + e^-2|z|) - log 2, tanh's
;; integral.
;;   z  a form
(def log-cosh-form (z) `(- (+ (abs ,z) (log (+ 1 (exp (* -2 (abs ,z)))))) 0.6931471805599453))

;; A one-pole follower's coefficient for a time constant.
;;   seconds  the time constant, s
(def follow-coef (seconds) `(- 1 ,(pole-form seconds)))

;; The amp's forms on the signal named in, ending in amp_speaker (and its
;; history, which m/speaker reads next sample).
;;   in  the input signal's symbol
(def amp-forms (in)
  (list `(def amp_x (* (svf ,in 80 0.5 2) amp_drive))
        ;; the bias, less what loud peaks charged the coupling cap with
        '(make-history amp_charge)
        '(def amp_b (- amp_bias (* amp_bloom (read-history amp_charge))))
        '(def amp_z (+ amp_x amp_b))
        '(make-history amp_zp)
        ;; tanh's integral now and (kept) last sample
        '(make-history amp_lcp)
        `(def amp_lc ,(log-cosh-form 'amp_z))
        '(def amp_dz (- amp_z (read-history amp_zp)))
        '(def amp_far (gt (abs amp_dz) 0.00001))
        '(def amp_fuzz (- (gswitch amp_far
                                   (/ (- amp_lc (read-history amp_lcp)) (gswitch amp_far amp_dz 1))
                                   (tanh (* 0.5 (+ amp_z (read-history amp_zp)))))
                          (tanh amp_b)))
        '(write-history amp_zp amp_z)
        '(write-history amp_lcp amp_lc)
        ;; charges in ~5 ms on the input's positive peaks, leaks over ~200 ms
        `(def amp_peak (max 0 amp_x))
        `(write-history amp_charge (+ (read-history amp_charge)
                                      (* (gswitch (gt amp_peak (read-history amp_charge)) ,(follow-coef 0.005) ,(follow-coef 0.2))
                                         (- amp_peak (read-history amp_charge)))))
        ;; the next stage's coupling capacitor blocks the bias's DC
        '(def amp_t (svf amp_fuzz 30 0.5 2))
        '(def amp_tone (+ amp_t (* (- amp_bass 1) (svf amp_t 120 0.6 0))
                          (* (- amp_mid 1) (/ (svf amp_t 700 0.8 1) 0.8))
                          (* (- amp_treble 1) (svf amp_t 2200 0.6 2))))
        ;; the supply sags with the power drawn (~60 ms)
        '(make-history amp_load)
        '(def amp_pw (tanh (/ (* amp_tone amp_power) (+ 1 (* 4 amp_sag (read-history amp_load))))))
        `(write-history amp_load (+ (read-history amp_load) (* ,(follow-coef 0.06) (- (* amp_pw amp_pw) (read-history amp_load)))))
        '(def amp_cone (+ (svf amp_pw 100 1.4 2) (* amp_presence (/ (svf amp_pw 2800 1.2 1) 1.2))))
        `(def amp_speaker (* (svf (svf amp_cone (min amp_cab (* 0.4 samplerate)) 0.9 0)
                                  ,(below-nyquist '(* 1.25 amp_cab)) 0.6 0)
                             amp_level))
        `(write-history ,(speaker-history) amp_speaker)))

;; ── the whole program ───────────────────────────────────────────────────────

;; The forms every modal instrument starts with: the voice inputs (gate,
;; pitch, velocity, trigger, clock), and from them hit (1 on the sample a
;; note starts), age (seconds since it did), hit_velocity (the velocity it
;; started with) and f0 (the note's frequency, at least 20 Hz).
(def preamble ()
  (list '(def gate (in 1 @name gate))
        '(def pitch (in 2 @name pitch))
        '(def velocity (in 3 @name velocity))
        '(def trigger (in 4 @name trigger))
        '(def clock (in 5 @name clock))
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
        ;; per sample: note-following objects recompute their coefficients
        ;; every sample (hop-hold silences an instrument; eseq-k0m2.31)
        '(def f0 (max pitch 20))))

;; A (use-defmacro name) form for every protocol macro in content/defmacros
;; the generated code may call.
(def library-imports ()
  (map (lambda (name) (list 'use-defmacro name))
       '(stiff-string-freqs rect-plate rect-plate-shape circ-membrane
         modal-band modal-loss modal-coefs modal-predict modal-point modal-advance
         two-mass-coefs mallet-predict mallet-advance pluck-contact spring-connection
         felt-contact bow-contact voice-amp)))

;; Every param spec of the instrument, in declaration order: the objects',
;; the connections', the mics', then the output gain.
;;   patch  the patch
;;   conns  its connections, resolved
(def all-params (patch conns)
  (concat-all (list (concat-all (map (lambda (o) (get o :params)) (get patch :objects)))
                    (concat-all (map (lambda (c) (connection-params patch c)) conns))
                    (concat-all (map (lambda (k) (listen-params patch k)) (range (len (get patch :listens)))))
                    (amp-params patch)
                    (list (out-gain-param))
                    (tone-params patch)
                    (voicing-params patch))))

;; Every form of the instrument's dsp.lisp, in the order one sample runs:
;; preamble, params, macro imports, each object's setup and prediction, the
;; connections, each object's commit, each bow's point position for next
;; sample, the mics, then the mix, a DC blocker, tanh, and the audio and amp
;; outputs.
;;   patch  a patch that checks clean
(def patch-forms (patch)
  (let ((conns (map (lambda (c) (resolve-connection patch c)) (get patch :connections)))
        (tapped (tapped-connections patch))
        (objects (get patch :objects))
        (listens (get patch :listens)))
    (concat-all
      (list (preamble)
            (map param-form (all-params patch conns))
            (library-imports)
            (if (keyed? patch) (key-forms) (list))
            (if (or (any-held? patch) (any? strokes? conns)) (coef-tick-forms patch) (list))
            (concat-all (map (lambda (o) (held-pass o (spectral-pass o (object-setup o)))) objects))
            (concat-all (map object-predict (filter physical? objects)))
            ;; the force histories feedback taps read, and the speaker's
            (map (lambda (id) `(make-history ,(nm id "F_h"))) tapped)
            (if (get patch :amp) (list `(make-history ,(speaker-history))) (list))
            (connection-forms patch conns)
            (map (lambda (id) `(write-history ,(nm id "F_h") ,(nm id "F"))) tapped)
            (concat-all (map (lambda (o) (object-commit o (terms-of conns (range (len conns)) (get o :id))
                                                        (pushes-on conns (get o :id))))
                             (filter physical? objects)))
            ;; a bow needs its point's true position after every force
            (concat-all (map (lambda (c) (if (bow? c)
                                           (list `(write-history ,(nm (get c :id) "y_prev")
                                                    (sum (* ,(p (find-object patch (get c :b)) "next") ,(nm (get c :id) "phi_b")))))
                                           nil))
                             conns))
            (concat-all (map (lambda (k) (listen-forms patch (nth listens k) k)) (range (len listens))))
            (list `(def mix ,(sum-form (append (map (lambda (k) `(* ,(nm (mic k) "v") ,(nm (mic k) "gain")))
                                                    (range (len listens)))
                                               ;; strokes' click and wash
                                               (map (lambda (c) (nm (get c :id) "out")) (filter strokes? conns))))))
            ;; the mix at its level, through the amp when there is one; then
            ;; the output gain and the tone filter
            (let ((norm (excite-norm patch conns)))
              (let ((sound (if (get patch :amp) '(* amp_speaker out_gain) `(* mix ,norm ,(get patch :level) out_gain))))
                (append (if (get patch :amp) (cons `(def heard (* mix ,norm ,(get patch :level))) (amp-forms 'heard)) (list))
                        (list `(def raw ,(if (get patch :tone) `(svf ,sound (min out_tone (* samplerate 0.43)) 0.707 0) sound))))))
            (list '(make-history dcx)
                  '(make-history dcy)
                  '(def dc-out (+ (- raw (read-history dcx)) (* 0.9995 (read-history dcy))))
                  '(write-history dcx raw)
                  '(write-history dcy dc-out)
                  '(def voice_out (tanh dc-out))
                  '(out voice_out 1 @name audio)
                  '(out (voice-amp (max (gt gate 0.5) (lt age 0.25)) voice_out voice_out) 2 @name amp @amp true))))))

;; The instrument's dsp.lisp source, pretty-printed.
;;   patch  a patch that checks clean
(def dsp (patch)
  (str "; generated by eseq.modal; edit the patch script, not this file\n"
       (pp-forms (patch-forms patch))))

;; ── panel: ui.lisp in the style of .local/instruments/modalys-pluck ───────
;; One section per object and connection, then OUTPUT (the mics and the
;; gain). Each section gets a panel with its first three params as knobs;
;; the screen in the middle draws the patch (click an object or connection
;; to select it) and lists every param of the selected section.
;; A section's number (k below) is its position in (sections patch).

;; The param specs of the OUTPUT section: every mic's gain, the output gain,
;; then where each mic listens (titled by mic, "mic1 at").
;;   patch  the patch
(def output-params (patch)
  (let ((ks (range (len (get patch :listens)))))
    (append (map (lambda (k) (mic-gain-param patch k)) ks)
            (list (out-gain-param))
            (tone-params patch)
            (voicing-params patch)
            (concat-all (map (lambda (k) (mic-at-params patch k :label (source (mic k)))) ks))
            (concat-all (map (lambda (k) (pickup-params patch k)) ks)))))

;; The panel's sections, in order: each object, each connection, OUTPUT.
;; Each is (dict :title :tab :params): the panel heading ("STRING str"), the
;; screen tab's label, and its param specs.
;;   patch  the patch
(def sections (patch)
  (let ((conns (map (lambda (c) (resolve-connection patch c)) (shown-connections patch))))
    (append (map (lambda (o) (dict :title (str (get o :title) " " (source (get o :id))) :tab (source (get o :id))
                                   :params (get o :params)))
                 (shown-objects patch))
            (map (lambda (c) (dict :title (str (get c :title) " " (source (get c :id))) :tab (source (get c :id))
                                   :params (connection-params patch c)))
                 conns)
            (if (get patch :amp) (list (dict :title "AMP" :tab "amp" :params (amp-params patch))) (list))
            (list (dict :title "OUTPUT" :tab "out" :params (output-params patch))))))

;; The section number of the object or connection called id.
;;   patch  the patch
;;   id     an object or connection id
(def section-of (patch id)
  (let ((ids (append (map (lambda (o) (get o :id)) (shown-objects patch))
                     (map (lambda (c) (get c :id)) (shown-connections patch))))
        (shown (lambda (id)
                 (let ((o (find-object patch id))
                       (c (first (filter (lambda (c) (= (get c :id) id)) (get patch :connections)))))
                   (if (and o (get o :plane-of)) (get o :plane-of)
                     (if (and c (get c :owner)) (get c :owner) id))))))
    (index-of ids (shown id))))

;; The objects and connections with panel sections: not second directions
;; (their params are the first's) nor hidden connections.
;;   patch  the patch
(def shown-objects (patch) (filter (lambda (o) (not (get o :plane-of))) (get patch :objects)))
(def shown-connections (patch) (filter (lambda (c) (not (get c :internal))) (get patch :connections)))

;; The first n items of xs (all of them when there are fewer).
;;   xs  a list
;;   n   how many
(def take (xs n) (map (lambda (i) (nth xs i)) (range (min n (len xs)))))

;; The definitions every generated panel starts with, as forms. They are
;; named mx-* to keep them apart from the user's own code when they edit the
;; result. In them, section is a section number, name a param's host name
;; (see host-name), title a label, and width a size in panel units.
;;   mx-c        the panel's one accent colour
;;   mx-section  the selected section's number
;;   mx-p        the host's param record for name
;;   mx-select   a callback selecting section
;;   mx-knob     a knob (decimals shown, :linear / :log taper)
;;   mx-label    a panel's heading strip
;;   mx-panel    a section's panel holding body; clicking it selects section
;;   mx-option   a dropdown for a menu param (options: its option strings)
;;   mx-num      a number field for the screen's param page
;;   mx-chip     an object in the screen's patch drawing
;;   mx-link     a connection or mic in the drawing
;;   mx-tab      a tab under the screen selecting section
(def ui-helpers ()
  '((def mx-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
    (def mx-section () eseq.vanilla/custom-ui-selected-section)
    (def mx-p (name) (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
    (def mx-select (section) (eseq.effects.custom-ui-sections/ui-section-select-callback section))
    (def mx-knob (section name title decimals taper)
      (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title 5.6 3.45 2.15
        (mx-c) decimals taper :widget-knob-track 8.5 8 :center))
    (def mx-label (title width)
      (box :width width :height 0.72 :background-color (mx-c)
        (label title :width width :height 0.72 :h-align :center :v-align :center
          :font-size 8.2 :color :black :bg :transparent)))
    (def mx-panel (section title width body)
      (box :width width :height 4.8 :padding 0.18
        :background-color (if (= (mx-section) section) :instrument-panel-bg :instrument-group-bg)
        :debug-name (str "modal-panel-" section)
        :on-click (mx-select section)
        (v-stack :gap 0.14 (mx-label title (min width 9)) body)))
    (def mx-option (section name title options)
      (let ((p (mx-p name)) (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "modal-opt-mod-" name)
          (box :width 6.6 :height 3.45 :v-align :center
            (v-stack :width 6.6 :height 1.4 :gap 0.12
              (label title :width 6.6 :height 0.5 :h-align :center :v-align :center
                :font-size 8.5 :bg :transparent)
              (dropdown :debug-name (str "modal-" name) :width 6.6 :height 0.7 :font-size 7.2
                :options options
                :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
                :text-color (mx-c) :chevron-color (mx-c) :bg-color :instrument-control-bg :border-color :transparent
                :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
                :on-change (lambda (v)
                  (do
                    (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                    (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                      (eseq.effects.param-controls/custom-ui-option-index options v))))))))))
    (def mx-num (section name title width decimals)
      (let ((p (mx-p name))
            (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white :black)))
        (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "modal-mod-" name)
          (subtree :key (str "modal-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
              (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
            (v-stack :width width :height 1.15 :gap 0.08
              (label title :height 0.5 :v-align :center :font-size 9.5 :color ink :bg :transparent)
              (number-picker :width width :height 0.55 :noui true :font-size 10 :decimals decimals
                :unit (eseq.effects.custom-ui-runtime/custom-ui-param-control-unit p)
                :step (pow 10 (- 0 decimals))
                :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
                :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
                :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
                :text-align :left
                :text-color ink :edit-color ink :cursor-color ink
                :plock-style :underline
                :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
                :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)))))))
    ;; the screen's patch drawing: objects as chips, connections as links
    (def mx-chip (section title)
      (button title :width (+ 1.4 (* 0.62 (len title))) :height 0.8 :padding 0 :font-size 8.5
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :black
        :background-color (if (= (mx-section) section) :black (mx-c))
        :on-click (mx-select section)))
    (def mx-link (section title)
      (button title :width (+ 1 (* 0.55 (len title))) :height 0.8 :padding 0 :font-size 7.5
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :transparent
        :background-color (if (= (mx-section) section) :black :transparent)
        :on-click (mx-select section)))
    (def mx-tab (section title width)
      (button title :width width :height 0.8 :padding 0 :font-size 8
        :color (if (= (mx-section) section) (mx-c) :black)
        :border-color :transparent
        :background-color (if (= (mx-section) section) :black (mx-c))
        :on-click (mx-select section)))))

;; The name the host knows a grouped param by, group.name: "str.str_length".
;;   p  a param spec
(def host-name (p) (str (source (get p :group)) "." (source (get p :sym))))

;; A panel control form for a param: a dropdown for a menu, else a knob.
;;   k  the section number
;;   p  a param spec
(def control-form (k p)
  (if (get p :options)
    `(mx-option ,k ,(host-name p) ,(get p :title) (quote ,(get p :options)))
    `(mx-knob ,k ,(host-name p) ,(get p :title) ,(get p :dec) ,(get p :taper))))

;; A number field form for a param on the screen's page.
;;   k      the section number
;;   p      a param spec
;;   width  the field's width
(def num-form (k p width)
  `(mx-num ,k ,(host-name p) ,(get p :title) ,width ,(get p :dec)))

;; A section's panel form, holding controls for its first params.
;;   k      the section number
;;   sec    the section
;;   knobs  how many params get a control
;;   width  the panel's width
(def panel-form (k sec knobs width)
  `(mx-panel ,k ,(get sec :title) ,width
     (h-stack :gap 0.15 ,@(map (lambda (p) (control-form k p)) (take (get sec :params) knobs)))))

;; One line of the drawing for a connection: a -law id-> b (a bow has no a).
;; Clicking either object or the link selects its section.
;;   patch  the patch
;;   c      the connection
(def link-row (patch c)
  (let ((k (section-of patch (get c :id))) (law (law-name c)))
    `(h-stack :gap 0.1 :align :center
       ,@(if (get c :a)
           (list `(mx-chip ,(section-of patch (get c :a)) ,(source (get c :a))))
           (list))
       (mx-link ,k ,(str "-" law " " (source (get c :id)) "->"))
       (mx-chip ,(section-of patch (get c :b)) ,(source (get c :b))))))

;; The drawing's lines for the mics, two to a line: "mic1 @ str".
;;   patch  the patch
;;   out    the OUTPUT section's number, which clicking a mic selects
(def mic-rows (patch out)
  (map (lambda (ks)
         `(h-stack :gap 0.1 :align :center
            ,@(concat-all
                (map (lambda (k)
                       (let ((l (nth (get patch :listens) k)))
                         (list `(mx-link ,out ,(str (source (mic k)) " @"))
                               `(mx-chip ,(section-of patch (get l :obj)) ,(source (get l :obj))))))
                     ks))))
       (chunks (range (len (get patch :listens))) 2)))

;; The screen's patch drawing: a line per connection, then the mics, in two
;; columns once there are more than four lines.
;;   patch  the patch
;;   out    the OUTPUT section's number
(def drawing-form (patch out)
  (let ((rows (append (map (lambda (c) (link-row patch c)) (shown-connections patch))
                      (mic-rows patch out))))
    (if (> (len rows) 4)
      `(h-stack :gap 0.6 :align :start
         ,@(map (lambda (col) `(v-stack :gap 0.12 ,@col)) (chunks rows (ceil (/ (len rows) 2)))))
      `(v-stack :gap 0.12 ,@rows))))

;; The screen's page for one section: a number field for every param, six to
;; a row. Menu params are left out; their dropdown is on the panel.
;;   k    the section number
;;   sec  the section
(def page-form (k sec)
  `(v-stack :gap 0.12
     ,@(map (lambda (row) `(h-stack :gap 0.35 ,@(map (lambda (p) (num-form k p 5.3)) row)))
            (chunks (filter (lambda (p) (not (get p :options))) (get sec :params)) 6))))

;; A form showing the selected section's page: nested ifs over the sections
;; from k on, the last one shown when none before it is selected.
;;   secs  the sections
;;   k     the first section number to test
(def pages-from (secs k)
  (if (= k (- (len secs) 1))
    (page-form k (nth secs k))
    `(if (= (mx-section) ,k) ,(page-form k (nth secs k)) ,(pages-from secs (+ k 1)))))

;; The screen form: a title naming the patch and the selected section, the
;; patch drawing, the selected section's page, and a tab per section.
;;   patch  the patch
;;   secs   its sections
(def display-form (patch secs)
  (let ((out (- (len secs) 1))
        (tab-w (/ (- 35.2 (* 0.18 (- (len secs) 1))) (len secs))))
    `(box :debug-name "modal-display" :part "instrument/screen" :width 35.8 :height 9.8 :padding 0.3
          :background-color (mx-c)
       (v-stack :gap 0.15
         (label (str ,(str (get patch :name) " / ") (nth (quote ,(map (lambda (s) (get s :title)) secs)) (mx-section)))
           :width 35.2 :height 0.7 :h-align :center :v-align :center :font-size 8 :color (mx-c) :bg :black)
         (box :width 35.2 :height 4.1 ,(drawing-form patch out))
         (box :width 35.2 :height 2.6 ,(pages-from secs 0))
         (h-stack :gap 0.18
           ,@(map (lambda (k) `(mx-tab ,k ,(get (nth secs k) :tab) ,tab-w)) (range (len secs))))))))

;; Every form of the instrument's ui.lisp: the mx-* helpers, then the
;; defsynth-ui. Objects' panels go to the left of the screen, connections'
;; and OUTPUT's to the right, two to a column; past three columns the panels
;; narrow to two knobs.
;;   patch  a patch that checks clean
(def ui-forms (patch)
  (let ((secs (sections patch))
        (meshes (mesh-views patch))
        (n-obj (len (shown-objects patch)))
        (columns (+ (ceil (/ n-obj 2)) (ceil (/ (- (len secs) n-obj) 2))))
        (knobs (if (> columns 3) 2 3))
        (width (if (> columns 3) 12.6 18.7))
        (panels (lambda (from to)
                  (map (lambda (pair) `(v-stack :gap 0.2 ,@pair))
                       (chunks (map (lambda (k) (panel-form k (nth secs k) knobs width)) (range from to)) 2)))))
    (append (if (empty? meshes) (list) (list '(import eseq.modal.mesh-view)))
            (ui-helpers)
            (list `(defsynth-ui
                     (h-stack :height 9.8 :gap 0.3 :align :start
                       ,@(panels 0 n-obj)
                       ,(display-form patch secs)
                       ,@(panels n-obj (len secs))
                       ,@meshes))))))

;; A 3-D view per object that has a mesh (a solid, or data from an
;; eseq-modes file), moving through its lowest modes (mode-mesh-cycle);
;; drag it to orbit.
;;   patch  the patch
(def mesh-views (patch)
  (map (lambda (o)
         `(box :width 18.7 :height 9.8 :padding 0.2 :background-color (mx-c)
               (v-stack :gap 0.14
                 (mx-label ,(str (get o :title) " " (source (get o :id))) 9)
                 (eseq.modal.mesh-view/mode-mesh-cycle :src ,(get o :mesh-file) :width 18.3 :height 8.4
                                                      :key ,(str "modal-mesh-" (source (get o :id)))))))
       (filter (lambda (o) (string? (get o :mesh-file))) (get patch :objects))))

;; The instrument's ui.lisp source, pretty-printed.
;;   patch  a patch that checks clean
(def ui (patch)
  (str ";; generated by eseq.modal; edit the patch script, not this file\n"
       (pp-forms (ui-forms patch))))

;; What a host runs on a script's patch: (dict :name :dsp :ui), or
;; (dict :errors messages) when check finds problems. eseq modal emit and
;; the app's C-c C-c drafts both call this.
;;   patch  the patch a script returned
(def generate (patch)
  (let ((errors (check patch)))
    (if (empty? errors)
      (dict :name (get patch :name) :dsp (dsp patch) :ui (ui patch))
      (dict :errors errors))))
