;; Shared UI state + cursor/page primitives. Loads before the render-root files below so their defstates and defs exist when those files compile.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; This is the vanilla UI's shared-state hub: ~20 lisp files and several Rust
;; call sites reach its names by their flat spellings. It therefore converts
;; with NO renames and a full set of *identity* compat aliases (the
;; custom-ui-lego precedent). Identity aliases serve both rungs at once — an
;; unconverted vanilla caller matches the alias key flat, and a converted
;; module's bare reference qualifies against itself, misses, and lands on the
;; same alias by base name. Every aliased name here is a function or a
;; `defstate`, both of which are immune to hazard (m): function slots are
;; written once by their `def`, and `defstate` resolves through
;; `state_bindings` (whose keyspace the alias also covers).
;;
;; The one exception is `cursor-step` — see its pin below.
(module eseq.seq-core-state)

(export selected-bus
        selected-bus-name
        seq-has-selected-bus?
        samples-sidebar-visible
        mixer-panel-visible
        lower-panel-visible
        patch-macros-panel-visible
        param-mode
        page-size
        cursor-step-value
        set-cursor-step-value
        cursor-num-steps
        current-step
        page-count
        visible-page
        playhead-page
        page-offset
        cool-off-follow
        sel-track-vis-field
        sel-group-vis-field
        sel-bus-vis-field
        track-selected-vis-binding
        group-selected-vis-binding
        bus-selected-vis-binding)


(defstate selected-bus -1)

(def selected-bus-name ()
  (if (and (>= selected-bus 0) (< selected-bus (len SEQ.bus-names)))
    (nth SEQ.bus-names selected-bus)
    "Bus"))

(def seq-has-selected-bus? ()
  (and (>= selected-bus 0) (< selected-bus (len SEQ.bus-names))))

(defstate samples-sidebar-visible true)
(defstate mixer-panel-visible true)
(defstate lower-panel-visible true)
(defstate patch-macros-panel-visible true)

; 0=vel 1=dur 2=aux_a 3=transpose 4=pan 5=sync 6=delay
(defstate param-mode 0)

(def page-size 16)

;; Step cursor helpers are used by the FX step buffer root, so define them
;; before loading render roots.
;;
;; PINNED to eseq.vanilla (spec §3 escape hatch). `cursor-step` is the one name
;; in this file that is a *mutable plain def* — precisely hazard (m)'s exposure
;; — and it is spelled flat from outside lisp in two ways that an alias cannot
;; cover: production Rust reads it with
;; `rt.global_value("cursor-step")` (src/ui/state_values/param_fields_and_sync.rs:1299),
;; and a Rust test seeds it with a headerless `(def cursor-step N)`
;; (src/ui/host_commands/step_history.rs:1632) — a re-def that would strand any
;; healed module slot on the old cell. Pinning keeps the single flat slot every
;; one of those spellings already resolves to, so nothing outside changes.
(def eseq.vanilla/cursor-step 0)

;; Owner-side reader for the mutable global above (module spec §10 hazard m).
;; A converted module's bare `cursor-step` interns its own
;; `<module>/cursor-step` slot; the late-binding heal aliases that slot to this
;; cell on first read, but the very next `(set! cursor-step …)` below is a
;; StoreGlobal that replaces the owner's slot and unlinks the alias, freezing
;; the module's view at whatever the value was when it first read.  Reading
;; through a function is immune: function slots are written once, by their
;; `def`, so the heal never gets unlinked.  ui/sequencer.lisp is the caller.
;; NB: every in-file reference to the pinned global must use the
;; `eseq.vanilla/` spelling — a bare `cursor-step` here would intern this
;; module's own `eseq.seq-core-state/cursor-step`, a different slot.
(def cursor-step-value () eseq.vanilla/cursor-step)

(def set-cursor-step-value (step)
  (let ((parameter-step
          (if (> (or SEQ.fx-step-selection-count 0) 0)
            (or SEQ.fx-step-parameter-step step)
            step)))
    (do
      (set! eseq.vanilla/cursor-step step)
      (reactive-set "SEQ" "fx-step-cursor-number" (+ step 1))
      (reactive-set "SEQ" "fx-step-parameter-step" parameter-step)
      (reactive-set "SEQ" "fx-step-value-transpose" (nth SEQ.transposes parameter-step))
      (reactive-set "SEQ" "fx-step-value-velocity" (nth SEQ.velocities parameter-step))
      (reactive-set "SEQ" "fx-step-value-duration" (nth SEQ.durations parameter-step))
      (reactive-set "SEQ" "fx-step-value-retrig" (nth SEQ.retrigs parameter-step))
      (reactive-set "SEQ" "fx-step-value-retrig-rate" (nth SEQ.retrig-rates parameter-step)))))

