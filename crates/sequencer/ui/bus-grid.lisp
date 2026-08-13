;; Bus sequencer panel: bus-seq-* accessors and bus-step-* grid interactions.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; Notes specific to this file:
;;
;;   * THE ELEVEN PINNED DRAG-STATE GLOBALS (hazard m). This file and
;;     ui/step-grid-interactions.lisp share ONE gesture state — only one drag
;;     runs at a time, so the bus lane and the track lanes deliberately use the
;;     same cells. Those cells are *mutable plain defs*, which a compat alias
;;     cannot rescue (the late-binding heal repairs an empty slot and the next
;;     `set!` unlinks it), so eseq.step-grid-interactions pins them into
;;     `eseq.vanilla` with the spec §3 escape hatch. Two files sharing mutable
;;     state must name the SAME slot, so every reference here — read and
;;     `set!` alike — uses the explicit `eseq.vanilla/` spelling. A bare one
;;     would intern THIS module's own private shadow and the divergence would
;;     be silent in both directions. The eleven are: step-key-select-anchor,
;;     step-drag-anchor, step-click-pending, step-move-last,
;;     step-toggle-drag-value, step-click-was-active, step-press-ms,
;;     step-press-step, step-drag-progressed, step-hold-select,
;;     step-cmd-drag-last.
;;   * `eseq.vanilla/cursor-step` is pinned the same way by
;;     ui/seq-core-state.lisp; `%bus-current-step`'s read uses that spelling.
;;   * `selected-bus` and `param-mode` stay BARE: both are eseq.seq-core-state
;;     `defstate`s, which resolve through `state_bindings` on the flat key and
;;     never touch the global ladder.
;;   * Hazard (n) checked: no Rust harness reads and slices this file
;;     (`grep -rn "bus-grid.lisp" crates/sequencer/src` finds one comment and
;;     no `read_to_string`), so `:as` import aliases are safe here.
;;   * The public names below keep their flat spellings and get *identity*
;;     compat aliases. Stripping the `bus-` prefix is not an option: it would
;;     collide head-on with eseq.seq-core-state / eseq.step-grid-interactions
;;     (`page-count`, `current-step`, `page-offset`, `step-index`,
;;     `step-visible?`, `goto-page`, `toggle-step`, …) — hazard (k). Their two
;;     callers are both converted modules that spell them bare
;;     (ui/step-grid-interactions.lisp, which this wave may not edit, and the
;;     legacy parse-only ui/step-grid.lisp); an identity alias is reached by a
;;     converted module's bare reference through the base-name rung.

(module eseq.bus-grid)

(import eseq.seq-core-state :as core)
(import eseq.step-grid-interactions :as sgi)
(import eseq.seqv-track-params)
(import eseq.seq-grid-mode)

(module-compat-alias bus-seq-list bus-seq-list)
(module-compat-alias bus-seq-playhead bus-seq-playhead)
(module-compat-alias bus-seq-timebase bus-seq-timebase)
(module-compat-alias bus-seq-param-values bus-seq-param-values)
(module-compat-alias bus-seq-param-name bus-seq-param-name)
(module-compat-alias bus-seq-param-min bus-seq-param-min)
(module-compat-alias bus-seq-param-max bus-seq-param-max)
(module-compat-alias bus-page-count bus-page-count)
(module-compat-alias bus-current-step bus-current-step)
(module-compat-alias bus-current-page bus-current-page)
(module-compat-alias bus-step-index bus-step-index)
(module-compat-alias bus-step-visible? bus-step-visible?)
(module-compat-alias bus-page-panel-width bus-page-panel-width)
(module-compat-alias bus-goto-page bus-goto-page)
(module-compat-alias bus-set-step-param bus-set-step-param)
(module-compat-alias bus-set-selected-step-param bus-set-selected-step-param)
(module-compat-alias bus-toggle-step bus-toggle-step)
(module-compat-alias bus-step-active? bus-step-active?)
(module-compat-alias bus-select-step-range bus-select-step-range)
(module-compat-alias bus-select-all-steps bus-select-all-steps)
(module-compat-alias bus-delete-selected-steps bus-delete-selected-steps)
(module-compat-alias bus-shift-selected-steps bus-shift-selected-steps)
(module-compat-alias bus-step-select-drag-start bus-step-select-drag-start)
(module-compat-alias bus-step-select-drag-over bus-step-select-drag-over)
(module-compat-alias bus-step-pointer-down bus-step-pointer-down)
(module-compat-alias bus-step-pointer-up bus-step-pointer-up)
(module-compat-alias bus-set-sequencer-label bus-set-sequencer-label)

