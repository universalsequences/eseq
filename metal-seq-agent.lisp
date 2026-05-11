;; Agent Mode buffer. Loaded by metal-seq-grid.lisp.

(defstate agent-current-conv 0)
(defstate agent-prompt "")
(defstate agent-finalize-name "")

(def agent-open-instrument ()
  (do
    (if (= agent-current-conv 0)
      (set! agent-current-conv (agent/new :kind 'instrument))
      nil)
    (set-window-buffer-for "*track*" "*agent-artifacts*")
    (switch-to-buffer "*agent*")))

(def agent-close-panel ()
  (do
    (set-window-buffer-for "*agent-artifacts*" "*track*")
    (switch-to-buffer "*metal*")))

(def agent-new-conversation ()
  (do
    (set! agent-current-conv (agent/new :kind 'instrument))
    (set! agent-finalize-name "")))

(def agent-send-current ()
  (if (and (> agent-current-conv 0) (not (= agent-prompt "")))
    (do
      (agent/send agent-current-conv agent-prompt)
      (set! agent-prompt ""))
    nil))

(def agent-status-label ()
  (if (> agent-current-conv 0)
    (str (agent/status agent-current-conv))
    "idle"))

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
                             (if (= role 'system) :widget-bg :buffer-bg))
      (v-stack :width :fill :gap 0.25
        (label (str role)
          :font-size 9
          :color (if (= role 'system) :orange :gray)
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
      (button "Ask agent for an instrument"
        :variant :primary
        :height 1.5
        :on-click |x y r| (agent-open-instrument)))
    (let ((messages (agent/messages agent-current-conv)))
      (if (= (len messages) 0)
        (box :width :fill :flex 1 :align :center :padding 1.0
          (v-stack :gap 0.65 :align :center
            (label "Describe an instrument to generate"
              :font-size 13
              :color :white
              :bg :transparent)
            (button "glassy FM pad with slow attack"
              :variant :ghost
              :on-click |x y r| (set! agent-prompt "a glassy FM pad with slow attack"))))
        (scroll :key (str "agent-scroll-" agent-current-conv)
                :width :fill
                :flex 1
                :stick-to-bottom true
          (v-stack :width :fill :gap 0.45 :padding 0.4
            (each (range 0 (len messages)) |i|
              (agent-message-card (nth messages i) i))))))))

(def agent-draft-actions ()
  (if (and (> agent-current-conv 0) (agent/draft-source agent-current-conv))
    (h-stack :width :fill :gap 0.5 :align :center
      (button "Accept as new track"
        :variant :primary
        :height 1.25
        :on-click |x y r| (agent/accept agent-current-conv))
      (button "Discard"
        :variant :danger
        :height 1.25
        :on-click |x y r| (agent/discard agent-current-conv)))
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
            (label "No instrument artifact yet"
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
      (h-stack :width :fill :gap 0.5 :align :center
        (text-input
          :value agent-prompt
          :placeholder "Describe an instrument..."
          :flex 1
          :height 1.4
          :on-change (lambda (v) (set! agent-prompt v)))
        (button "Send"
          :variant :primary
          :height 1.4
          :on-click |x y r| (agent-send-current))
        (button "Cancel"
          :variant :danger
          :height 1.4
          :on-click |x y r| (if (> agent-current-conv 0) (agent/cancel agent-current-conv) nil))))))

(effect-buffer "*agent-artifacts*"
  (let ((agent-generation AGENT.generation))
    (agent-artifact-panel)))

(bind-key "C-g" "agent-open-instrument")
