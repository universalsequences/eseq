;; Step-grid pointer/cursor/selection interactions: paging, drag gestures, drum-step gestures, step param helpers.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; This is the step-gesture hub: ui/bus-grid.lisp (still vanilla), ui/step-grid.lisp,
;; ui/sequencer.lisp, ui/seq-grid-mode.lisp, ui/seqv-track-params.lisp and several
;; Rust call sites reach its names by their flat spellings, so it converts with NO
;; renames and a full set of *identity* compat aliases (the seq-core-state /
;; custom-ui-lego precedent, spec §10 wave-7 addendum): an unconverted vanilla
;; caller matches the alias key flat, and a converted module's bare reference
;; qualifies against itself, misses, and lands on the same alias by base name.
;; Every aliased name here is a function or a `defstate`, both immune to hazard (m).
;;
;; TWO exceptions, both hazard (m)/(i):
;;
;;   1. The eleven drag-state globals below are *mutable plain defs* that
;;      ui/bus-grid.lisp — a headerless vanilla file — reads AND `set!`s by flat
;;      spelling for the bus-lane gestures (they are genuinely one shared gesture
;;      state: only one drag runs at a time). A compat alias cannot rescue that:
;;      the late-binding heal repairs an *empty* slot and the next `set!` unlinks
;;      it. They are pinned into `eseq.vanilla` with the §3 escape hatch, get no
;;      alias, and EVERY in-file reference must use the `eseq.vanilla/` spelling —
;;      a bare one here would intern this module's own slot, a different cell, and
;;      the divergence is silent. They fold back in when bus-grid.lisp converts.
;;   2. `sequencer-cursor-step-changed` is a stub-then-override protocol:
;;      ui/sequencer.lisp pins its own `(def eseq.vanilla/sequencer-cursor-step-changed …)`
;;      override (see its comment at :304) which must land on the same slot the
;;      stub and the call site below use.
;;
;; Reads of ui/seq-core-state.lisp (a converted module) are written with its FULL
;; dotted module spelling rather than the `core/` import alias, deliberately:
;; `crates/sequencer/src/ui/state_values/tests.rs` evals two *slices* of this file
;; (`load_step_gesture_source`, `load_keyboard_step_selection_source`) with no
;; import in scope, and the compiler treats an undotted unknown alias as a hard
;; error while a dotted module namespace only warns — the qualified slot then
;; heals onto those harnesses' flat natives (function slots, so heal-safe).
;; `param-mode` stays bare: it is a `defstate`, which resolves through
;; `state_bindings` and never touches the global ladder.
(module eseq.step-grid-interactions)

(import eseq.seq-core-state :as core)


(def page-button-width 2.8)

(def %page-button-gap 0.4)

(def page-slot-width ()
  (+ page-button-width %page-button-gap))

;; Private: no caller anywhere in the app (ui/bus-grid.lisp has its own
;; `bus-page-panel-width`); kept as the track-grid twin of that helper.
(def %page-panel-width ()
  (+ 0.4 (* (eseq.seq-core-state/page-count) (page-slot-width))))

(def step-index (i)
  (+ (eseq.seq-core-state/page-offset) i))

(def step-visible? (i)
  (< (step-index i) SEQ.tp-num-steps))

(def cursor-left ()
  (if (seq-has-selection?)
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set! eseq.vanilla/step-key-select-anchor nil)
      (if (eseq.seq-core-state/seq-has-selected-bus?)
        (eseq.bus-grid/bus-shift-selected-steps -1)
        (seq-shift-selected-steps -1)))
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set! eseq.vanilla/step-key-select-anchor nil)
      (let ((num-steps (max 1 (eseq.seq-core-state/cursor-num-steps))))
        (set-track-cursor-step
          (if (= (eseq.seq-core-state/current-step) 0)
            (- num-steps 1)
            (- (eseq.seq-core-state/current-step) 1)))))))

(def cursor-right ()
  (if (seq-has-selection?)
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set! eseq.vanilla/step-key-select-anchor nil)
      (if (eseq.seq-core-state/seq-has-selected-bus?)
        (eseq.bus-grid/bus-shift-selected-steps 1)
        (seq-shift-selected-steps 1)))
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set! eseq.vanilla/step-key-select-anchor nil)
      (let ((num-steps (max 1 (eseq.seq-core-state/cursor-num-steps))))
        (set-track-cursor-step
          (if (>= (eseq.seq-core-state/current-step) (- num-steps 1))
            0
            (+ (eseq.seq-core-state/current-step) 1)))))))

