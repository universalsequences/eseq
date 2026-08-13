;; Panel hide/toggle commands, piano-roll open/close, main-view switching, and their keybindings.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; This is a command hub: nearly every def here is reached by its FLAT spelling
;; from outside — production Rust (`src/ui/input.rs`, `src/ui/edit_sessions.rs`)
;; invokes them by name, M-x users type them, and several callers are
;; modules that cannot import this one (see below). It therefore converts with
;; NO renames and a full set of *identity* compat aliases (the seq-core-state /
;; custom-ui-lego precedent): an unconverted caller matches the alias key flat,
;; and a converted module's bare reference qualifies against itself, misses, and
;; lands on the same alias by base name. Every aliased name is a function, so
;; hazard (m) cannot bite (function slots are written once, by their `def`).
;;
;; NO IMPORTS, deliberately, for two independent reasons:
;;
;;  (1) HAZARD (n). `metal_seq_fx_lisp_lays_out_inline_custom_instrument_mod_selector`
;;      (src/ui/state_values/tests.rs) `read_to_string`s THIS FILE and evals only
;;      the slice running from the seq-show-fx-lower-panel def down to the Tab
;;      bind-key line, into a VM that never loads ui/*.lisp. (Its slicer is a
;;      plain `str::find` on those two literals, so do NOT spell either of them
;;      verbatim in this comment — the first match wins and the slice becomes
;;      garbage. That is why they are described rather than quoted here.)
;;      Any `alias/name` spelling inside that slice
;;      is a hard compile error, and a dotted `eseq.foo/name` spelling would
;;      `warn_once` — and that test panics on a leftover status message. So the
;;      slice's bodies stay byte-identical, and for consistency the whole file
;;      keeps its bare cross-file references.
;;  (2) Import cycle. ui/seq-layout.lisp (eseq.seq-layout) loads BEFORE this file
;;      and calls seq-hide-samples-sidebar / seq-hide-mixer-panel /
;;      seq-hide-fx-panel / seq-hide-patch-macros-panel / seq-current-step-buffer;
;;      ui/effects/param-controls.lisp (imported by seq-layout) calls
;;      seq-show-fx-lower-panel. Importing seq-layout here and teaching it to
;;      import this module back would be a cycle. The identity aliases below are
;;      what those already-converted modules resolve through.
;;
;; The cross-file names this file reads are all safe bare:
;;  - samples-sidebar-visible / mixer-panel-visible / lower-panel-visible /
;;    patch-macros-panel-visible / selected-bus are `defstate`s owned by
;;    eseq.seq-core-state; step-panel-buffer, remembered-step-panel-buffer,
;;    lower-panel-buffer, piano-roll-placement, seq-main-view and seq-layout-mode
;;    are `defstate`s in ui/seq-step-tabs.lisp (eseq.seq-step-tabs). `defstate`
;;    resolves through `state_bindings` on the flat key, so both reads and the
;;    `set!`s below are hazard (j)/(m) immune. There is not one outbound write to
;;    a plain mutable `def` in this file.
;;  - seq-apply-fx-layout / seq-apply-piano-roll-layout / seq-refresh-current-layout
;;    are eseq.seq-layout compat-alias keys; seqv-toggle-current-track-expanded is
;;    an eseq.sequencer key (UI root — never import); instrument-toggle-mods-view
;;    is an eseq.effects.effect-panels key; seq-toggle-sound-palette is an
;;    eseq.sound-palette key. All reachable bare via the base-name alias rung.
(module eseq.seq-panels)

;; Identity aliases. Grouped by what forces each:
;;   [rust]     production Rust invokes/evals the flat name
;;   [key]      also a `bind-key` handler string — NOT itself a reason to alias:
;;              `Runtime::bind_key` calls `qualify_registration_name` exactly
;;              like `mode_bind_key` (runtime.rs:864), so a handler defined in
;;              THIS module qualifies to a slot this module owns and dispatch is
;;              an exact hit. Recorded only to say which chord reaches the name.
;;   [module]   an already-converted module calls it bare and cannot import us
;;   [vanilla]  a headerless lisp caller (ui/capture-fixtures/*.lisp)
;;   [m-x]      user-facing command; the flat spelling is the documented one
(module-compat-alias seq-hide-samples-sidebar seq-hide-samples-sidebar)            ; [module] seq-layout
(module-compat-alias seq-hide-mixer-panel seq-hide-mixer-panel)                    ; [module] seq-layout
(module-compat-alias seq-hide-fx-panel seq-hide-fx-panel)                          ; [module] seq-layout
(module-compat-alias seq-hide-patch-macros-panel seq-hide-patch-macros-panel)      ; [module] seq-layout
;; (seq-toggle-patch-macros-panel needs no alias: its only external reach was
;; the C-x m `bind-key`, which qualifies against this module — see [key] above.)
(module-compat-alias seq-toggle-samples-sidebar seq-toggle-samples-sidebar)        ; [module] transport.lisp
(module-compat-alias seq-toggle-mixer-panel seq-toggle-mixer-panel)                ; [rust] input.rs, [module] transport.lisp
(module-compat-alias seq-toggle-fx-panel seq-toggle-fx-panel)                      ; [module] transport.lisp
(module-compat-alias seq-restore-instrument-patcher-layout seq-restore-instrument-patcher-layout) ; [rust] edit_sessions.rs
(module-compat-alias seq-current-step-buffer seq-current-step-buffer)              ; [rust] input.rs, [module] seq-layout
(module-compat-alias seq-close-piano-roll seq-close-piano-roll)                    ; [m-x]
(module-compat-alias seq-open-piano-roll-bottom-for-track seq-open-piano-roll-bottom-for-track) ; [module] sequencer.lisp (UI root)
(module-compat-alias seq-open-arrangement-piano-roll-bottom-for-track seq-open-arrangement-piano-roll-bottom-for-track) ; [module] arrangement.lisp, [vanilla] capture-fixtures
(module-compat-alias seq-open-piano-roll-bottom seq-open-piano-roll-bottom)        ; [module] sequencer.lisp (UI root)
(module-compat-alias seq-open-piano-roll-main seq-open-piano-roll-main)            ; [m-x]
(module-compat-alias seq-open-piano-roll-preferred seq-open-piano-roll-preferred)  ; [m-x]
(module-compat-alias seq-show-sequencer-main seq-show-sequencer-main)              ; [module] transport.lisp
(module-compat-alias seq-open-arrangement seq-open-arrangement)                    ; [module] arrangement.lisp + transport.lisp, [vanilla] capture-fixtures
(module-compat-alias seq-toggle-arrangement seq-toggle-arrangement)                ; [rust] input.rs, [key] Tab
(module-compat-alias seq-toggle-current-track-expanded-main seq-toggle-current-track-expanded-main) ; [m-x]
(module-compat-alias seq-toggle-piano-roll-main seq-toggle-piano-roll-main)        ; [m-x]
(module-compat-alias seq-toggle-piano-roll-placement seq-toggle-piano-roll-placement) ; [rust] input.rs
(module-compat-alias seq-toggle-fx-piano-roll seq-toggle-fx-piano-roll)            ; [m-x]
(module-compat-alias seq-toggle-main-or-piano-roll seq-toggle-main-or-piano-roll)  ; [rust] input.rs, [key] BackTab
(module-compat-alias seq-show-fx-lower-panel seq-show-fx-lower-panel)              ; [module] sequencer.lisp + effects/param-controls.lisp
(module-compat-alias seq-toggle-current-track-mods-view seq-toggle-current-track-mods-view) ; [rust] input.rs

(def seq-hide-samples-sidebar ()
  (if samples-sidebar-visible
    (do
      (set! samples-sidebar-visible false)
      (seq-refresh-current-layout))
    nil))

(def seq-hide-mixer-panel ()
  (if mixer-panel-visible
    (do
      (%sync-step-panel-buffer-from-current-window)
      (set! mixer-panel-visible false)
      (seq-refresh-current-layout))
    nil))

(def seq-hide-fx-panel ()
  (if lower-panel-visible
    (do
      (set! lower-panel-visible false)
      (seq-refresh-current-layout))
    nil))

(def seq-hide-patch-macros-panel ()
  (if patch-macros-panel-visible
    (do
      (set! patch-macros-panel-visible false)
      (seq-refresh-current-layout))
    nil))

;; Bound to C-x m below: dragging the macro sidebar past the collapse threshold
;; calls `seq-hide-patch-macros-panel`, so without a discoverable toggle the
;; only way back is M-x.
(def seq-toggle-patch-macros-panel ()
  (do
    (set! patch-macros-panel-visible (not patch-macros-panel-visible))
    (seq-refresh-current-layout)))

(bind-key "C-x m" "seq-toggle-patch-macros-panel")

(def seq-toggle-samples-sidebar ()
  (do
    (set! samples-sidebar-visible (not samples-sidebar-visible))
    (seq-refresh-current-layout)))

(def seq-toggle-mixer-panel ()
  (do
    (%sync-step-panel-buffer-from-current-window)
    (set! mixer-panel-visible (not mixer-panel-visible))
    (seq-refresh-current-layout)))

(def seq-toggle-fx-panel ()
  (do
    (set! lower-panel-visible (not lower-panel-visible))
    (seq-refresh-current-layout)))

(def seq-restore-instrument-patcher-layout ()
  (do
    (set! step-panel-buffer remembered-step-panel-buffer)
    (if (= lower-panel-buffer "*piano-roll*")
      (seq-apply-piano-roll-layout)
      (seq-apply-fx-layout))))

(def seq-current-step-buffer ()
  (seq-sanitized-step-buffer
    (if (= step-panel-buffer "*piano-roll*")
      remembered-step-panel-buffer
      step-panel-buffer)))

(def %sync-step-panel-buffer-from-current-window ()
  (let ((buffer (current-buffer-name)))
    (if (seq-main-step-tab-buffer? buffer)
      (do
        (set! step-panel-buffer buffer)
        (set! remembered-step-panel-buffer buffer))
      nil)))

(def %piano-roll-open? ()
  (or (= step-panel-buffer "*piano-roll*")
    (= lower-panel-buffer "*piano-roll*")))

(def seq-close-piano-roll ()
  (if (= step-panel-buffer "*piano-roll*")
    (do
      (set! step-panel-buffer (seq-current-step-buffer))
      (set-window-buffer step-panel-buffer)
      (seq-apply-fx-layout))
    (do
      (if (= (current-buffer-name) "*piano-roll*")
        (set-window-buffer "*fx*")
        (set-window-buffer-for "*piano-roll*" "*fx*"))
      (seq-apply-fx-layout))))

(def %open-piano-roll-bottom-for-track-core (track)
  (do
    (set! lower-panel-visible true)
    (if (= step-panel-buffer "*piano-roll*")
      (set! step-panel-buffer (seq-current-step-buffer))
      nil)
    (if (= (current-buffer-name) "*fx*")
      (set-window-buffer "*piano-roll*")
      (set-window-buffer-for "*fx*" "*piano-roll*"))
    (piano-roll-request-fit-for-track track)
    (seq-apply-piano-roll-layout)))

(def seq-open-piano-roll-bottom-for-track (track)
  (do
    (reactive-set "SEQV" "piano-roll-arrangement-mode" 0)
    (%open-piano-roll-bottom-for-track-core track)))

(def seq-open-arrangement-piano-roll-bottom-for-track (track)
  (do
    (reactive-set "SEQV" "piano-roll-arrangement-mode" 1)
    (%open-piano-roll-bottom-for-track-core track)))

(def seq-open-piano-roll-bottom ()
  (seq-open-piano-roll-bottom-for-track SEQ.current-track))

(def seq-open-piano-roll-main ()
  (seq-open-piano-roll-bottom))

(def seq-open-piano-roll-preferred ()
  (seq-open-piano-roll-bottom))

(def %switch-main-view (view)
  (let ((old-buffer (seq-visible-main-panel-buffer)))
    (do
      (set! seq-main-view view)
      (if (= seq-layout-mode :lower-panel)
        ;; Reapplying the layout when nothing changed clobbers incremental
        ;; relayout dirty tracking (e.g. expand/collapse row diffs), so only
        ;; refresh when the visible main panel actually switched buffers.
        (if (not (= old-buffer (seq-visible-main-panel-buffer)))
          (do
            (set-window-buffer-for old-buffer (seq-visible-main-panel-buffer))
            (seq-refresh-current-layout))
          nil)
        ;; An explicit view switch leaves the instrument patcher workspace and
        ;; returns to the normal sequencer workspace.
        (seq-apply-fx-layout)))))

(def seq-show-sequencer-main ()
  (do
    (reactive-set "SEQV" "piano-roll-arrangement-mode" 0)
    (set! remembered-step-panel-buffer "*sequencer*")
    (set! step-panel-buffer "*sequencer*")
    (%switch-main-view :session)))

;; Arrangement is an app view, not a sequencer tile tab. It owns a wider main
;; layout without the step and track context panes.
(def seq-open-arrangement ()
  (%switch-main-view :arrangement))

(def seq-toggle-arrangement ()
  (if (seq-arrangement-view?)
    (seq-show-sequencer-main)
    (seq-open-arrangement)))

(def seq-toggle-current-track-expanded-main ()
  (do
    (seq-show-sequencer-main)
    (seqv-toggle-current-track-expanded)))

(def seq-toggle-piano-roll-main ()
  (seq-toggle-main-or-piano-roll))

(def seq-toggle-piano-roll-placement ()
  (do
    (set! piano-roll-placement :bottom)
    (if (= step-panel-buffer "*piano-roll*")
      (seq-open-piano-roll-bottom)
      nil)))

(def seq-toggle-fx-piano-roll ()
  (do
    (set! lower-panel-visible true)
    (if (= (current-buffer-name) "*fx*")
    (do
      (set-window-buffer "*piano-roll*")
      (set! lower-panel-buffer "*piano-roll*")
      (piano-roll-request-fit)
      (seq-apply-piano-roll-layout))
    (if (= (current-buffer-name) "*piano-roll*")
      (do
        (set-window-buffer "*fx*")
        (set! lower-panel-buffer "*fx*")
        (seq-apply-fx-layout))
      (if (= lower-panel-buffer "*fx*")
        (do
          (set-window-buffer-for "*fx*" "*piano-roll*")
          (set! lower-panel-buffer "*piano-roll*")
          (piano-roll-request-fit)
          (seq-apply-piano-roll-layout))
        (do
          (set-window-buffer-for "*piano-roll*" "*fx*")
          (set! lower-panel-buffer "*fx*")
          (seq-apply-fx-layout)))))))

(def seq-toggle-main-or-piano-roll ()
  (if (or (= SEQ.editor-mode "new-instrument")
          (= SEQ.editor-mode "edit-instrument")
          (= SEQ.editor-mode "new-effect")
          (= SEQ.editor-mode "edit-effect"))
    (host-command "toggle-instrument-patcher-source" (dict))
    (if (%piano-roll-open?)
      (seq-close-piano-roll)
      (seq-open-piano-roll-bottom))))

(def seq-show-fx-lower-panel ()
  (do
    ;; This is an explicit mode transition even when the FX buffer is already
    ;; visible; do not leave a stale arrangement-editor mode behind.
    (reactive-set "SEQV" "piano-roll-arrangement-mode" 0)
    (if (or (not lower-panel-visible) (= lower-panel-buffer "*piano-roll*"))
      (do
        (set! lower-panel-visible true)
        (if (= lower-panel-buffer "*piano-roll*")
          (if (= (current-buffer-name) "*piano-roll*")
            (set-window-buffer "*fx*")
            (set-window-buffer-for "*piano-roll*" "*fx*"))
          nil)
        (seq-apply-fx-layout))
      nil)))

(def seq-toggle-current-track-mods-view ()
  (do
    (set! selected-bus -1)
    (instrument-toggle-mods-view)
    (seq-show-fx-lower-panel)))

(bind-key "Tab" "seq-toggle-arrangement")
;; Sound palette overlay (takes spec 17.6): toggles on the bound clip, or
;; the current track's binding outside the timeline.
(bind-key "C-x p" "seq-toggle-sound-palette")
(bind-key "BackTab" "seq-toggle-main-or-piano-roll")
