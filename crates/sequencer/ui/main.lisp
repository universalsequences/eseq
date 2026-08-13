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
; ORDER (spec §10 hazard p): `import` is a *runtime* form — the importing
; file is compiled in full before the import executes — so a declared edge
; guarantees only that the target is evaluated before the importer's body
; runs. Anything a file needs at COMPILE time (another module's `defstate`
; keyspace, its macros, its compat-alias spellings) can only be ordered here,
; by the loader. That is what pins the two entries below to the top; the
; render roots that follow are likewise not freely reorderable, because they
; reference each other bare and a module must never import a UI root.

(load "@/ui/themes.lisp")
(seq-theme-mac-osx-dark)
;; Macros, expanded at the compile time of every consumer — including the
;; ~305 headerless content files Rust loads after us, which cannot import.
(import eseq.materials)
;; The shared-state hub. Its `defstate` keys have to exist before any reader
;; compiles, and an import inside a reader is too late to supply them.
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
;; interactions, seqv-track-params and seq-grid-mode. seq-script-picker also
;; reads eseq.seq-step-tabs `defstate`s, so it has to stay below the import
;; of eseq.transport (which is what evaluates seq-step-tabs).
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
