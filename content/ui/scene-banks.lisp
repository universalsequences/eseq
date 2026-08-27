;; ui/scene-banks.lisp — Shared scene-bank view state.
;;
;; The viewed scene bank is pure presentation state (scene-banks spec §4):
;; switching it never calls the host. It lives here rather than in
;; ui/transport.lisp because two render roots read it — the transport scene
;; strip and the mixer's per-track clip grid (spec §10.1) — and ui/main.lisp
;; loads ui/mixer.lisp BEFORE ui/transport.lisp, so a transport-owned
;; `defstate` would not exist when the mixer's readers compile. Both roots
;; import this module and share one bank view.
;;
;; State/accessor hub only: no `effect-buffer` here, so importing it from a
;; render root is safe.

(module eseq.scene-banks)
;; Compile-time edge (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile.
(import eseq.seq-core-state)

(export scene-banks
        scene-bank-index-containing
        viewed-scene-bank-index
        viewed-scene-bank-pending-new
        scene-viewed-bank-index
        scene-viewed-bank
        clip-in-viewed-bank?)

;; UI source evaluation can precede the first song-state publication. Keep the
;; strip renderable during that interval; the reactive SEQ.scene-banks read
;; replaces this model-consistent single-bank value as soon as sync arrives.
(def scene-banks ()
  (if (> (len SEQ.scene-banks) 0)
    SEQ.scene-banks
    (list (dict :id 0 :label "A" :name nil :len SEQ.num-patterns :offset 0))))

(def scene-bank-index-containing (scene)
  (let ((banks (scene-banks)))
    (let ((matches (filter
            (lambda (i)
              (let ((bank (nth banks i)))
                (and (>= scene (get bank :offset))
                  (< scene (+ (get bank :offset) (get bank :len))))))
            (range 0 (len banks)))))
      (if (> (len matches) 0) (nth matches 0) 0))))

;; Pure presentation state: switching this index never calls the host. The -1
;; sentinel initializes the first rendered view to the bank containing the
;; current scene. Structural edits clamp a stale index to the nearest survivor.
(defstate viewed-scene-bank-index -1)
(defstate viewed-scene-bank-pending-new false)

(def scene-viewed-bank-index ()
  (let ((count (len (scene-banks))))
    (if (= count 0)
      0
      (if viewed-scene-bank-pending-new
        (if (< viewed-scene-bank-index count)
          (do
            (set! viewed-scene-bank-pending-new false)
            viewed-scene-bank-index)
          ;; The host has not published the appended bank yet. Keep the pending
          ;; index intact while rendering the old last bank in the meantime.
          (- count 1))
        (let ((index (if (< viewed-scene-bank-index 0)
                (scene-bank-index-containing SEQ.current-pattern)
                (min viewed-scene-bank-index (- count 1)))))
          (do
            (if (not (= viewed-scene-bank-index index))
              (set! viewed-scene-bank-index index)
              nil)
            index))))))

(def scene-viewed-bank ()
  (nth (scene-banks) (scene-viewed-bank-index)))

;; Clip-grid membership (spec §10.1). `:banks` is the host-published list of
;; bank indices whose scenes reference this clip on its track. An empty list
;; means no scene in any bank references it: those orphans (a freshly cloned
;; clip, a clip whose only scene was deleted) stay visible in every bank so
;; they are never stranded behind a bank the user cannot guess.
(def clip-in-viewed-bank? (cell)
  (let ((banks (or (get cell :banks) (list)))
        (viewed (scene-viewed-bank-index)))
    (if (= (len banks) 0)
      true
      (> (len (filter (lambda (index) (= index viewed)) banks)) 0))))
