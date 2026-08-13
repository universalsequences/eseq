; Minimal Metal Sequencer - Step Grid UI
; C-p to toggle play/stop, Esc to clear step selection
;
; This file is the DISTRO ROOT (module spec §4/§7): it assembles the vanilla
; eseq UI out of entry modules and nothing else. Every module a module needs
; is now a declared `import` edge inside that module, so the manifest lists
; only the roots — files nothing imports because their top level *is* the
; side effect (they register effect-buffers, modes and keymaps).
;
; `load` survives for the three files whose evaluation is the point and which
; must therefore re-run every time: themes.lisp (loading a theme file IS
; applying the theme), effects.lisp (its own nested manifest) and
; effects/step-buffer.lisp (a headerless side-effect root).
;
; ORDER (spec §4, hazard (p) RESOLVED by eseq-mods.12): `import` now has a
; compile-time half — the compiler evaluates an import's target before any
; later form in the importing unit compiles — so every module declares its
; own compile-time deps (`defstate` keyspaces, macros, aliases) and the
; import block below is order-free. What still binds this file:
;   - a `load` and the calls that use it stay ordered (themes.lisp then the
;     theme call; `load` is by design the raw evaluate-here primitive);
;   - eseq.materials and eseq.seq-core-state stay LISTED (nothing imports
;     materials — its macros expand flat inside :shader/:material bodies —
;     and both serve the ~305 headerless content files Rust loads after
;     boot, which cannot import), but their position no longer matters.

(load "@/ui/themes.lisp")
(seq-theme-mac-osx-dark)
(import eseq.materials)
(import eseq.seq-core-state)

(import eseq.browser)
(import eseq.mixer)
(import eseq.patch-macros)
(load "@/ui/effects.lisp")
(import eseq.macros)
(import eseq.piano-roll)
(import eseq.choose-model)
(import eseq.transport)
(import eseq.agent)

(def seq-clear-ui-selection ()
  (do
    (seq-clear-selection)))

(bind-key "C-p" "seq-toggle-play")
(bind-key "ESC" "seq-clear-ui-selection")

;; Code editor: compile + hot-swap the current dsp code buffer. The host
;; command no-ops unless a code edit session is active.
(def seq-eval-editor-code ()
  (host-command "evaluate-editor-source" (dict)))
(bind-key "C-c C-c" "seq-eval-editor-code")

;; Library anchors: nothing imports these three, but each one imports the
;; layer below it, so they are what pulls in seq-layout, step-grid-
;; interactions, seqv-track-params and seq-grid-mode. (seq-script-picker's
;; read of eseq.seq-step-tabs `defstate`s is now its own declared import.)
(import eseq.seq-script-picker)
(import eseq.seq-macro-mapping-hooks)
(import eseq.bus-grid)

;; ui/step-grid.lisp (the legacy *metal* step grid) is intentionally NOT
;; loaded: no tile shows it, but as a loaded effect-buffer its whole-list
;; SEQ reads (steps/velocities/...) forced a full hidden-buffer rerun on
;; every step or scene edit (~5ms per launch/edit). The file is kept for
;; reference; `metal-track-tick` (the one widget the live UI still uses)
;; now lives in ui/sequencer.lisp.
(import eseq.sequencer)
(import eseq.arrangement)
(load "@/ui/effects/step-buffer.lisp")

; Startup layout is applied by Rust after this file loads. Keep this file free of
; top-level layout side effects so hot reload and buffer re-evaluation do not
; replace the active editor layout.
