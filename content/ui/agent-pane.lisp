;; ui/agent-pane.lisp — the agent pane: Claude in a side pane beside the
;; project, building sequencers and extensions with the authoring tools
;; (bead eseq-maxc.6). The conversation lives in the host (the agent-pane/…
;; natives); this draws it, and redraws as agent.pane-generation moves while
;; a turn streams. Open it with C-x C-a or the "Agent Pane" command.
;;
;; MODULE NOTE: a render root (it registers its buffer and a global key), so
;; only ui/main.lisp imports it.

(module eseq.agent-pane)

(import eseq.kinds :refer (agent))
(import eseq.markdown-view :as markdown-view)
(import eseq.view-kit :refer (nothing))

(export agent-pane-open agent-pane-close agent-pane-send)

(def buffer-name "*agent-pane*")

;; The composer's text.
(defstate draft "")

;; Open the agent pane beside the project.
(def agent-pane-open ()
  :doc "Talk to Claude in a side pane: it builds sequencers and extensions with the authoring tools."
  :command "Agent Pane"
  :group "View"
  (show-in-side-pane buffer-name))

;; Close the agent pane (the conversation stays).
(def agent-pane-close ()
  (close-window-for buffer-name))

;; Send the composer's text to the agent.
(def agent-pane-send ()
  (if (= draft "")
    nil
    (if (agent-pane/busy?)
      (status "The agent is still working; stop it or wait.")
      (let ((text draft))
        (set! draft "")
        (agent-pane/send text)))))

;; Secondary text: prefixes, tool rows, the counters.
(def dim-text (text size wrap)
  (label text :font-size size :color :dim :wrap wrap :bg :transparent))

(def tool-view (entry i)
  (let ((status (get entry :status)))
    (v-stack :key (str "entry-" i) :width :fill :gap 0.15
      (h-stack :width :fill :gap 0.5
        (dim-text (str "• " (get entry :name) (cond ((= status "failed") " failed") ((= status "running") " …") (else "")))
          11 false)
        (dim-text (get entry :input) 10 false))
      (if (= (get entry :output) "")
        (nothing)
        (dim-text (get entry :output) 10 true))
      (v-stack :width :fill :gap 0.2
        (each (get entry :images) |path j|
          (image :key (str "entry-" i "-image-" j) :src path :width :fill :height 14))))))

(def text-view (entry i prefix color size)
  (label (str prefix (get entry :text))
    :key (str "entry-" i) :font-size size :color color :wrap true :bg :transparent))

(def open-link (target)
  (if (markdown-view/external-link? target)
    (markdown-view/open-external target)
    (status (str "Not a web link: " target))))

;; The agent's replies are markdown: bold, code, lists, headings.
(def reply-view (entry i)
  (box :key (str "entry-" i) :width :fill
    (markdown-view/render-markdown (get entry :text) (dict :size 12 :on-link open-link))))

(def entry-view (entry i)
  (let ((kind (get entry :kind)))
    (cond
      ((= kind "tool") (tool-view entry i))
      ((= kind "user") (text-view entry i "› " :fg 12))
      ((= kind "thinking") (text-view entry i "… " :dim 10))
      ((= kind "notice") (text-view entry i "! " :fg 11))
      (else (reply-view entry i)))))

(def header (busy usage)
  (h-stack :key "agent-pane-header" :width :fill :gap 0.5 :align :center
    (label "Agent" :font-size 14 :color :fg :bg :transparent)
    (dim-text (if busy "working…" "") 10 false)
    (box :flex 1)
    (dim-text (str (get usage :input) " in · " (get usage :output) " out") 9 false)
    (if busy
      (button "Stop" :key "agent-pane-stop" :variant :danger :height 1.2
        :on-click |x y r| (agent-pane/cancel))
      (button "New" :key "agent-pane-new" :variant :ghost :height 1.2
        :on-click |x y r| (agent-pane/reset)))))

(def empty-state ()
  (v-stack :key "agent-pane-empty" :width :fill :flex 1 :gap 0.6 :padding 1.0
    (dim-text "Ask for a sequencer, a panel or an extension. The agent writes it as a Local module, checks how it plays and looks, and your project picks it up."
      11 true)
    (button "a euclidean sequencer with three rings" :key "agent-pane-example" :variant :ghost
      :on-click |x y r| (set! draft "Build a euclidean rhythm sequencer with three rings, each routed to a track."))))

;; The conversation, redrawn as agent.pane-generation moves while a turn
;; streams (and not as the composer is typed into).
(def conversation ()
  ;; Reading the generation is what ties this subtree to the conversation.
  (let ((generation agent.pane-generation)
        (state (agent-pane/state)))
    (let ((entries (get state :entries)))
      (v-stack :width :fill :flex 1 :gap 0.5
        (header (get state :busy) (get state :usage))
        (if (= (len entries) 0)
          (empty-state)
          (scroll :key "agent-pane-scroll" :width :fill :flex 1 :stick-to-bottom true
            (v-stack :key "agent-pane-entries" :width :fill :gap 0.5 :padding 0.3
              ;; One subtree per entry: while a reply streams only it redraws,
              ;; the finished ones are reused.
              (each entries |entry i|
                (subtree :key (str "agent-entry-" i) (entry-view entry i))))))))))

(def composer ()
  (box :key "agent-pane-composer" :width :fill :padding 0.5 :corner-radius 8
    :background-color :button-ghost-bg :border-width 1 :border-color :dropdown-menu-border
    (v-stack :width :fill :gap 0.3
      (textbox :key "agent-pane-input" :value draft :placeholder "Ask the agent… (C-c C-c sends)"
        :width :fill :min-lines 2 :max-lines 8 :font-size 12 :bg :transparent
        :on-change (lambda (v) (set! draft v)))
      (h-stack :width :fill :gap 0.5
        (box :flex 1)
        (button "Send" :key "agent-pane-send" :variant :primary :height 1.2
          :on-click |x y r| (agent-pane-send))))))

(effect-buffer "*agent-pane*"
  (box :width :fill :height :fill :background-color :buffer-bg
    (v-stack :width :fill :height :fill :gap 0.5 :padding 0.6
      (subtree :key "agent-pane-conversation" (conversation))
      (subtree :key "agent-pane-composer-tree" (composer)))))

(define-mode "eseq.agent-pane/agent-pane-mode" :read-only true)
(mode-bind-key "eseq.agent-pane/agent-pane-mode" "C-c C-c" "eseq.agent-pane/agent-pane-send")
(mode-bind-key "eseq.agent-pane/agent-pane-mode" "q" "eseq.agent-pane/agent-pane-close")
(set-buffer-mode-for "*agent-pane*" "eseq.agent-pane/agent-pane-mode")
(bind-key "C-x C-a" "agent-pane-open")
