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
;; v-stack with `(choose-model-panel)`. main.lisp only guarantees this file
;; loads before those buffers exist. The panel lives in the patcher's own
;; buffer because a modal only receives pointer input through the *active*
;; tile, and the patch canvas is the active tile; closed, it costs no layout.
;;
;; The two mounts share the single global `choose-model-open?` below, which is
;; safe because an instrument patcher buffer and an effect patcher buffer are
;; never on screen together — entering either edit session replaces the other's
;; tile. Adding a third mount in a buffer that CAN coexist with a patcher would
;; render two panels off one flag; scope the state per buffer first if so.

;; Modal visibility is pure UI state, so it lives here rather than in Rust.
(defstate choose-model-open? false)
;; Bumped on every write so the panel re-reads `agent/patch-model`, which is a
;; plain native call and therefore not reactive on its own.
(defstate choose-model-generation 0)

;; The sentinel row standing in for "no explicit choice" — the bubbles then
;; fall back to their built-in provider preference.
(def choose-model-default-label () "Default (auto)")

(def choose-model-options ()
  (cons (choose-model-default-label) (agent/models)))

(def choose-model-current ()
  (let ((chosen (agent/patch-model)))
    (if (= chosen "") (choose-model-default-label) chosen)))

;; The label the patcher's bubbles show on their header chip. Reads the
;; generation so the chip re-renders after a pick: `agent/patch-model` is a
;; plain native call and is not reactive on its own.
(def choose-model-current-label ()
  (let ((generation choose-model-generation))
    (choose-model-current)))

(def choose-model-open ()
  (set! choose-model-open? true))

(def choose-model-close ()
  (set! choose-model-open? false))

;; The M-x entry point. Any zero-arg global `def` is an M-x candidate, so
;; naming it `choose-model` is the whole registration.
(def choose-model ()
  (choose-model-open))

(def choose-model-select (value)
  (do
    (agent/set-patch-model
      (if (= value (choose-model-default-label)) "" value))
    (set! choose-model-generation (+ choose-model-generation 1))
    (status (str "Patch agent model: " value))
    (choose-model-close)))

(def choose-model-body ()
  ;; Reading the generation here is what re-renders the row after a write.
  (let ((generation choose-model-generation))
    (v-stack :width :fill :gap 0.45
      (h-stack :width :fill :gap 0.3 :align :baseline
        (label "Agent Model"
          :key "choose-model-title"
          :font-size 13 :color :white :bg :transparent)
        (box :flex 1 :bg :transparent)
        (button "x"
          :key "choose-model-close"
          :font-size 12
          :background-color (rgba 1 1 1 0.05)
          :border-color (rgba 1 1 1 0.14) :color :dim
          :on-click |x y r| (choose-model-close)))
      (label "Used by the patch editor's agentic bubbles (cmd+k, cmd+shift+k)."
        :key "choose-model-subtitle"
        :font-size 9 :color :dim :bg :transparent)
      (box :height 0.2)
      (h-stack :width :fill :gap 0.4 :align :center
        (label "Model"
          :key "choose-model-field-label"
          :font-size 10 :color :dim :bg :transparent)
        (dropdown
          :key "choose-model-dropdown"
          :value (choose-model-current)
          :options (choose-model-options)
          :width 22.0
          :height 1.35
          :font-size 11
          :on-change (lambda (v) (choose-model-select v)))
        (box :flex 1 :bg :transparent))
      ;; No escaped quotes here: the lexer does not support \" inside a string
      ;; literal, and silently reads the rest as symbols.
      (label "Default (auto) restores the built-in provider preference."
        :key "choose-model-hint"
        :font-size 8.5 :color :dim :bg :transparent))))

(def choose-model-panel ()
  (modal :is-open choose-model-open?
         :on-close (lambda () (choose-model-close))
         :width-px 620 :height-px 300
    (box :debug-name "choose-model-panel"
      :width :fill :height :fill :bg :transparent :padding 0.5
      (choose-model-body))))
