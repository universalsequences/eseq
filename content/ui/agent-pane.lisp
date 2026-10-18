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

;; ── What the agent built ──
;;
;; One card per package the conversation wrote: its files with lines (or
;; lines changed, for a package that existed), and buttons to add its
;; kinds, load it into the project, rename it (one the agent made) or
;; reject it (back to how it was; one the agent made is deleted).

;; The package whose name is being edited, and the name typed so far.
(defstate renaming "")
(defstate rename-draft "")
;; The package whose Reject waits for a second click.
(defstate confirm-reject "")

(def change-text (file)
  (let ((change (get file :change)))
    (cond
      ((= change "added") (str (get file :lines) " lines"))
      ((= change "deleted") "deleted")
      (else (str "+" (get file :added) " −" (get file :removed))))))

(def summary-text (artifact)
  (let ((count (len (get artifact :files))))
    (str (if (get artifact :created) "new" "edited")
      " · " count (if (= count 1) " file" " files") " · "
      (if (get artifact :created)
        (str (get artifact :lines) " lines")
        (str "+" (get artifact :added) " −" (get artifact :removed))))))

(def open-file (file)
  (host-command "packages-open-source" (dict :path (get file :path))))

(def file-row (file key)
  (h-stack :key key :width :fill :gap 0.5 :align :center
    (if (= (get file :change) "deleted")
      (label (get file :name) :font-size 10 :color :dim :mono true :bg :transparent)
      (label (get file :name) :font-size 10 :color :fg :hover-color :dim :mono true :bg :transparent
        :on-click (lambda (event) (open-file file))))
    (box :flex 1)
    (dim-text (change-text file) 10 false)))

(def start-rename (artifact)
  (if (get artifact :loaded)
    (status "Unload the package to rename it.")
    (do
      (set! rename-draft (get artifact :package))
      (set! renaming (get artifact :package)))))

(def commit-rename ()
  (let ((package renaming) (name rename-draft))
    (set! renaming "")
    (set! confirm-reject "")
    (if (or (= name "") (= name package))
      nil
      (status (agent-pane/rename package name)))))

(def press-reject (artifact)
  (let ((package (get artifact :package)))
    (cond
      ((and (get artifact :created) (get artifact :loaded))
        (status "Unload the package before rejecting it."))
      ((= confirm-reject package)
        (do (set! confirm-reject "") (set! renaming "") (status (agent-pane/reject package))))
      (else (set! confirm-reject package)))))

(def set-loaded (artifact load)
  (for-each (lambda (module)
              (host-command (if load "packages-attach" "packages-detach") (dict :module module)))
    (get artifact :entries)))

(def package-name (artifact key)
  (let ((package (get artifact :package)))
    (if (= renaming package)
      (text-input :key (str key "-rename") :width 12 :height 1.4 :font-size 12
        :value rename-draft :auto-focus true :select-all-on-focus true
        :on-change (lambda (value) (set! rename-draft value))
        :on-submit (lambda () (commit-rename))
        :on-cancel (lambda () (set! renaming "")))
      (if (get artifact :created)
        (label package :key (str key "-name") :font-size 12 :color :fg :hover-color :dim :bold true
          :bg :transparent :on-click (lambda (event) (start-rename artifact)))
        (label package :key (str key "-name") :font-size 12 :color :fg :bold true :bg :transparent)))))

(def small-button (text key variant action)
  (button text :key key :variant variant :height 1.0 :font-size 10 :on-click |x y r| (action)))

(def artifact-card (artifact i)
  (let ((key (str "agent-artifact-" i)) (package (get artifact :package)))
    (box :key key :width :fill :padding 0.4 :corner-radius 6
      :background-color :button-ghost-bg :border-width 1 :border-color :dropdown-menu-border
      (v-stack :width :fill :gap 0.3
        (h-stack :width :fill :gap 0.5 :align :center
          (package-name artifact key)
          (dim-text (summary-text artifact) 10 false)
          (box :flex 1)
          (dim-text (if (get artifact :loaded) "loaded" "") 10 false))
        (v-stack :width :fill :gap 0.05
          (each (get artifact :files) |file j| (file-row file (str key "-file-" j))))
        (wrap :width :fill :gap 0.3 :row-gap 0.3
          (each (get artifact :kinds) |decl j|
            (small-button (str "Add " (get decl :kind)) (str key "-add-" j) :primary
              (lambda () (host-command "packages-new-instance"
                           (dict :module (get decl :module) :kind (get decl :kind))))))
          (if (= (len (get artifact :entries)) 0)
            (nothing)
            (small-button (if (get artifact :loaded) "Unload" "Load") (str key "-load") :ghost
              (lambda () (set-loaded artifact (not (get artifact :loaded))))))
          (if (get artifact :created)
            (small-button "Rename" (str key "-rename-button") :ghost (lambda () (start-rename artifact)))
            (nothing))
          (if (= confirm-reject package)
            (small-button "Really reject?" (str key "-reject") :danger (lambda () (press-reject artifact)))
            (small-button "Reject" (str key "-reject") :ghost (lambda () (press-reject artifact)))))))))

(def artifacts-view (artifacts)
  (if (= (len artifacts) 0)
    (nothing)
    (v-stack :key "agent-pane-artifacts" :width :fill :gap 0.4
      (each artifacts |artifact i| (artifact-card artifact i)))))

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
                (subtree :key (str "agent-entry-" i) (entry-view entry i))))))
        (artifacts-view (get state :artifacts))))))

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
