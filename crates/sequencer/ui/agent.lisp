;; ui/agent.lisp — Agent Mode: the *agent* conversation buffer and its
;; *agent-artifacts* side panel. Loaded by ui/main.lisp.
;;
;; MODULE NOTE (spec §10, S3b): this file is a RENDER ROOT — it registers two
;; effect-buffers at top level. `import` EVALUATES its target, so NEVER import
;; this module from a library file; that would drag a UI root into every VM that
;; loads the importer.
;;
;; It adds NO imports of its own, and must not grow any: five Rust tests in
;; src/ui/state_values/tests.rs eval this whole file into a bare
;; `Runtime::new()` with no `@/` source root (hazard n2), where an `import`
;; would resolve through `module_file_candidates`, fall through to a
;; cwd-relative path that does not exist, and push a load error into every one
;; of those VMs. Nothing here needs one: everything it touches outside the file
;; is a Rust native (the `agent/…` conversation API — an undotted slash
;; namespace that resolves flat, exactly as it does in eseq.choose-model), a
;; builtin widget, or the AGENT reactive namespace, none of which are
;; module-scoped.
;;
;; Widget `:key` props auto-qualify (hazard a), so the hand-rolled `agent-`
;; prefix is dropped from every key string here and the layout assertions in
;; the Rust tests moved to the `/`-suffix matcher. The `*agent*` /
;; `*agent-artifacts*` buffer names and the `agent-submit-icon` `defwidget`
;; name are flat keyspaces and stay byte-identical (hazard e).
(module eseq.agent)

;; Compat aliases (spec §10 step 2) — identity, one per name with a caller
;; outside this file. Nothing is renamed: every one of these is spelled flat
;; from somewhere that cannot import a render root.
;;
;; `agent-open` — the Rust agent tests eval `(agent-open)` by flat name.
;; `agent-submit-current` — the busy/cancel test evals it by flat name.
;; `agent-current-conv` — the same tests seed it with a flat
;;   `(set! agent-current-conv 1)`. It is a `defstate`, so the write lands in
;;   `state_bindings`, whose lookup ladder honours this alias too (hazard b);
;;   the writers are `#[cfg(test)]` only, so no `eseq.vanilla` pin is needed.
;;
;; All three are functions or a `defstate`, i.e. the two shapes immune to
;; hazard (m). This file has no mutable plain `def` and no outbound `set!` —
;; every `set!` below targets one of its own `defstate`s (hazard j clear).

(defstate agent-current-conv 0)
(defstate %prompt "")
(defstate %finalize-name "")

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