;; PINNED (hazard m): ui/bus-grid.lisp `set!`s this flat for the bus-lane
;; gestures. See the file header.
(def eseq.vanilla/step-key-select-anchor nil)

(def %cursor-select-step-range (start end)
  (if (eseq.seq-core-state/seq-has-selected-bus?)
    (eseq.bus-grid/bus-select-step-range start end)
    (seq-select-step-range start end)))

(def %cursor-select-move (direction)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (let ((num-steps (max 1 (eseq.seq-core-state/cursor-num-steps)))
          (start (eseq.seq-core-state/current-step)))
      (let ((anchor (if (= eseq.vanilla/step-key-select-anchor nil) start eseq.vanilla/step-key-select-anchor))
            (next (if (< direction 0)
                    (if (= start 0) 0 (- start 1))
                    (if (>= start (- num-steps 1)) (- num-steps 1) (+ start 1)))))
        (do
          (set! eseq.vanilla/step-key-select-anchor anchor)
          (set-track-cursor-step next)
          (%cursor-select-step-range anchor next))))))

(def cursor-select-left ()
  (%cursor-select-move -1))

(def cursor-select-right ()
  (%cursor-select-move 1))

(def cursor-toggle ()
  (do
    (eseq.seq-core-state/cool-off-follow)
    (set! eseq.vanilla/step-key-select-anchor nil)
    (if (eseq.seq-core-state/seq-has-selected-bus?)
      (eseq.bus-grid/bus-toggle-step (eseq.bus-grid/bus-current-step))
      (seq-toggle-step (eseq.seq-core-state/current-step)))))

(def selection-click? (evt)
  (or (get evt :shift)
    (get evt :cmd)
    (get evt :super)
    (get evt :meta)
    (get evt :ctrl)))

(def cmd-click? (evt)
  (or (get evt :cmd)
    (get evt :super)
    (get evt :meta)
    (get evt :ctrl)))

;; PINNED (hazard i, lisp→lisp stub-then-override): ui/sequencer.lisp installs
;; the real implementation over this slot with its own
;; `(def eseq.vanilla/sequencer-cursor-step-changed …)` at :313, and the call
;; below must reach that override — a module-local slot never would.
(def eseq.vanilla/sequencer-cursor-step-changed (track step)
  nil)

(def set-track-cursor-step (step)
  (do
    (eseq.seq-core-state/set-cursor-step-value step)
    (eseq.vanilla/sequencer-cursor-step-changed SEQ.current-track step)))

;; PINNED (hazard m): the shared drag-gesture state. ui/bus-grid.lisp reads and
;; `set!`s all eleven flat. See the file header.
(def eseq.vanilla/step-drag-anchor nil)
(def eseq.vanilla/step-click-pending nil)
(def eseq.vanilla/step-move-last nil)

;; Owner-side setter for the three drag-state globals above, so a *module* can
;; reset them (module spec §10 hazard m).  A converted module's bare
;; `(set! step-click-pending nil)` interns its own `<module>/step-click-pending`
;; slot and never reaches these, silently leaving a stale click pending; routing
;; the write through this function keeps it on the pinned vanilla slot.
;; ui/sequencer.lisp's grid pointer-down handlers are the callers.
(def step-clear-drag-state ()
  (do
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-drag-anchor nil)
    (set! eseq.vanilla/step-move-last nil)))
(def eseq.vanilla/step-toggle-drag-value nil)
(def eseq.vanilla/step-click-was-active nil)
(def eseq.vanilla/step-press-ms nil)
(def eseq.vanilla/step-press-step nil)
(def eseq.vanilla/step-drag-progressed nil)
(def eseq.vanilla/step-hold-select nil)
(def eseq.vanilla/step-cmd-drag-last nil)

; Finder-style shift extension: anchor at the prior keyboard anchor if any,
; else the cursor step when a selection exists, else the clicked step.
; `cursor-step` is pinned to eseq.vanilla by ui/seq-core-state.lisp.
(def step-shift-anchor (step)
  (if (not (= eseq.vanilla/step-key-select-anchor nil))
    eseq.vanilla/step-key-select-anchor
    (if (seq-has-selection?) eseq.vanilla/cursor-step step)))

