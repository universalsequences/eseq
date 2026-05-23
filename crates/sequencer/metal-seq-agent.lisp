;; Agent Mode buffer. Loaded by metal-seq-grid.lisp.

(defstate agent-current-conv 0)
(defstate agent-prompt "")
(defstate agent-finalize-name "")

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

(def agent-open-instrument ()
  (agent-open))

(def agent-close-panel ()
  (do
    (set-window-buffer-for "*agent-artifacts*" "*track*")
    (switch-to-buffer "*metal*")))

(def agent-new-conversation ()
  (do
    (set! agent-current-conv (agent/new :kind 'general))
    (set! agent-finalize-name "")))

(def agent-send-current ()
  (if (and (> agent-current-conv 0) (not (= agent-prompt "")))
    (do
      (agent/send agent-current-conv agent-prompt)
      (set! agent-prompt ""))
    nil))

(def agent-submit-current ()
  (if (agent-busy?)
    (if (> agent-current-conv 0)
      (agent/cancel agent-current-conv)
      nil)
    (agent-send-current)))

(def agent-status-label ()
  (if (> agent-current-conv 0)
    (str (agent/status agent-current-conv))
    "idle"))

(def agent-busy? ()
  (if (> agent-current-conv 0)
    (let ((status (agent/status agent-current-conv)))
      (or (= status 'streaming)
          (= status 'compiling)
          (= status 'auditioning)))
    false))

(def agent-current-model ()
  (if (> agent-current-conv 0)
    (agent/model agent-current-conv)
    (nth (agent/models) 0)))

(def agent-set-current-model (model)
  (if (> agent-current-conv 0)
    (agent/set-model agent-current-conv model)
    nil))

(def agent-message-card (m i)
  (let ((role (get m :role))
        (text (get m :display-text))
        (has-code (get m :has-code-blocks)))
    (box :key (str "agent-message-" i)
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

(def agent-message-list ()
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
              :on-click |x y r| (set! agent-prompt "create a tape delay effect"))))
        (scroll :key (str "agent-scroll-" agent-current-conv)
                :width :fill
                :flex 1
                :stick-to-bottom true
          (virtual-v-stack
            :key "agent-message-stack"
            :width :fill
            :gap 0.45
            :padding 0.4
            :estimated-item-height 4.0
            :overscan 6
            (each (range 0 (len messages)) |i|
              (agent-message-card (nth messages i) i))))))))

(def agent-draft-actions ()
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

(def agent-artifact-finalize-name (artifact)
  (if (= agent-finalize-name "")
    (str (get artifact :display-name))
    agent-finalize-name))

(def agent-finalize-current (artifact)
  (if (and (> agent-current-conv 0) (get artifact :can-finalize))
    (agent/finalize agent-current-conv (agent-artifact-finalize-name artifact))
    nil))

(def agent-artifact-panel ()
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
              :value agent-finalize-name
              :placeholder (str (get artifact :display-name))
              :width :fill
              :height 1.35
              :on-change (lambda (v) (set! agent-finalize-name v)))
            (if (get artifact :can-finalize)
              (button "Finalize"
                :variant :primary
                :width :fill
                :height 1.35
                :on-click |x y r| (agent-finalize-current artifact))
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
        (label (agent-status-label)
          :font-size 10
          :color :blue
          :bg :transparent)
        (box :flex 1)
        (button "New"
          :variant :ghost
          :height 1.2
          :on-click |x y r| (agent-new-conversation))
        (button "Back"
          :variant :ghost
          :height 1.2
          :on-click |x y r| (agent-close-panel)))
      (agent-message-list)
      (agent-draft-actions)
      (box :key "agent-composer"
        :width :fill
        :padding 0.65
        :corner-radius 34
        :background-color :button-ghost-bg
        :border-width 1
        :border-color :dropdown-menu-border
        :align :stretch
        (v-stack :width :fill :gap 0.25
          (textbox
            :key "agent-prompt-input"
            :value agent-prompt
            :placeholder "Describe an instrument, effect, or change..."
            :width :fill
            :min-lines 2
            :max-lines 7
            :font-size 13
            :bg :transparent
            :on-change (lambda (v) (set! agent-prompt v)))
          (box :flex 1)
          (h-stack :key "agent-composer-actions"
            :padding 0.5
            :width :fill
            :gap 0.5
            :align :end
            (dropdown
              :key "agent-model-select"
              :value (agent-current-model)
              :options (agent/models)
              :width 14.0
              :height 1.35
              :font-size 11
              :on-change (lambda (v) (agent-set-current-model v)))
            (box :flex 1)
            (box :key "agent-submit"
              :width 3.4
              :height 1.34
              :h-align :center
              :v-align :end
              :on-click |x y r| (agent-submit-current)
              (agent-submit-icon
                :on-click |x y r| (agent-submit-current)
                :active (if (or (agent-busy?) (not (= agent-prompt ""))) 1 0)
                :canceling (if (agent-busy?) 1 0)))))))))

(effect-buffer "*agent-artifacts*"
  (let ((agent-generation AGENT.generation))
    (agent-artifact-panel)))

(bind-key "C-g" "agent-open")
