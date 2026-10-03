;; alez.jaki.chords — declared harmony for jaki (docs/harmony-declaration-spec.md).
;;
;; One row declares the chord, the others relate to it:
;;
;;   (jak "band" :16
;;     ((fig (-) (slow 8))
;;      -> chords (note (seq :fig 0 5 10 3)) (chord :min7))  ; Cm7 Fm7 Bb7 Ebm7
;;     (. . - .
;;      -> 0 (note (deg 1)) (note+ -24)                      ; roots
;;      -> 1 (voice)                                         ; nearest free
;;      -> 2 (voice)))                                       ;   chord tones
;;
;; A `-> chords` row plays no track: each of its hits sets the instance's
;; current chord (root = the hit's final note, quality = its (chord Q)).
;; Chord rows play first in every tick, so the other rows read the new chord
;; on the same beat. The chord lives in generator state cells (numbers),
;; one field per name ("chords" unless `-> (chords "name")`).
;;
;; Everything here runs at emit, on the scheduler VM (state-get / state-set!).

(module alez.jaki.chords)

(export qualities quality-names root-names chord-tones chord-name
        chord-code encode-raw decode-code parse-root key-modes
        chord-words numeral-words menu-roots
        deg-interval field-get field-set! field-code decode-name
        chord-notes voice-pick voice-reset)

;; ── vocabulary ──────────────────────────────────────────────────────────────
;; Each quality: its tones (intervals from the root, root first) and a scale
;; whose even steps are those tones, so (deg n) reads chord tones on odd n
;; and fills the rest from the scale.

(def ionian '(0 2 4 5 7 9 11))
(def dorian '(0 2 3 5 7 9 10))
(def mixolydian '(0 2 4 5 7 9 10))
(def aeolian '(0 2 3 5 7 8 10))
(def locrian '(0 1 3 5 6 8 10))
(def melodic-minor '(0 2 3 5 7 9 11))
(def whole-tone '(0 2 4 6 8 10))
(def octatonic '(0 2 3 5 6 8 9 11))
(def phrygian-dominant '(0 1 4 5 7 8 10))
(def altered '(0 1 3 4 6 8 10))

;; (keyword display tones scale), in menu order
(def qualities
  (list (list :maj "" '(0 4 7) ionian)
        (list :min "m" '(0 3 7) aeolian)
        (list :dom7 "7" '(0 4 7 10) mixolydian)
        (list :maj7 "maj7" '(0 4 7 11) ionian)
        (list :min7 "m7" '(0 3 7 10) dorian)
        (list :hdim7 "m7b5" '(0 3 6 10) locrian)
        (list :dim "dim" '(0 3 6) locrian)
        (list :dim7 "dim7" '(0 3 6 9) octatonic)
        (list :aug "aug" '(0 4 8) whole-tone)
        (list :sus2 "sus2" '(0 2 7) ionian)
        (list :sus4 "sus4" '(0 5 7) mixolydian)
        (list :dom7sus4 "7sus4" '(0 5 7 10) mixolydian)
        (list :maj6 "6" '(0 4 7 9) ionian)
        (list :min6 "m6" '(0 3 7 9) dorian)
        (list :minmaj7 "m(maj7)" '(0 3 7 11) melodic-minor)
        (list :add9 "add9" '(0 4 7 14) ionian)
        (list :minadd9 "madd9" '(0 3 7 14) aeolian)
        (list :dom9 "9" '(0 4 7 10 14) mixolydian)
        (list :maj9 "maj9" '(0 4 7 11 14) ionian)
        (list :min9 "m9" '(0 3 7 10 14) dorian)
        (list :dom11 "11" '(0 4 7 10 14 17) mixolydian)
        (list :min11 "m11" '(0 3 7 10 14 17) dorian)
        (list :dom13 "13" '(0 4 7 10 14 17 21) mixolydian)
        (list :dom7b9 "7b9" '(0 4 7 10 13) phrygian-dominant)
        (list :dom7s9 "7#9" '(0 4 7 10 15) altered)
        (list :power "5" '(0 7) ionian)))

(def quality-names (map (lambda (q) (first q)) qualities))
(def root-names (list "C" "Db" "D" "Eb" "E" "F" "Gb" "G" "Ab" "A" "Bb" "B"))

(def index-of (xs x i)
  (if (>= i (len xs)) -1 (if (= (nth xs i) x) i (index-of xs x (+ i 1)))))

;; the quality's entry; an unknown name reads as :maj
(def quality (q)
  (let ((i (index-of quality-names q 0))) (nth qualities (if (< i 0) 0 i))))

(def chord-tones (q) (nth (quality q) 2))
(def chord-scale (q) (nth (quality q) 3))

(def pc (n) (imod (round-int n) 12))
(def chord-name (root q) (str (nth root-names (pc root)) (nth (quality q) 1)))

;; ── chord words: lead-sheet symbols and Roman numerals ─────────────────────
;; A chords lane names chords as single words, `Am7`, `C#m7b5`, `Bb7`, `E`,
;; or as Roman numerals against the lane's (key …): `i`, `iv7`, `V7`,
;; `bVII`, `vii°`, `iiø7`, `V7/iv`. Numerals follow the chromatic convention:
;; degrees count from the key's tonic on the major scale and a borrowed chord
;; carries its accidental (in A minor: i iv V7 bVI bVII), so the key's mode
;; never changes a root. Case gives the triad (upper major, lower minor).
;;
;; At route build each word becomes a number, so jaki's value rules (per
;; cycle lists, seq clocks, on / every) carry chords unchanged:
;;   10000 + root·64 + quality   an absolute chord
;;   20000 + interval·64 + quality   a numeral, rooted on the key at emit

(def letter-pcs (list (list "C" 0) (list "D" 2) (list "E" 4) (list "F" 5)
                      (list "G" 7) (list "A" 9) (list "B" 11)))

(def lookup (table key)
  (reduce (lambda (acc e) (if (and (= acc nil) (= (first e) key)) (nth e 1) acc)) nil table))

(def text (w) (if (string? w) w (source w)))

;; chord-symbol suffix → quality (the part after the root)
(def symbol-suffixes
  (list (list "" :maj) (list "maj" :maj) (list "M" :maj)
        (list "m" :min) (list "min" :min) (list "-" :min)
        (list "7" :dom7) (list "maj7" :maj7) (list "M7" :maj7) (list "Δ" :maj7) (list "Δ7" :maj7)
        (list "m7" :min7) (list "min7" :min7) (list "-7" :min7)
        (list "m7b5" :hdim7) (list "ø" :hdim7) (list "ø7" :hdim7)
        (list "dim" :dim) (list "°" :dim) (list "o" :dim)
        (list "dim7" :dim7) (list "°7" :dim7) (list "o7" :dim7)
        (list "aug" :aug) (list "+" :aug)
        (list "sus2" :sus2) (list "sus4" :sus4) (list "sus" :sus4)
        (list "7sus4" :dom7sus4) (list "7sus" :dom7sus4)
        (list "6" :maj6) (list "m6" :min6)
        (list "mmaj7" :minmaj7) (list "mM7" :minmaj7) (list "m(maj7)" :minmaj7)
        (list "add9" :add9) (list "madd9" :minadd9)
        (list "9" :dom9) (list "maj9" :maj9) (list "m9" :min9)
        (list "11" :dom11) (list "m11" :min11) (list "13" :dom13)
        (list "7b9" :dom7b9) (list "7#9" :dom7s9) (list "5" :power)))

;; a lowercase numeral's suffix reads minor-first: iv7 is m7, iv9 m9
(def minor-suffixes
  (list (list "" :min) (list "7" :min7) (list "9" :min9) (list "6" :min6)
        (list "11" :min11) (list "add9" :minadd9) (list "maj7" :minmaj7)
        (list "°" :dim) (list "o" :dim) (list "°7" :dim7) (list "o7" :dim7)
        (list "ø" :hdim7) (list "ø7" :hdim7) (list "m7b5" :hdim7)))

;; "C#…" → (pc rest) or nil
(def split-root (t)
  (if (< (len t) 1)
      nil
      (let ((pc (lookup letter-pcs (substring t 0 1))))
        (if (= pc nil)
            nil
            (let ((acc (if (> (len t) 1) (substring t 1 2) "")))
              (if (= acc "#")
                  (list (imod (+ pc 1) 12) (substring t 2))
                  (if (= acc "b")
                      (list (imod (- pc 1) 12) (substring t 2))
                      (list pc (substring t 1)))))))))

(def parse-root (w) (let ((r (split-root (text w)))) (if (= r nil) nil (first r))))

;; "Am7" → (root quality) or nil
(def parse-symbol (t)
  (let ((r (split-root t)))
    (if (= r nil)
        nil
        (let ((q (lookup symbol-suffixes (nth r 1))))
          (if (= q nil) nil (list (first r) q))))))

(def numerals
  (list (list "VII" 11 true) (list "III" 4 true) (list "VI" 9 true) (list "IV" 5 true)
        (list "II" 2 true) (list "V" 7 true) (list "I" 0 true)
        (list "vii" 11 false) (list "iii" 4 false) (list "vi" 9 false) (list "iv" 5 false)
        (list "ii" 2 false) (list "v" 7 false) (list "i" 0 false)))

;; the longest numeral at the start of t → (interval upper? rest) or nil
(def split-numeral (t)
  (reduce (lambda (acc n)
            (if (and (= acc nil) (>= (len t) (len (first n)))
                     (= (substring t 0 (len (first n))) (first n)))
                (list (nth n 1) (nth n 2) (substring t (len (first n))))
                acc))
          nil numerals))

;; "bVII7" (no slash) → (interval quality) or nil
(def parse-numeral-part (t)
  (let ((acc (if (> (len t) 0) (substring t 0 1) "")))
    (let ((shift (if (= acc "b") -1 (if (= acc "#") 1 0))))
      (let ((n (split-numeral (if (= shift 0) t (substring t 1)))))
        (if (= n nil)
            nil
            (let ((q (if (nth n 1)
                         (lookup symbol-suffixes (nth n 2))
                         (lookup minor-suffixes (nth n 2)))))
              (if (= q nil) nil (list (imod (+ (first n) shift) 12) q))))))))

(def slash-at (t i) (if (>= i (len t)) -1 (if (= (substring t i (+ i 1)) "/") i (slash-at t (+ i 1)))))

;; "V7/iv" → the V7 of iv's root: intervals add
(def parse-numeral (t)
  (let ((i (slash-at t 0)))
    (if (< i 0)
        (parse-numeral-part t)
        (let ((a (parse-numeral-part (substring t 0 i)))
              (b (parse-numeral-part (substring t (+ i 1)))))
          (if (or (= a nil) (= b nil)) nil (list (imod (+ (first a) (first b)) 12) (nth a 1)))))))

(def quality-index (q) (+ 1 (max 0 (index-of quality-names q 0))))

;; a chord word → its code, or nil when the word is not a chord
(def chord-code (w)
  (if (or (number? w) (= w nil))
      nil
      (let ((t (text w)))
        (let ((s (parse-symbol t)))
          (if (not (= s nil))
              (+ 10000 (+ (* (first s) 64) (quality-index (nth s 1))))
              (let ((n (parse-numeral t)))
                (if (= n nil) nil (+ 20000 (+ (* (first n) 64) (quality-index (nth n 1)))))))))))

;; every chord word in raw value data → its code; clock words, seq / cyc
;; heads and numbers stay
(def encode-raw (raw)
  (if (or (= raw nil) (number? raw) (string? raw))
      raw
      (if (= (nth raw 0) nil)
          (let ((c (chord-code raw))) (if (= c nil) raw c))
          (map encode-raw raw))))

;; a code against the key's tonic → (root-pc quality), or nil for a non-code
(def decode-code (code key-root)
  (if (or (not (number? code)) (< code 10000))
      nil
      (let ((rel (>= code 20000)) (body (imod (round-int code) 10000)))
        (let ((pc (idiv body 64)) (qi (imod body 64)))
          (list (if rel (imod (+ pc key-root) 12) pc)
                (nth quality-names (max 0 (- qi 1))))))))

(def key-modes (list ":major" ":minor" ":dorian" ":phrygian" ":lydian" ":mixolydian"
                     ":locrian" ":harmonic-minor" ":melodic-minor"))

;; menu words for the chords lane: every root with the common suffixes, and
;; the numerals with theirs (typed words outside these still parse)
(def menu-suffixes (list "" "m" "7" "maj7" "m7" "m7b5" "dim" "dim7" "aug" "sus2" "sus4"
                         "7sus4" "6" "m6" "9" "maj9" "m9" "add9" "11" "13" "7b9" "7#9" "5"))
(def menu-roots (list "C" "C#" "D" "Eb" "E" "F" "F#" "G" "Ab" "A" "Bb" "B"))
(def chord-words
  (reduce (lambda (acc r) (append acc (map (lambda (s) (str r s)) menu-suffixes))) (list) menu-roots))

(def numeral-words
  (append
    (reduce (lambda (acc n) (append acc (map (lambda (s) (str n s)) (list "" "7" "maj7" "9" "6" "sus4"))))
            (list) (list "I" "II" "III" "IV" "V" "VI" "VII" "bII" "bIII" "bVI" "bVII"))
    (reduce (lambda (acc n) (append acc (map (lambda (s) (str n s)) (list "" "7" "9" "6" "°" "°7" "ø7"))))
            (list) (list "i" "ii" "iii" "iv" "v" "vi" "vii"))
    (list "V/ii" "V/iii" "V/IV" "V/V" "V/vi" "V7/ii" "V7/iii" "V7/IV" "V7/V" "V7/vi")))

;; ── degrees ─────────────────────────────────────────────────────────────────
;; (deg n), counted from 1 like a scale: odd degrees are chord tones while the
;; chord has them (1 root, 3 third, 5 fifth, 7 seventh, 9 …), every other
;; degree comes from the quality's scale, octaves continuing past the 7th.
(def deg-interval (q n)
  (let ((k (- (round-int n) 1)) (tones (chord-tones q)) (scale (chord-scale q)))
    (if (and (>= k 0) (= (imod k 2) 0) (< (idiv k 2) (len tones)))
        (nth tones (idiv k 2))
        (let ((l (len scale)))
          (+ (nth scale (imod k l)) (* 12 (idiv k l)))))))

;; ── the field: one current chord per name, in generator state cells ────────

(def cell (name part) (str "jf:" name ":" part))

;; (dict :root n :q quality) or nil before anything declared
(def field-get (name)
  (let ((qi (state-get (cell name "q") 0)))
    (if (<= qi 0)
        nil
        (dict :root (state-get (cell name "root") 0)
              :q (nth quality-names (- (round-int qi) 1))))))

(def field-set! (name root q)
  (do (state-set! (cell name "root") root)
      (state-set! (cell name "q") (+ 1 (max 0 (index-of quality-names q 0))))
      nil))

;; the chord as one number for a UI mark: (root + 60) * 64 + quality index + 1
(def field-code (root q)
  (+ (* (+ (round-int root) 60) 64) (+ 1 (max 0 (index-of quality-names q 0)))))

(def decode-name (code)
  (if (or (not (number? code)) (<= code 0))
      ""
      (let ((qi (imod (round-int code) 64)) (root (- (idiv (round-int code) 64) 60)))
        (if (<= qi 0) "" (chord-name root (nth quality-names (- qi 1)))))))

;; ── bundles: the notes of a chord on one hit ───────────────────────────────
;; root + each tone, the lowest `inv` tones raised an octave (inversion)
(def chord-notes (root q inv)
  (let ((tones (chord-tones q)) (k (max 0 (round-int inv))))
    (map (lambda (i)
           (+ root (+ (nth tones i) (if (< i (imod k (max 1 (len tones)))) 12 0))))
         (range 0 (len tones)))))

;; ── voices: nearest free chord tone, voice-led ─────────────────────────────
;; A (voice) row moves to the chord tone nearest its own last note (its
;; authored note the first time), skipping pitch classes an earlier voice
;; claimed this tick, so N voice rows spread over the chord. With every tone
;; taken it doubles the nearest. Claims reset each tick.

(def claims-for (tick)
  (if (= (state-get "jv:tick" -1) tick)
      (state-get "jv:mask" 0)
      (do (state-set! "jv:tick" tick) (state-set! "jv:mask" 0) 0)))

(def bit? (mask i) (= 1 (imod (idiv mask (pow2 i)) 2)))
(def pow2 (i) (if (<= i 0) 1 (* 2 (pow2 (- i 1)))))

;; candidates: every chord-tone pitch within an octave of `from`
(def tone-pitches (root q from)
  (let ((pcs (map (lambda (t) (pc (+ root t))) (chord-tones q))))
    (reduce (lambda (acc p) (if (member? (pc p) pcs) (append acc (list p)) acc))
            (list) (range (- (round-int from) 12) (+ (round-int from) 13)))))

(def member? (x l) (reduce (lambda (acc i) (or acc (= i x))) false l))

;; Voice leading with register gravity: the tone closest to where the voice
;; was (smooth motion), with half that weight pulling toward its home note
;; (the row's own note), so a voice that leads downward chord after chord
;; drifts back instead of sinking (plain nearest-tone leading has no memory
;; of register: I IV V7 II walks a voice down a fifth per loop). Ties go to
;; home.
(def lead-cost (p from home) (+ (abs (- p from)) (* 0.5 (abs (- p home)))))

(def lead-to (from home ps)
  (reduce (lambda (best p)
            (if (or (= best nil)
                    (< (lead-cost p from home) (lead-cost best from home))
                    (and (= (lead-cost p from home) (lead-cost best from home))
                         (< (abs (- p home)) (abs (- best home)))))
                p
                best))
          nil ps))

(def nearest (from ps)
  (reduce (lambda (best p)
            (if (or (= best nil) (< (abs (- p from)) (abs (- best from)))) p best))
          nil ps))

;; the note row `key` voices to now; `authored` its own note
;; the note row `key` voices to now; `authored` its own note. `offset`
;; steps that many chord tones from the voice-led note (1 the tone above, -1
;; the one below): the led note stays the voice's memory, so an offset never
;; drifts, and two rows on one track sit a fixed chord-step apart.
(def voice-pick (key authored field tick offset)
  (if (= field nil)
      authored
      (let ((from (if (= (state-get (str "jv:" key ":has") 0) 1)
                      (state-get (str "jv:" key ":last") authored)
                      authored))
            (mask (claims-for tick)))
        (let ((ps (tone-pitches (get field :root) (get field :q) from)))
          (let ((free (reduce (lambda (acc p) (if (bit? mask (pc p)) acc (append acc (list p))))
                              (list) ps)))
            (let ((p (or-nil (lead-to from authored (if (empty? free) ps free)) authored)))
              (let ((out (step-tones p (get field :root) (get field :q) (round-int offset))))
                (do (state-set! (str "jv:" key ":last") p)
                    (state-set! (str "jv:" key ":has") 1)
                    (if (bit? mask (pc out)) nil (state-set! "jv:mask" (+ mask (pow2 (pc out)))))
                    out))))))))

;; the chord tone n steps from chord-tone pitch p (n may be negative)
(def step-tones (p root q n)
  (if (= n 0)
      p
      (let ((pcs (map (lambda (t) (pc (+ root t))) (chord-tones q))))
        (let ((ladder (reduce (lambda (acc x) (if (member? (pc x) pcs) (append acc (list x)) acc))
                              (list) (range (- p 48) (+ p 49)))))
          (let ((i (index-of ladder p 0)))
            (if (< i 0) p (nth ladder (max 0 (min (- (len ladder) 1) (+ i n))))))))))

(def or-nil (v d) (if (= v nil) d v))

(def voice-reset (key) (state-set! (str "jv:" key ":has") 0))

(def imod (a n) (- a (* n (floor (/ a n)))))
(def idiv (a b) (floor (/ a b)))
(def round-int (x) (floor (+ x 0.5)))