(def %step-hold-select-ms 300)

; Press-and-hold (~300ms) before dragging turns the drag into a selection
; sweep instead of a move/paint. Once a move or paint has already advanced
; past the pressed step, the hold can no longer engage.
(def step-hold-select-maybe-engage (step evt)
  (if (and (not eseq.vanilla/step-hold-select)
        (not (selection-click? evt))
        (not eseq.vanilla/step-drag-progressed)
        (not (= eseq.vanilla/step-press-ms nil))
        (>= (- (now-ms) eseq.vanilla/step-press-ms) %step-hold-select-ms))
    (do
      (set! eseq.vanilla/step-hold-select true)
      (set! eseq.vanilla/step-click-pending nil)
      (set! eseq.vanilla/step-move-last nil)
      (set! eseq.vanilla/step-toggle-drag-value nil)
      (set! eseq.vanilla/step-drag-anchor (if (= eseq.vanilla/step-press-step nil) step eseq.vanilla/step-press-step)))
    nil))

(def step-selected? (step)
  (seq-step-selected? step))

(def step-select-drag-start (step evt)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-press-ms nil)
    (set! eseq.vanilla/step-press-step nil)
    (set! eseq.vanilla/step-drag-progressed nil)
    (set! eseq.vanilla/step-hold-select nil)
    (if (cmd-click? evt)
      (do
        (set! eseq.vanilla/step-key-select-anchor nil)
        (set-track-cursor-step step)
        (set! eseq.vanilla/step-drag-anchor nil)
        (set! eseq.vanilla/step-cmd-drag-last step)
        (seq-select-step step))
      (let ((anchor (step-shift-anchor step)))
        (do
          (set! eseq.vanilla/step-key-select-anchor anchor)
          (set-track-cursor-step step)
          (set! eseq.vanilla/step-drag-anchor anchor)
          (set! eseq.vanilla/step-cmd-drag-last nil)
          (seq-select-step-range anchor step))))))

(def %step-set-cursor-if (update-cursor step)
  (if update-cursor
    (set-track-cursor-step step)
    nil))

(def %step-select-drag-over-for-track-with-cursor (track step evt update-cursor)
  (do
    (step-hold-select-maybe-engage step evt)
    (if (or (selection-click? evt) eseq.vanilla/step-hold-select)
      (do
        (set! eseq.vanilla/step-click-pending nil)
        (set! eseq.vanilla/step-move-last nil)
        (set! eseq.vanilla/step-toggle-drag-value nil)
        (eseq.seq-core-state/cool-off-follow)
        (%step-set-cursor-if update-cursor step)
        (if (and (cmd-click? evt) (not eseq.vanilla/step-hold-select))
          (if (= step eseq.vanilla/step-cmd-drag-last)
            nil
            (do
              (set! eseq.vanilla/step-cmd-drag-last step)
              (if (step-selected? step) nil (seq-select-step step))))
          (do
            (if (= eseq.vanilla/step-drag-anchor nil) (set! eseq.vanilla/step-drag-anchor step) nil)
            (seq-select-step-range eseq.vanilla/step-drag-anchor step))))
      (do
        (if (= step eseq.vanilla/step-press-step) nil (set! eseq.vanilla/step-drag-progressed true))
        (if (not (= eseq.vanilla/step-toggle-drag-value nil))
          (do
            (set! eseq.vanilla/step-click-pending nil)
            (eseq.seq-core-state/cool-off-follow)
            (%step-set-cursor-if update-cursor step)
            (if (= (seq-track-step-active? track step) eseq.vanilla/step-toggle-drag-value)
              nil
              (seq-toggle-step step)))
          (if (= eseq.vanilla/step-move-last nil)
            nil
            (if (= step eseq.vanilla/step-move-last)
              nil
              (do
                (set! eseq.vanilla/step-click-pending nil)
                (eseq.seq-core-state/cool-off-follow)
                (seq-move-step-drag eseq.vanilla/step-move-last step)
                (set! eseq.vanilla/step-move-last step)
                (%step-set-cursor-if update-cursor step)))))))))