(def bus-seq-list (lists)
  (if (core/seq-has-selected-bus?)
    (nth lists selected-bus)
    '()))

(def bus-seq-playhead ()
  (if (core/seq-has-selected-bus?)
    (nth SEQ.bus-playheads selected-bus)
    0))

(def %bus-seq-num-steps ()
  (if (core/seq-has-selected-bus?)
    (nth SEQ.bus-num-steps selected-bus)
    16))

(def bus-seq-timebase ()
  (if (core/seq-has-selected-bus?)
    (nth SEQ.bus-timebases selected-bus)
    "16"))

(def %bus-seq-swing ()
  (if (core/seq-has-selected-bus?)
    (nth SEQ.bus-swings selected-bus)
    50))

(def %bus-seq-swing-resolution ()
  (if (core/seq-has-selected-bus?)
    (nth SEQ.bus-swing-resolutions selected-bus)
    "1/16"))

(def bus-seq-param-values ()
  (if (= param-mode 1) (bus-seq-list SEQ.bus-durations)
    (if (= param-mode 2) (bus-seq-list SEQ.bus-syncs)
      (bus-seq-list SEQ.bus-velocities))))

(def bus-seq-param-name ()
  (if (= param-mode 1) "Duration"
    (if (= param-mode 2) "Sync"
      "Gate Amount")))

(def %bus-seq-param-key ()
  (if (= param-mode 1) "duration"
    (if (= param-mode 2) "sync"
      "velocity")))

(def bus-seq-param-min ()
  (if (= param-mode 1) 0.1 0))

(def bus-seq-param-max ()
  (if (= param-mode 1) 2
    (if (= param-mode 2) (- (len SEQ.sync-labels) 1)
      1)))

(def bus-page-count ()
  (max 1 (floor (/ (+ (%bus-seq-num-steps) (- core/page-size 1)) core/page-size))))

;; `cursor-step` is pinned to eseq.vanilla by ui/seq-core-state.lisp.
(def bus-current-step ()
  (min eseq.vanilla/cursor-step (- (max 1 (%bus-seq-num-steps)) 1)))

(def bus-current-page ()
  (min (floor (/ (bus-current-step) core/page-size)) (- (bus-page-count) 1)))

(def %bus-page-offset ()
  (* (bus-current-page) core/page-size))

(def bus-step-index (i)
  (+ (%bus-page-offset) i))

(def bus-step-visible? (i)
  (< (bus-step-index i) (%bus-seq-num-steps)))

(def bus-page-panel-width ()
  (+ 0.4 (* (bus-page-count) (sgi/page-slot-width))))

(def bus-goto-page (page)
  (do
    (core/cool-off-follow)
    (core/set-cursor-step-value (min (* page core/page-size) (- (max 1 (%bus-seq-num-steps)) 1)))))

(def bus-set-step-param (step value)
  (host-command "set-bus-step-param"
    (dict :bus selected-bus :step step :param (%bus-seq-param-key) :value value)))

(def bus-set-selected-step-param (value)
  (host-command "set-selected-bus-step-param"
    (dict :bus selected-bus :param (%bus-seq-param-key) :value value)))

(def bus-toggle-step (step)
  (do
    (core/cool-off-follow)
    (core/set-cursor-step-value step)
    (host-command "toggle-bus-step" (dict :bus selected-bus :step step))))

(def %bus-set-step-active (step active)
  (do
    (core/cool-off-follow)
    (core/set-cursor-step-value step)
    (host-command "set-bus-step-active"
      (dict :bus selected-bus :step step :active active))))

(def bus-step-active? (step)
  (nth (bus-seq-list SEQ.bus-steps) step))

(def bus-select-step-range (start end)
  (host-command "select-bus-step-range"
    (dict :bus selected-bus :start start :end end)))

(def %bus-select-step (step)
  (host-command "select-bus-step"
    (dict :bus selected-bus :step step)))

(def bus-select-all-steps ()
  (host-command "select-all-bus-steps" (dict :bus selected-bus)))

(def bus-delete-selected-steps ()
  (host-command "delete-selected-bus-steps" (dict :bus selected-bus)))

(def %bus-move-step-drag (start target)
  (host-command "move-bus-step-drag"
    (dict :bus selected-bus :start start :target target)))

(def bus-shift-selected-steps (direction)
  (host-command "shift-selected-bus-steps"
    (dict :bus selected-bus :direction direction)))

;; Every `eseq.vanilla/step-*` below is one of the eleven shared drag-state
;; cells pinned by ui/step-grid-interactions.lisp — see the header. Bare
;; spellings here would silently fork the gesture state.
(def bus-step-select-drag-start (step evt)
  (do
    (core/cool-off-follow)
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-press-ms nil)
    (set! eseq.vanilla/step-press-step nil)
    (set! eseq.vanilla/step-drag-progressed nil)
    (set! eseq.vanilla/step-hold-select nil)
    (if (sgi/cmd-click? evt)
      (do
        (set! eseq.vanilla/step-key-select-anchor nil)
        (core/set-cursor-step-value step)
        (set! eseq.vanilla/step-drag-anchor nil)
        (set! eseq.vanilla/step-cmd-drag-last step)
        (%bus-select-step step))
      (let ((anchor (sgi/step-shift-anchor step)))
        (do
          (set! eseq.vanilla/step-key-select-anchor anchor)
          (core/set-cursor-step-value step)
          (set! eseq.vanilla/step-drag-anchor anchor)
          (set! eseq.vanilla/step-cmd-drag-last nil)
          (bus-select-step-range anchor step))))))

(def bus-step-select-drag-over (step evt)
  (do
    (sgi/step-hold-select-maybe-engage step evt)
    (if (or (sgi/selection-click? evt) eseq.vanilla/step-hold-select)
      (do
        (set! eseq.vanilla/step-click-pending nil)
        (set! eseq.vanilla/step-move-last nil)
        (set! eseq.vanilla/step-toggle-drag-value nil)
        (core/cool-off-follow)
        (core/set-cursor-step-value step)
        (if (and (sgi/cmd-click? evt) (not eseq.vanilla/step-hold-select))
          (if (= step eseq.vanilla/step-cmd-drag-last)
            nil
            (do
              (set! eseq.vanilla/step-cmd-drag-last step)
              (if (sgi/step-selected? step) nil (%bus-select-step step))))
          (do
            (if (= eseq.vanilla/step-drag-anchor nil) (set! eseq.vanilla/step-drag-anchor step) nil)
            (bus-select-step-range eseq.vanilla/step-drag-anchor step))))
      (do
        (if (= step eseq.vanilla/step-press-step) nil (set! eseq.vanilla/step-drag-progressed true))
        (if (not (= eseq.vanilla/step-toggle-drag-value nil))
          (do
            (set! eseq.vanilla/step-click-pending nil)
            (core/cool-off-follow)
            (core/set-cursor-step-value step)
            (if (= (bus-step-active? step) eseq.vanilla/step-toggle-drag-value)
              nil
              (%bus-set-step-active step eseq.vanilla/step-toggle-drag-value)))
          (if (= eseq.vanilla/step-move-last nil)
            nil
            (if (= step eseq.vanilla/step-move-last)
              nil
              (do
                (set! eseq.vanilla/step-click-pending nil)
                (core/cool-off-follow)
                (%bus-move-step-drag eseq.vanilla/step-move-last step)
                (set! eseq.vanilla/step-move-last step)
                (core/set-cursor-step-value step)))))))))

(def bus-step-pointer-down (step evt)
  (if (sgi/selection-click? evt)
    (bus-step-select-drag-start step evt)
    (do
      (core/cool-off-follow)
      (core/set-cursor-step-value step)
      (set! eseq.vanilla/step-drag-anchor nil)
      (set! eseq.vanilla/step-press-ms (now-ms))
      (set! eseq.vanilla/step-press-step step)
      (set! eseq.vanilla/step-drag-progressed nil)
      (set! eseq.vanilla/step-hold-select nil)
      (if (or (bus-step-active? step) (sgi/step-selected? step))
        (do
          (set! eseq.vanilla/step-move-last step)
          (set! eseq.vanilla/step-click-pending step)
          (set! eseq.vanilla/step-click-was-active (bus-step-active? step))
          (set! eseq.vanilla/step-toggle-drag-value nil))
        (do
          (set! eseq.vanilla/step-move-last nil)
          (set! eseq.vanilla/step-click-pending nil)
          (set! eseq.vanilla/step-toggle-drag-value true)
          (bus-step-select-drag-over step evt))))))

(def bus-step-pointer-up (step evt)
  (do
    (if (and (= eseq.vanilla/step-click-pending step) (not (sgi/selection-click? evt)))
      (if eseq.vanilla/step-click-was-active
        (bus-select-step-range step step)
        (bus-toggle-step step))
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

(def %bus-set-sequencer-param (param value)
  (host-command "set-bus-sequencer-param"
    (dict :bus selected-bus :param param :value value)))

(def bus-set-sequencer-label (param label)
  (host-command "set-bus-sequencer-param"
    (dict :bus selected-bus :param param :label label)))
