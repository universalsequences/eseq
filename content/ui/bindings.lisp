;; eseq.bindings — Lisp-owned reactive bindings.
;;
;; A widget prop bound to a reactive float updates with NO re-render: the
;; renderer reads the slot each frame. The host publishes plenty of these
;; (SEQ.*), but a UI's own state — a cursor, a selection, a follow target —
;; is just as hot, and reading it as plain `defstate` re-renders every widget
;; that looked at it. This module makes the fast path the obvious one:
;;
;;   (def B (eseq.bindings/scope "alez.tracker"))          ; once per package
;;   (def cursor-rows (eseq.bindings/channel B "cursor-rows"))
;;
;;   ;; in a widget: no reactive dependency, just a slot
;;   (box :selected (eseq.bindings/bound-nth cursor-rows row) …)
;;   (scroll :center-row (eseq.bindings/bound center-row) …)
;;
;;   ;; in a handler: write, and only the bound widgets repaint
;;   (eseq.bindings/one-hot! cursor-rows (pattern-rows) row)
;;   (eseq.bindings/write! center-row 12)
;;
;; Channels live in the Lisp-writable SEQV namespace under "<scope>/<name>",
;; so packages never collide. List channels pad with zeros to the longest
;; length ever written to them: a slot past the end of a shorter list would
;; otherwise keep its old value, which is the classic "the last cursor is
;; still lit" bug. `clear!` is "all zeros".
;;
;; Bindings carry floats (bools read as 0/1). Text still needs a re-render.

(module eseq.bindings)

(export scope channel field bound bound-nth value write! write-list! one-hot! clear!)

(def namespace "SEQV")

(def scope (name)
  (dict :prefix name))

(def channel (scope name)
  (dict :field (str (get scope :prefix) "/" name)))

(def field (ch)
  (get ch :field))

;; ── reads for widgets (slots, not dependencies) ────────────────────────────

(def bound (ch)
  (bind namespace (field ch)))

(def bound-nth (ch i)
  (bind-nth namespace (field ch) i))

;; A plain read, for handlers (this one IS a reactive dependency if used in
;; a widget body).
(def value (ch)
  (reactive-get namespace (field ch)))

;; ── writes ──────────────────────────────────────────────────────────────────

(def write! (ch v)
  (reactive-set namespace (field ch) v))

;; Longest list length written per field, as an assoc list of (field len).
(defstate written-lengths (list))

(def written-length (ch)
  (let ((hits (filter |e| (= (nth e 0) (field ch)) written-lengths)))
    (if (= (len hits) 0) 0 (nth (nth hits 0) 1))))

(def remember-length! (ch n)
  (if (> n (written-length ch))
    (set! written-lengths
      (append (filter |e| (not (= (nth e 0) (field ch))) written-lengths)
              (list (list (field ch) n))))
    nil))

(def pad-zeros (items n)
  (if (>= (len items) n) items
    (append items (map |_| 0 (range 0 (- n (len items)))))))

(def write-list! (ch items)
  (do
    (remember-length! ch (len items))
    (reactive-set namespace (field ch) (pad-zeros items (written-length ch)))))

(def one-hot! (ch n i)
  (write-list! ch (map |j| (if (= j i) 1 0) (range 0 n))))

(def clear! (ch)
  (write-list! ch (list)))
