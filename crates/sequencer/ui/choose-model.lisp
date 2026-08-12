;; ui/choose-model.lisp -- `M-x choose-model`: pick the LLM the patcher's
;; agentic bubbles (cmd+k / cmd+shift+k) run on.
;;
;; The bubbles are one-shot turns rather than conversations, so unlike the
;; *agent* panel's per-conversation `agent/set-model` the choice is a single
;; process-global setting, owned by Rust (agent::model_choice) and persisted
;; to .eseq/prefs.json. This file is only the picker.
;;
;; Mounted by the two Rust-generated patcher buffer templates in
;; src/ui/edit_sessions.rs — `instrument_patcher_buffer_source` and
;; `effect_patcher_buffer_source` — which each wrap their `patcher` in a
;; v-stack with `(choose-model-panel)`. Those templates are compiled when an
;; edit session opens, i.e. long after this file loads, so they keep resolving
;; through the compat aliases below. main.lisp only guarantees this file loads
;; before those buffers exist.
;;
;; The panel lives in the patcher's own buffer because a modal only receives
;; pointer input through the *active* tile, and the patch canvas is the active
;; tile; closed, it costs no layout.
;;
;; The two mounts share the single global `open?` below, which is safe because
;; an instrument patcher buffer and an effect patcher buffer are never on
;; screen together — entering either edit session replaces the other's tile.
;; Adding a third mount in a buffer that CAN coexist with a patcher would
;; render two panels off one flag; scope the state per buffer first if so.
(module eseq.choose-model)

;; Compat aliases (module-system spec §10 slice 3) for every name with callers
;; outside this file: the Rust buffer templates, and tests that drive the
;; picker by name. `choose-model` itself keeps its spelling — it is the M-x
;; command name — but still needs an alias, because bare `(choose-model)` from
;; an unconverted caller does not find `eseq.choose-model/choose-model`.
(module-compat-alias choose-model choose-model)
(module-compat-alias choose-model-open? open?)
(module-compat-alias choose-model-open open)
(module-compat-alias choose-model-close close)
(module-compat-alias choose-model-select select)
(module-compat-alias choose-model-current current)
(module-compat-alias choose-model-current-label current-label)
(module-compat-alias choose-model-panel panel)

;; Modal visibility is pure UI state, so it lives here rather than in Rust.
(defstate open? false)
;; Bumped on every write so the panel re-reads `agent/patch-model`, which is a
;; plain native call and therefore not reactive on its own.
(defstate generation 0)

;; The sentinel row standing in for "no explicit choice" — the bubbles then
;; fall back to their built-in provider preference.
(def %default-label () "Default (auto)")

(def %options ()
  (cons (%default-label) (agent/models)))

(def current ()
  (let ((chosen (agent/patch-model)))
    (if (= chosen "") (%default-label) chosen)))

;; The label the patcher's bubbles show on their header chip. Reads the
;; generation so the chip re-renders after a pick: `agent/patch-model` is a
;; plain native call and is not reactive on its own.
(def current-label ()
  (let ((epoch generation))
    (current)))

(def open ()
  (set! open? true))

(def close ()
  (set! open? false))

;; The M-x entry point. Any zero-arg global `def` is an M-x candidate, so
;; naming it `choose-model` is the whole registration. Inside a module the
;; candidate list shows it qualified (`eseq.choose-model/choose-model`), which
;; still filters on the typed "choose-model".
(def choose-model ()
  (open))

(def select (value)
  (do
    (agent/set-patch-model
      (if (= value (%default-label)) "" value))
    (set! generation (+ generation 1))
    (status (str "Patch agent model: " value))
    (close)))

;; Widget `:key`s auto-qualify inside a declared module (spec §5): these hash
;; as `eseq.choose-model/title`, `…/dropdown`, and so on. Nothing serializes
;; them — they are layout/focus identity only — but tests that look a node up
;; by stable key have to spell the qualified form.
(def %body ()
  ;; Reading the generation here is what re-renders the row after a write.
  (let ((epoch generation))
    (v-stack :width :fill :gap 0.45
      (h-stack :width :fill :gap 0.3 :align :baseline
        (label "Agent Model"
          :key "title"
          :font-size 13 :color :white :bg :transparent)
        (box :flex 1 :bg :transparent)
        (button "x"
          :key "close"
          :font-size 12
          :background-color (rgba 1 1 1 0.05)
          :border-color (rgba 1 1 1 0.14) :color :dim
          :on-click |x y r| (close)))
      (label "Used by the patch editor's agentic bubbles (cmd+k, cmd+shift+k)."
        :key "subtitle"
        :font-size 9 :color :dim :bg :transparent)
      (box :height 0.2)
      (h-stack :width :fill :gap 0.4 :align :center
        (label "Model"
          :key "field-label"
          :font-size 10 :color :dim :bg :transparent)
        (dropdown
          :key "dropdown"
          :value (current)
          :options (%options)
          :width 22.0
          :height 1.35
          :font-size 11
          :on-change (lambda (v) (select v)))
        (box :flex 1 :bg :transparent))
      ;; No escaped quotes here: the lexer does not support \" inside a string
      ;; literal, and silently reads the rest as symbols.
      (label "Default (auto) restores the built-in provider preference."
        :key "hint"
        :font-size 8.5 :color :dim :bg :transparent))))

(def panel ()
  (modal :is-open open?
         :on-close (lambda () (close))
         :width-px 620 :height-px 300
    (box :debug-name "choose-model-panel"
      :width :fill :height :fill :bg :transparent :padding 0.5
      (%body))))