(def step-select-drag-over-for-track (track step evt)
  (%step-select-drag-over-for-track-with-cursor track step evt true))

(def step-select-drag-over-for-track-no-cursor (track step evt)
  (%step-select-drag-over-for-track-with-cursor track step evt false))

(def step-select-drag-over (step evt)
  (step-select-drag-over-for-track SEQ.current-track step evt))

(def step-pointer-down-for-track (track step evt use-selection)
  (if (selection-click? evt)
    (step-select-drag-start step evt)
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set-track-cursor-step step)
      (set! eseq.vanilla/step-drag-anchor nil)
      (set! eseq.vanilla/step-press-ms (now-ms))
      (set! eseq.vanilla/step-press-step step)
      (set! eseq.vanilla/step-drag-progressed nil)
      (set! eseq.vanilla/step-hold-select nil)
      (if (or (seq-track-step-active? track step) (and use-selection (step-selected? step)))
        (do
          (set! eseq.vanilla/step-move-last step)
          (set! eseq.vanilla/step-click-pending step)
          (set! eseq.vanilla/step-click-was-active (seq-track-step-active? track step))
          (set! eseq.vanilla/step-toggle-drag-value nil))
        (do
          (set! eseq.vanilla/step-move-last nil)
          (set! eseq.vanilla/step-click-pending nil)
          (set! eseq.vanilla/step-toggle-drag-value true)
          (step-select-drag-over-for-track track step evt))))))

(def step-pointer-down (step evt)
  (step-pointer-down-for-track SEQ.current-track step evt true))

(def step-pointer-up (step evt)
  (do
    (if (and (= eseq.vanilla/step-click-pending step) (not (selection-click? evt)))
      (if eseq.vanilla/step-click-was-active
        (seq-select-step-range step step)
        (seq-toggle-step step))
      nil)
    (set! eseq.vanilla/step-click-was-active nil)
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-drag-anchor nil)
    (set! eseq.vanilla/step-move-last nil)
    (set! eseq.vanilla/step-toggle-drag-value nil)
    (set! eseq.vanilla/step-press-ms nil)
    (set! eseq.vanilla/step-press-step nil)
    (set! eseq.vanilla/step-drag-progressed nil)
    (set! eseq.vanilla/step-hold-select nil)
    (set! eseq.vanilla/step-cmd-drag-last nil)))

(def step-double-click-for-track (track step evt)
  (if (and (not (selection-click? evt)) (seq-track-step-active? track step))
    (seq-toggle-step step)
    nil))

(def step-double-click (step evt)
  (step-double-click-for-track SEQ.current-track step evt))

(defstate drum-step-gesture-track nil)
(defstate drum-step-gesture-pad nil)
(defstate drum-step-cursor-track nil)
(defstate drum-step-cursor-pad nil)

(def %drum-step-gesture-lane? (track pad-note)
  (and (= drum-step-gesture-track track)
    (= drum-step-gesture-pad pad-note)))

(def drum-step-set-cursor (track pad-note step)
  (do
    (seq-set-track track)
    (set! drum-step-cursor-track track)
    (set! drum-step-cursor-pad pad-note)
    (set-track-cursor-step step)))

(def %drum-step-selected? (track pad-note step)
  (seq-drum-lane-step-selected? track pad-note step))

(def %drum-step-shift-anchor (track pad-note step)
  (if (and (not (= eseq.vanilla/step-key-select-anchor nil))
        (seq-drum-lane-has-selection? track pad-note))
    eseq.vanilla/step-key-select-anchor
    (if (seq-drum-lane-has-selection? track pad-note) eseq.vanilla/cursor-step step)))

(def %drum-step-select-drag-start (track pad-note step evt)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (drum-step-set-cursor track pad-note step)
    (set! drum-step-gesture-track track)
    (set! drum-step-gesture-pad pad-note)
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-press-ms nil)
    (set! eseq.vanilla/step-press-step nil)
    (set! eseq.vanilla/step-drag-progressed nil)
    (set! eseq.vanilla/step-hold-select nil)
    (if (cmd-click? evt)
      (do
        (set! eseq.vanilla/step-key-select-anchor nil)
        (set! eseq.vanilla/step-drag-anchor nil)
        (set! eseq.vanilla/step-cmd-drag-last step)
        (seq-select-drum-lane-step track pad-note step))
      (let ((anchor (%drum-step-shift-anchor track pad-note step)))
        (do
          (set! eseq.vanilla/step-key-select-anchor anchor)
          (set! eseq.vanilla/step-drag-anchor anchor)
          (set! eseq.vanilla/step-cmd-drag-last nil)
          (seq-select-drum-lane-step-range track pad-note anchor step))))))

