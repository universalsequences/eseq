;; ui/agent.lisp — Agent Mode: the *agent* conversation buffer and its
;; *agent-artifacts* side panel. Loaded by ui/main.lisp.
;;
;; MODULE NOTE (spec §10, S3b): this file is a RENDER ROOT — it registers two
;; effect-buffers and a global key binding at top level. `import` EVALUATES its
;; target, so NEVER import this module from a library file; that would drag a
;; UI root into every VM that loads the importer.
;;
;; Its one import is `eseq.kinds`, for the host's `agent` singleton: both
;; buffers read `agent.generation`, which moves whenever an agent session
;; changes, so they re-render then. Everything else it touches outside the
;; file is a Rust native (the `agent/…` conversation API — an undotted slash
;; namespace that resolves flat, exactly as it does in eseq.choose-model) or a
;; builtin widget, none of which are module-scoped.
;;
;; Widget `:key` props auto-qualify (hazard a), so the hand-rolled `agent-`
;; prefix is dropped from every key string here and the layout assertions in
;; the Rust tests moved to the `/`-suffix matcher. The `*agent*` /
;; `*agent-artifacts*` buffer names and the `agent-submit-icon` `defwidget`
;; name are flat keyspaces and stay byte-identical (hazard e).
(module eseq.agent)
(import eseq.kinds :refer (agent))

(export agent-chat
        agent-open
        agent-submit-current)

;; Compat aliases (spec §10 step 2) — identity, one per name with a caller
;; outside this file. Nothing is renamed: every one of these is spelled flat
;; from somewhere that cannot import a render root.
;;
;; `agent-open` — the Rust agent tests eval `(agent-open)` by flat name (it is
;;   also this file's own `C-x a` bind-key handler, which needs no alias:
;;   bind-key qualifies the handler string against the binding module and
;;   finds it exactly here).
;; `agent-submit-current` — the busy/cancel test evals it by flat name.

;; The open conversation (0: none), the composer's draft and the artifact's
;; save-as name.
(def-kind agent-chat
  :key ()
  :state ((conv 0)
          (prompt "")
          (finalize-name "")))

(defwidget agent-submit-icon
  :width 3.2 :height 3.2
  :paint-margin 0.3
  :state (active canceling)
  :shader
  (let ((bg-col (if (> (+ active canceling) 0)
                  (rgba 0.98 0.98 0.99 1.0)
                  (rgba 0.90 0.91 0.93 1.0)))
        (arrow-col (rgba 0.07 0.075 0.085 (- 1.0 canceling)))
        (stop-col (rgba 0.07 0.075 0.085 canceling)))
    (sdf/layer
      (sdf/fill (sdf/circle 0.92)
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 0.14
          (sdf/rounded-rect 0.05 0.46 0.025))
        (material :color arrow-col))
      (sdf/fill
        (let ((clip (max (- (abs x) 0.58) (- (abs y) 0.62)))
              (left (max (sdf/line -0.38 -0.08 0.0 -0.42) clip))
              (right (max (sdf/line 0.38 -0.08 0.0 -0.42) clip)))
          (- (min left right) 0.045))
        (material :color arrow-col))
      (sdf/fill
        (sdf/rounded-rect 0.34 0.34 0.04)
        (material :color stop-col)))))

(def chatting? () (> agent-chat.conv 0))

(def agent-open ()
  (unless (chatting?)
    (new-conversation))
  (when (chatting?)
    (set-window-buffer-for "*track*" "*agent-artifacts*")
    (switch-to-buffer "*agent*")))

(def close-panel ()
  (set-window-buffer-for "*agent-artifacts*" "*track*")
  ;; *metal* (legacy step grid) is no longer loaded; land on the live view.
  (switch-to-buffer "*sequencer*"))

