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
(import eseq.kinds :refer (banks tracks transport))
(import eseq.bank-power :refer (kit-bank-state))
(import eseq.view-kit :as kit)

(export scene-bank-view
        scene-viewed-bank
        scene-viewed-bank-index
        scene-viewed-bank-id
        scene-viewed-bank-label
        view-scene-bank!
        view-new-scene-bank!
        clip-in-viewed-bank?
        show-all-tracks
        toggle-show-all-tracks!
        kit-bank-view
        track-used-in-bank?
        group-used-in-bank?
        used-in-viewed-bank?
        in-banks?
        bank-tracks)

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
  (if (and (>= scene-bank-view.index 0) (kit/listed? scene-bank-view.other all))
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
      (if (kit/listed? b all)
        (do
          ;; Keep the fallback current: b's index and a bank beside it.
          (unless (and (= scene-bank-view.index b.index)
                       (or (= (len all) 1) (kit/listed? scene-bank-view.other all)))
            (view-scene-bank! b))
          b)
        (view-scene-bank! (fallback-bank all))))))

(def scene-viewed-bank-index ()
  (let ((b (scene-viewed-bank)))
    (if b b.index 0)))

;; The viewed bank's stable id, the :bank-id an "in this bank" host command
;; takes (docs/rig-spec.md §4); 0 before the host publishes any bank, which
;; the host reads as the playing scene's bank.
(def scene-viewed-bank-id ()
  (let ((b (scene-viewed-bank)))
    (if b b.bid 0)))

;; The viewed bank's label (A, or A — its name); "" before any is published.
(def scene-viewed-bank-label ()
  (let ((b (scene-viewed-bank)))
    (if b b.label "")))

;; Whether bank `viewed` is in bank list `in`, an empty list counting as
;; every bank: a cell or track no scene uses yet stays visible in every bank,
;; so it is never stranded behind a bank the user cannot guess.
(def in-banks? (in viewed)
  (or (= (len in) 0) (kit/listed? viewed in)))

;; Clip-grid membership (spec §10.1): cell c (a track's pattern) belongs to
;; the viewed bank `viewed` (a bank instance, (scene-viewed-bank), read once
;; per render) when a scene of that bank uses it, or no scene does
;; (`c.banks` empty: a freshly cloned clip, a clip whose only scene was
;; deleted).
(def clip-in-viewed-bank? (c viewed)
  (in-banks? c.banks viewed))

;; ── Bank-scoped display (docs/rig-spec.md §9, eseq-ka5t.10) ─────────────
;; The sequencer track list, the mixer and the sequencers' route menus show
;; only what the viewed bank uses: the host publishes the banks using each
;; track and kit (`t.used-banks`, `g.used-banks`, recomputed when the scenes
;; change), so a membership test here is a list lookup. Hidden tracks keep
;; their indices. Pass the viewed bank (scene-viewed-bank, read once per
;; render) to the `-in-bank?` forms inside loops; a nil bank (none
;; published yet) shows everything.
;;
;; A track no scene of any bank uses (a fresh track, every cell cleared)
;; shows in every bank (in-banks?). "Show all tracks" shows everything; a
;; kit the bank switches off still draws as its dimmed mixer strip there
;; (docs/rig-spec.md §6).
(defcustom show-all-tracks false
  :type :bool
  :doc "Show every track in the sequencer, the mixer and the sequencers' route menus, not only the tracks the viewed scene bank uses.")

(def toggle-show-all-tracks! ()
  (set! show-all-tracks (not show-all-tracks)))

;; Kit (drum rack) g in bank `viewed`: :shown when the bank plays it, :off
;; when every scene of the bank switches it off (its dimmed mixer strip
;; stays; the sequencer grid hides it, pads and header), else :hidden. A kit
;; no bank plays shows, with one exception: a kit parked in every scene
;; reads :off in every bank (its `used-banks` is empty because it is parked,
;; not because it is new), so its pads hide everywhere and only its dimmed
;; strip offers the way back.
(def kit-bank-view (g viewed)
  (let ((in g.used-banks))
    (if (or show-all-tracks (not viewed) (kit/listed? viewed in))
      :shown
      (if (= (kit-bank-state g viewed) :off)
        :off
        (if (= (len in) 0) :shown :hidden)))))

;; Whether track t shows in bank `viewed`. A kit's pads follow the kit, all
;; or none (the host publishes the kit's banks on each pad as well).
(def track-used-in-bank? (t viewed)
  (let ((g t.group))
    (if (and g g.rack)
      (= (kit-bank-view g viewed) :shown)
      (or show-all-tracks (not viewed) (in-banks? t.used-banks viewed)))))

;; Whether group g shows in bank `viewed`: a kit unless :hidden; a plain
;; group (no used-banks of its own) while a member or child kit shows,
;; members no bank uses included (an empty group always).
(def group-used-in-bank? (g viewed)
  (if g.rack
    (not (= (kit-bank-view g viewed) :hidden))
    (or show-all-tracks (not viewed)
        (and (= (len g.tracks) 0) (= (len g.racks) 0))
        (reduce |found t| (or found (track-used-in-bank? t viewed)) false g.tracks)
        (reduce |found r| (or found (group-used-in-bank? r viewed)) false g.racks))))

;; Whether track t shows in the viewed bank.
(def used-in-viewed-bank? (t)
  (track-used-in-bank? t (scene-viewed-bank)))

;; The tracks bank `viewed` shows, in track order, plus those of `keep` (a
;; list) wherever they sit: the route menus keep the tracks their controls
;; name now, so the menu can show them.
(def bank-tracks (viewed keep)
  (filter (lambda (t) (or (kit/listed? t keep) (track-used-in-bank? t viewed))) (tracks)))