;; The step cursor always tracks the current track's pattern length.  The old
;; bus-gate step sequencer (and its `SEQ.bus-num-steps` reactive list) is gone,
;; so a selected bus/group no longer implies a separate step count.
(def cursor-num-steps () SEQ.tp-num-steps)

(def current-step ()
  (mod eseq.vanilla/cursor-step (max 1 (cursor-num-steps))))

(def page-count ()
  (max 1 (floor (/ (+ SEQ.tp-num-steps (- page-size 1)) page-size))))

;; Private: the app-wide sweep found no caller outside this file, and
;; `current-page` is one of the three names hazard (k) calls out by name as
;; collision-famous. `%` keeps it out of the flat keyspace entirely.
(def current-page ()
  (min (floor (/ (current-step) page-size)) (- (page-count) 1)))

(def visible-page ()
  (if (and SEQ.playing SEQ.auto-follow (not (seq-has-selection?)))
    (playhead-page)
    (current-page)))

(def playhead-page ()
  (min SEQ.playhead-page
    (- (page-count) 1)))

(def page-offset ()
  (* (visible-page) page-size))

(def cool-off-follow ()
  (seq-pause-auto-follow))

;; ── Selection-visibility projection (eseq-4jv) ──────────────────────────
;; `selected-bus` is the fx-panel owner discriminant, and it used to be read
;; directly from render bodies all over the UI: every sequencer track row,
;; every arrangement lane, the group blocks, and the mixer bus strips. A
;; single group/track selection therefore re-rendered every one of those
;; subtrees before the *fx* panel had even started its (legitimate) owner
;; switch. This one effect is now the only selection-visibility reader of the
;; defstate: it projects the combined "does this row draw selected?" answer
;; into per-row SEQV float fields, and rows bind those fields, so a selection
;; change dirties only the affected retained widgets — no subtree re-renders.
;; The *fx* buffer root is the intended remaining reader (it must
;; restructure); nothing else should read `selected-bus` in render.
(def sel-track-vis-field (i) (str "sel-track-vis-" i))
(def sel-group-vis-field (gid) (str "sel-group-vis-" gid))
(def sel-bus-vis-field (i) (str "sel-bus-vis-" i))

(def track-selected-vis-binding (i)
  (bind "SEQV" (sel-track-vis-field i)))

(def group-selected-vis-binding (gid)
  (bind "SEQV" (sel-group-vis-field gid)))

(def bus-selected-vis-binding (i)
  (bind "SEQV" (sel-bus-vis-field i)))

(def sel-bus-index-of (bus-id)
  (let ((matches (filter (lambda (i) (= (nth SEQ.bus-ids i) bus-id))
          (range 0 (len SEQ.bus-ids)))))
    (if (> (len matches) 0) (nth matches 0) -1)))

;; A named effect-buffer returning nil is inert (the *plock-sync* precedent):
;; it owns the churn-prone reads so no visible surface has to.
(effect-buffer "*sel-sync*"
  (let ((bus-active (seq-has-selected-bus?)))
    (do
      ;; Track highlight: the Rust-owned per-track selection, gated off while
      ;; a bus/group owns the fx panel (`reactive-value` records the reads,
      ;; so Rust-side selection changes re-run this projection too).
      (for-each
        (lambda (i)
          (reactive-set "SEQV" (sel-track-vis-field i)
            (if bus-active
              0
              (reactive-value (bind-seq (str "track-selected-" i))))))
        (range 0 SEQ.num-tracks))
      (for-each
        (lambda (i)
          (reactive-set "SEQV" (sel-bus-vis-field i)
            (if (and bus-active (= selected-bus i)) 1 0)))
        (range 0 (len SEQ.bus-names)))
      (for-each
        (lambda (gi)
          (let ((group (nth SEQ.groups gi)))
            (reactive-set "SEQV" (sel-group-vis-field (get group :id))
              (if (and bus-active
                    (= selected-bus (sel-bus-index-of (get group :bus-id))))
                1
                0))))
        (range 0 (len SEQ.groups)))
      nil)))