(def new-conversation ()
  (let ((id (agent/new :kind 'general)))
    (when id
      (set! agent-chat.conv id)
      (set! agent-chat.finalize-name ""))))

(def send-current ()
  (when (and (chatting?) (not (= agent-chat.prompt "")))
    (agent/send agent-chat.conv agent-chat.prompt)
    (set! agent-chat.prompt "")))

(def agent-submit-current ()
  (if (busy?)
    (when (chatting?)
      (agent/cancel agent-chat.conv))
    (send-current)))

(def status-label ()
  (if (chatting?)
    (str (agent/status agent-chat.conv))
    "idle"))

(def busy? ()
  (if (chatting?)
    (let ((status (agent/status agent-chat.conv)))
      (or (= status 'streaming)
          (= status 'compiling)
          (= status 'auditioning)))
    false))

(def model-options ()
  (let ((models (agent/models)))
    (if (= models false) (list) models)))

(def current-model ()
  (if (chatting?)
    (agent/model agent-chat.conv)
    (let ((models (model-options)))
      (if models (nth models 0) ""))))

(def set-current-model (model)
  (when (chatting?)
    (agent/set-model agent-chat.conv model)))

(def message-card (m i)
  (let ((role (get m :role))
        (text (get m :display-text))
        (has-code (get m :has-code-blocks)))
    (box :key (str "message-" i)
         :width :fill
         :padding 0.55
         :corner-radius 7
         :background-color (cond
                             ((= role 'user) :button-ghost-bg)
                             ((= role 'tool) :widget-bg)
                             ((= role 'system) :widget-bg)
                             (else :buffer-bg))
      (v-stack :width :fill :gap 0.25
        (label (str role)
          :font-size 9
          :color (cond
                   ((= role 'tool) :blue)
                   ((= role 'system) :orange)
                   (else :gray))
          :bg :transparent)
        (label text
          :font-size 11
          :wrap true
          :color :white
          :bg :transparent)
        (if has-code
          (label "full source captured in artifact/debug logs"
            :font-size 9
            :color :gray
            :wrap true
            :bg :transparent)
          (box :height 0.0))))))

(def message-list ()
  (if (not (chatting?))
    (box :width :fill :flex 1 :align :center
      (button "Ask agent"
        :variant :primary
        :height 1.5
        :on-click |x y r| (agent-open)))
    (let ((messages (agent/messages agent-chat.conv)))
      (if (= (len messages) 0)
        (box :width :fill :flex 1 :align :center :padding 1.0
          (v-stack :gap 0.65 :align :center
            (label "Describe an instrument, effect, or edit"
              :font-size 13
              :color :white
              :bg :transparent)
            (button "tape delay effect"
              :variant :ghost
              :on-click |x y r| (set! agent-chat.prompt "create a tape delay effect"))))
        (scroll :key (str "scroll-" agent-chat.conv)
                :width :fill
                :flex 1
                :stick-to-bottom true
          (virtual-v-stack
            :key "message-stack"
            :width :fill
            :gap 0.45
            :padding 0.4
            :estimated-item-height 4.0
            :overscan 6
            (each (range 0 (len messages)) |i|
              (message-card (nth messages i) i))))))))

(def draft-actions ()
  (if (chatting?)
    (let ((artifact (agent/artifact agent-chat.conv)))
      (if (get artifact :can-apply)
        (h-stack :width :fill :gap 0.5 :align :center
          (button (str (get artifact :apply-label))
            :variant :primary
            :height 1.25
            :on-click |x y r| (agent/accept agent-chat.conv))
          (button "Discard"
            :variant :danger
            :height 1.25
            :on-click |x y r| (agent/discard agent-chat.conv)))
        (box :height 0.1)))
    (box :height 0.1)))

(def artifact-finalize-name (artifact)
  (if (= agent-chat.finalize-name "")
    (str (get artifact :display-name))
    agent-chat.finalize-name))

(def finalize-current (artifact)
  (when (and (chatting?) (get artifact :can-finalize))
    (agent/finalize agent-chat.conv (artifact-finalize-name artifact))))

(def artifact-panel ()
  (if (not (chatting?))
    (box :width :fill :height :fill :padding 0.8
      (label "No artifact" :color :gray :bg :transparent))
    (let ((artifact (agent/artifact agent-chat.conv)))
      (if (get artifact :exists)
        (v-stack :width :fill :height :fill :gap 0.75 :padding 0.8
          (v-stack :width :fill :gap 0.2
            (label "Artifact"
              :font-size 12
              :color :white
              :bg :transparent)
            (label (str (get artifact :display-name))
              :font-size 14
              :color :white
              :wrap true
              :bg :transparent)
            (label (str (get artifact :status))
              :font-size 10
              :color :blue
              :bg :transparent))
          (v-stack :width :fill :gap 0.25
            (label "Track"
              :font-size 9
              :color :gray
              :bg :transparent)
            (label (if (get artifact :track)
                     (str "track " (get artifact :track))
                     "not loaded yet")
              :font-size 11
              :color :white
              :wrap true
              :bg :transparent))
          (v-stack :width :fill :gap 0.35
            (label "Save as"
              :font-size 9
              :color :gray
              :bg :transparent)
            (text-input
              :value agent-chat.finalize-name
              :placeholder (str (get artifact :display-name))
              :width :fill
              :height 1.35
              :on-change (lambda (v) (set! agent-chat.finalize-name v)))
            (if (get artifact :can-finalize)
              (button "Finalize"
                :variant :primary
                :width :fill
                :height 1.35
                :on-click |x y r| (finalize-current artifact))
              (button "Finalized"
                :variant :ghost
                :width :fill
                :height 1.35
                :on-click |x y r| nil)))
          (box :flex 1))
        (box :width :fill :height :fill :padding 0.8
          (v-stack :width :fill :gap 0.35
            (label "Artifact"
              :font-size 12
              :color :white
              :bg :transparent)
            (label "No artifact yet"
              :font-size 11
              :color :gray
              :wrap true
              :bg :transparent)))))))

;; Widget-only buffer: take the shared sequencer keymap (was an implicit host default).
(set-buffer-mode-for "*agent*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*agent*"
  (let ((agent-generation agent.generation))
    (v-stack :width :fill :height :fill :gap 0.5 :padding 0.65
      (h-stack :width :fill :align :center :gap 0.5
        (label "Agent"
          :font-size 15
          :color :white
          :bg :transparent)
        (label (status-label)
          :font-size 10
          :color :blue
          :bg :transparent)
        (box :flex 1)
        (button "New"
          :variant :ghost
          :height 1.2
          :on-click |x y r| (new-conversation))
        (button "Back"
          :variant :ghost
          :height 1.2
          :on-click |x y r| (close-panel)))
      (message-list)
      (draft-actions)
      (box :key "composer"
        :width :fill
        :padding 0.65
        :corner-radius 34
        :background-color :button-ghost-bg
        :border-width 1
        :border-color :dropdown-menu-border
        :align :stretch
        (v-stack :width :fill :gap 0.25
          (textbox
            :key "prompt-input"
            :value agent-chat.prompt
            :placeholder "Describe an instrument, effect, or change..."
            :width :fill
            :min-lines 2
            :max-lines 7
            :font-size 13
            :bg :transparent
            :on-change (lambda (v) (set! agent-chat.prompt v)))
          (box :flex 1)
          (h-stack :key "composer-actions"
            :padding 0.5
            :width :fill
            :gap 0.5
            :align :end
            (dropdown
              :key "model-select"
              :value (current-model)
              :options (model-options)
              :width 14.0
              :height 1.35
              :font-size 11
              :on-change (lambda (v) (set-current-model v)))
            (box :flex 1)
            (box :key "submit"
              :width 3.4
              :height 1.34
              :h-align :center
              :v-align :end
              :on-click |x y r| (agent-submit-current)
              (agent-submit-icon
                :on-click |x y r| (agent-submit-current)
                :active (if (or (busy?) (not (= agent-chat.prompt ""))) 1 0)
                :canceling (if (busy?) 1 0)))))))))

;; Widget-only buffer: take the shared sequencer keymap (was an implicit host default).
(set-buffer-mode-for "*agent-artifacts*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*agent-artifacts*"
  (let ((agent-generation agent.generation))
    (artifact-panel)))

;; Entry point. `C-g` used to open this panel, but Cmd/Ctrl+G is now the
;; track-group shortcut (eseq.mixer/seq-ctrl-g), so Agent Mode moves to the
;; `C-x <letter>` panel-toggle family alongside `C-x m` (patch macros),
;; `C-x p` (packages) and `C-x s` (sample browser). The handler string
;; qualifies against this module, which is where `agent-open` lives.
(bind-key "C-x a" "agent-open")