(def agent-open ()
  (do
    (if (= agent-current-conv 0)
      (set! agent-current-conv (agent/new :kind 'general))
      nil)
    (set-window-buffer-for "*track*" "*agent-artifacts*")
    (switch-to-buffer "*agent*")))

(def %close-panel ()
  (do
    (set-window-buffer-for "*agent-artifacts*" "*track*")
    ;; *metal* (legacy step grid) is no longer loaded; land on the live view.
    (switch-to-buffer "*sequencer*")))

(def %new-conversation ()
  (do
    (set! agent-current-conv (agent/new :kind 'general))
    (set! %finalize-name "")))

(def %send-current ()
  (if (and (> agent-current-conv 0) (not (= %prompt "")))
    (do
      (agent/send agent-current-conv %prompt)
      (set! %prompt ""))
    nil))

(def agent-submit-current ()
  (if (%busy?)
    (if (> agent-current-conv 0)
      (agent/cancel agent-current-conv)
      nil)
    (%send-current)))

(def %status-label ()
  (if (> agent-current-conv 0)
    (str (agent/status agent-current-conv))
    "idle"))

(def %busy? ()
  (if (> agent-current-conv 0)
    (let ((status (agent/status agent-current-conv)))
      (or (= status 'streaming)
          (= status 'compiling)
          (= status 'auditioning)))
    false))

(def %current-model ()
  (if (> agent-current-conv 0)
    (agent/model agent-current-conv)
    (nth (agent/models) 0)))

(def %set-current-model (model)
  (if (> agent-current-conv 0)
    (agent/set-model agent-current-conv model)
    nil))

(def %message-card (m i)
  (let ((role (get m :role))
        (text (get m :display-text))
        (has-code (get m :has-code-blocks)))
    (box :key (str "message-" i)
         :width :fill
         :padding 0.55
         :corner-radius 7
         :background-color (if (= role 'user)
                             :button-ghost-bg
                             (if (= role 'tool) :widget-bg
                               (if (= role 'system) :widget-bg :buffer-bg)))
      (v-stack :width :fill :gap 0.25
        (label (str role)
          :font-size 9
          :color (if (= role 'tool)
                   :blue
                   (if (= role 'system) :orange :gray))
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

(def %message-list ()
  (if (= agent-current-conv 0)
    (box :width :fill :flex 1 :align :center
      (button "Ask agent"
        :variant :primary
        :height 1.5
        :on-click |x y r| (agent-open)))
    (let ((messages (agent/messages agent-current-conv)))
      (if (= (len messages) 0)
        (box :width :fill :flex 1 :align :center :padding 1.0
          (v-stack :gap 0.65 :align :center
            (label "Describe an instrument, effect, or edit"
              :font-size 13
              :color :white
              :bg :transparent)
            (button "tape delay effect"
              :variant :ghost
              :on-click |x y r| (set! %prompt "create a tape delay effect"))))
        (scroll :key (str "scroll-" agent-current-conv)
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
              (%message-card (nth messages i) i))))))))

(def %draft-actions ()
  (if (> agent-current-conv 0)
    (let ((artifact (agent/artifact agent-current-conv)))
      (if (get artifact :can-apply)
        (h-stack :width :fill :gap 0.5 :align :center
          (button (str (get artifact :apply-label))
            :variant :primary
            :height 1.25
            :on-click |x y r| (agent/accept agent-current-conv))
          (button "Discard"
            :variant :danger
            :height 1.25
            :on-click |x y r| (agent/discard agent-current-conv)))
        (box :height 0.1)))
    (box :height 0.1)))

(def %artifact-finalize-name (artifact)
  (if (= %finalize-name "")
    (str (get artifact :display-name))
    %finalize-name))

(def %finalize-current (artifact)
  (if (and (> agent-current-conv 0) (get artifact :can-finalize))
    (agent/finalize agent-current-conv (%artifact-finalize-name artifact))
    nil))

(def %artifact-panel ()
  (if (= agent-current-conv 0)
    (box :width :fill :height :fill :padding 0.8
      (label "No artifact" :color :gray :bg :transparent))
    (let ((artifact (agent/artifact agent-current-conv)))
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
              :value %finalize-name
              :placeholder (str (get artifact :display-name))
              :width :fill
              :height 1.35
              :on-change (lambda (v) (set! %finalize-name v)))
            (if (get artifact :can-finalize)
              (button "Finalize"
                :variant :primary
                :width :fill
                :height 1.35
                :on-click |x y r| (%finalize-current artifact))
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

(effect-buffer "*agent*"
  (let ((agent-generation AGENT.generation))
    (v-stack :width :fill :height :fill :gap 0.5 :padding 0.65
      (h-stack :width :fill :align :center :gap 0.5
        (label "Agent"
          :font-size 15
          :color :white
          :bg :transparent)
        (label (%status-label)
          :font-size 10
          :color :blue
          :bg :transparent)
        (box :flex 1)
        (button "New"
          :variant :ghost
          :height 1.2
          :on-click |x y r| (%new-conversation))
        (button "Back"
          :variant :ghost
          :height 1.2
          :on-click |x y r| (%close-panel)))
      (%message-list)
      (%draft-actions)
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
            :value %prompt
            :placeholder "Describe an instrument, effect, or change..."
            :width :fill
            :min-lines 2
            :max-lines 7
            :font-size 13
            :bg :transparent
            :on-change (lambda (v) (set! %prompt v)))
          (box :flex 1)
          (h-stack :key "composer-actions"
            :padding 0.5
            :width :fill
            :gap 0.5
            :align :end
            (dropdown
              :key "model-select"
              :value (%current-model)
              :options (agent/models)
              :width 14.0
              :height 1.35
              :font-size 11
              :on-change (lambda (v) (%set-current-model v)))
            (box :flex 1)
            (box :key "submit"
              :width 3.4
              :height 1.34
              :h-align :center
              :v-align :end
              :on-click |x y r| (agent-submit-current)
              (agent-submit-icon
                :on-click |x y r| (agent-submit-current)
                :active (if (or (%busy?) (not (= %prompt ""))) 1 0)
                :canceling (if (%busy?) 1 0)))))))))

(effect-buffer "*agent-artifacts*"
  (let ((agent-generation AGENT.generation))
    (%artifact-panel)))