(def drum-step-select-drag-over (track pad-note step evt)
  (if (%drum-step-gesture-lane? track pad-note)
    (do
      (step-hold-select-maybe-engage step evt)
      (if (or (selection-click? evt) eseq.vanilla/step-hold-select)
        (do
          (set! eseq.vanilla/step-click-pending nil)
          (set! eseq.vanilla/step-move-last nil)
          (set! eseq.vanilla/step-toggle-drag-value nil)
          (eseq.seq-core-state/cool-off-follow)
          (drum-step-set-cursor track pad-note step)
          (if (and (cmd-click? evt) (not eseq.vanilla/step-hold-select))
            (if (= step eseq.vanilla/step-cmd-drag-last)
              nil
              (do
                (set! eseq.vanilla/step-cmd-drag-last step)
                (if (%drum-step-selected? track pad-note step)
                  nil
                  (seq-select-drum-lane-step track pad-note step))))
            (do
              (if (= eseq.vanilla/step-drag-anchor nil) (set! eseq.vanilla/step-drag-anchor step) nil)
              (seq-select-drum-lane-step-range
                track pad-note eseq.vanilla/step-drag-anchor step))))
        (do
          (if (= step eseq.vanilla/step-press-step) nil (set! eseq.vanilla/step-drag-progressed true))
          (if (not (= eseq.vanilla/step-toggle-drag-value nil))
            (do
              (set! eseq.vanilla/step-click-pending nil)
              (eseq.seq-core-state/cool-off-follow)
              (drum-step-set-cursor track pad-note step)
              (if (= (seq-drum-lane-step-active? track pad-note step)
                    eseq.vanilla/step-toggle-drag-value)
                nil
                (seq-toggle-drum-lane-step track pad-note step)))
            (if (= eseq.vanilla/step-move-last nil)
              nil
              (if (= step eseq.vanilla/step-move-last)
                nil
                (do
                  (set! eseq.vanilla/step-click-pending nil)
                  (eseq.seq-core-state/cool-off-follow)
                  (seq-move-drum-lane-step-drag
                    track pad-note eseq.vanilla/step-move-last step)
                  (set! eseq.vanilla/step-move-last step)
                  (drum-step-set-cursor track pad-note step))))))))
    nil))

(def drum-step-pointer-down (track pad-note step evt)
  (do
    (set! drum-step-gesture-track track)
    (set! drum-step-gesture-pad pad-note)
    (if (selection-click? evt)
      (%drum-step-select-drag-start track pad-note step evt)
      (do
        (eseq.seq-core-state/cool-off-follow)
        (drum-step-set-cursor track pad-note step)
        (set! eseq.vanilla/step-drag-anchor nil)
        (set! eseq.vanilla/step-press-ms (now-ms))
        (set! eseq.vanilla/step-press-step step)
        (set! eseq.vanilla/step-drag-progressed nil)
        (set! eseq.vanilla/step-hold-select nil)
        (if (or (seq-drum-lane-step-active? track pad-note step)
              (%drum-step-selected? track pad-note step))
          (do
            (set! eseq.vanilla/step-move-last step)
            (set! eseq.vanilla/step-click-pending step)
            (set! eseq.vanilla/step-click-was-active
              (seq-drum-lane-step-active? track pad-note step))
            (set! eseq.vanilla/step-toggle-drag-value nil))
          (do
            (set! eseq.vanilla/step-move-last nil)
            (set! eseq.vanilla/step-click-pending nil)
            (set! eseq.vanilla/step-toggle-drag-value true)
            (drum-step-select-drag-over track pad-note step evt)))))))

