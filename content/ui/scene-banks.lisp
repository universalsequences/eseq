;; ui/scene-banks.lisp — Shared scene-bank view state.
;;
;; The viewed scene bank is pure presentation state (scene-banks spec §4):
;; switching it never calls the host. It lives here rather than in
;; ui/transport.lisp because two render roots read it — the transport scene
;; strip and the mixer's per-track clip grid (spec §10.1) — and ui/main.lisp
;; loads ui/mixer.lisp BEFORE ui/transport.lisp. Both roots import this module
;; and share one bank view.
;;
;; State/accessor hub only: no `effect-buffer` here, so importing it from a
;; render root is safe.

(module eseq.scene-banks)
(import eseq.kinds :refer (banks transport))
(import eseq.view-kit :as kit)

(export scene-bank-view
        scene-viewed-bank
        scene-viewed-bank-index
        view-scene-bank!
        view-new-scene-bank!
        clip-in-viewed-bank?
        listed?)

;; The bank the strip shows (a bank instance), the index it last had, and
;; another bank listed beside it. A shown bank no longer listed falls back
;; (scene-banks spec §4):
;; - a structural edit (delete bank, undo) removes it but not `other`: the
;;   bank now at its last index, clamped (the previous one when it was last);
;; - a project load replaces every bank instance, `other` too, and nil is the
;;   first view: the bank of the playing scene.
;; `pending` is the bank count when "New bank" was picked (-1: none):
;; create-scene-bank appends, and the view lands on the new bank once the
;; host lists it.
(def-kind scene-bank-view
  :key ()
  :state ((bank bank :default nil)
          (index -1)
          (other bank :default nil)
          (pending -1)))

;; COMPAT(eseq-0l17): eseq.view-kit/listed?, re-exported for callers
;; that still refer it from here.
(def listed? kit/listed?)

(def view-scene-bank! (b)
  (let ((other (first (filter (lambda (x) (not (= x b))) (banks))))
        (index (if b b.index -1)))
    (unless (= scene-bank-view.pending -1) (set! scene-bank-view.pending -1))
    (unless (= scene-bank-view.bank b) (set! scene-bank-view.bank b))
    (unless (= scene-bank-view.other other) (set! scene-bank-view.other other))
    (unless (= scene-bank-view.index index) (set! scene-bank-view.index index))
    b))

;; Show the bank create-scene-bank is about to append.
(def view-new-scene-bank! ()
  (set! scene-bank-view.pending (len (banks))))

(def playing-bank (all)
  (if transport.scene transport.scene.bank (first all)))

;; Where a shown bank that is no longer listed falls back to.
(def fallback-bank (all)
  (if (and (>= scene-bank-view.index 0) (listed? scene-bank-view.other all))
    (nth all (min scene-bank-view.index (- (len all) 1)))
    (playing-bank all)))

;; The shown bank, or nil before the host has published any.
(def scene-viewed-bank ()
  (let ((all (banks))
        (pending scene-bank-view.pending)
        (b scene-bank-view.bank))
    (if (>= pending 0)
      (if (< pending (len all))
        (view-scene-bank! (nth all pending))
        ;; The host has not published the appended bank yet: show the last.
        (nth all (- (len all) 1)))
      (if (listed? b all)
        (do
          ;; Keep the fallback current: b's index and a bank beside it.
          (unless (and (= scene-bank-view.index b.index)
                       (or (= (len all) 1) (listed? scene-bank-view.other all)))
            (view-scene-bank! b))
          b)
        (view-scene-bank! (fallback-bank all))))))

(def scene-viewed-bank-index ()
  (let ((b (scene-viewed-bank)))
    (if b b.index 0)))

;; Clip-grid membership (spec §10.1): cell c (a track's pattern) belongs to
;; the viewed bank `viewed` (a bank instance, (scene-viewed-bank), read once
;; per render) when a scene of that bank uses it. A cell no scene uses yet
;; (`c.banks` empty: a freshly cloned clip, a clip whose only scene was
;; deleted) stays visible in every bank so it is never stranded behind a bank
;; the user cannot guess.
(def clip-in-viewed-bank? (c viewed)
  (let ((in c.banks))
    (or (= (len in) 0) (listed? viewed in))))