(def drum-step-pointer-up (track pad-note step evt)
  (do
    (if (and (%drum-step-gesture-lane? track pad-note)
          (= eseq.vanilla/step-click-pending step)
          (not (selection-click? evt)))
      (if eseq.vanilla/step-click-was-active
        (seq-select-drum-lane-step-range track pad-note step step)
        (seq-toggle-drum-lane-step track pad-note step))
      nil)
    (set! eseq.vanilla/step-click-was-active nil)
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-drag-anchor nil)
    (set! eseq.vanilla/step-move-last nil)
    (set! eseq.vanilla/step-toggle-drag-value nil)
    (set! eseq.vanilla/step-press-ms nil)
    (set! eseq.vanilla/step-press-step nil)
    (set! eseq.vanilla/step-drag-progressed nil)
    (set! eseq.vanilla/step-hold-select nil)
    (set! eseq.vanilla/step-cmd-drag-last nil)
    (set! drum-step-gesture-track nil)
    (set! drum-step-gesture-pad nil)))

(def drum-step-double-click (track pad-note step evt)
  (if (and (not (selection-click? evt))
        (seq-drum-lane-step-active? track pad-note step))
    (seq-toggle-drum-lane-step track pad-note step)
    nil))

(def bus-step-double-click (step evt)
  (if (and (not (selection-click? evt)) (eseq.bus-grid/bus-step-active? step))
    (eseq.bus-grid/bus-toggle-step step)
    nil))

(def seq-set-step-param-from-step (step param value)
  (if (step-selected? step)
    (seq-set-step-param-plock param value)
    (do
      (if (seq-has-selection?) (seq-clear-selection) nil)
      (seq-set-step-param step param value))))

(def %seq-selected-step-indexes ()
  (seq-selected-step-indexes-native))

(def %seq-set-process-lane-step-value (track lane step value)
  (seq-set-process-lane-step
    track
    (get lane :instance-id)
    (get lane :inlet)
    step
    value))

(def seq-set-process-lane-from-step (track mode step value)
  (let ((lane (eseq.seqv-track-params/seqv-track-process-lane track mode)))
    (if (step-selected? step)
      (for-each
        (lambda (selected-step)
          (%seq-set-process-lane-step-value track lane selected-step value))
        (%seq-selected-step-indexes))
      (do
        (if (seq-has-selection?) (seq-clear-selection) nil)
        (%seq-set-process-lane-step-value track lane step value)))))

(def select-all-steps ()
  (do
    (eseq.seq-core-state/cool-off-follow)
    (if (eseq.seq-core-state/seq-has-selected-bus?)
      (eseq.bus-grid/bus-select-all-steps)
      (seq-select-all-steps))))

(def seq-global-select-all-steps ()
  (if (and
        (or (buffer-read-only?) (= (current-buffer-name) "*transport*"))
        (not (= (current-buffer-name) "*piano-roll*")))
    (select-all-steps)
    false))

(bind-key "C-a" "seq-global-select-all-steps")

(def seq-global-toggle-record ()
  (if (or (buffer-read-only?) (= (view-mode) "ui"))
    (seq-toggle-record)
    false))

(bind-key "." "seq-global-toggle-record")

(def delete-selected-steps ()
  (do
    (eseq.seq-core-state/cool-off-follow)
    (if (eseq.seq-core-state/seq-has-selected-bus?)
      (eseq.bus-grid/bus-delete-selected-steps)
      (seq-delete-selected-steps))))

(def duration-slider-position (duration)
  (let ((d (max 0 (min duration 32))))
    (if (<= d 2)
      (/ d 4)
      (+ 0.5 (* 0.5 (pow (/ (- d 2) 30) 0.25))))))

(def duration-slider-value (position)
  (let ((p (max 0 (min position 1))))
    (if (<= p 0.5)
      (* p 4)
      (+ 2 (* 30 (pow (* 2 (- p 0.5)) 4))))))

;; `param-mode` is a `defstate` owned by eseq.seq-core-state: bare is the
;; documented path (state_bindings, not the global ladder).
(def step-param-value (v)
  (if (= eseq.seq-core-state/param-mode 3)
    (round v)
    v))

(def step-slider-param-value (v)
  (if (= eseq.seq-core-state/param-mode 1)
    (duration-slider-value v)
    (step-param-value v)))

(def param-decimals ()
  (eseq.seqv-track-params/seqv-param-decimals eseq.seq-core-state/param-mode))
