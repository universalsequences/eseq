;; ui/agent-pane.lisp — the agent pane: Claude in a side pane beside the
;; project, building sequencers and extensions with the authoring tools
;; (bead eseq-maxc.6). The conversation lives in the host (the agent-pane/…
;; natives); this draws it, redrawing its rows as agent.pane-shape moves and
;; the last row as agent.pane-generation moves while a turn streams. Open it
;; with C-x C-a or the "Agent Pane" command.
;;
;; MODULE NOTE: a render root (it registers its buffer and a global key), so
;; only ui/main.lisp imports it.

(module eseq.agent-pane)

(import eseq.kinds :refer (agent project))
(import eseq.markdown-view :as markdown-view)
(import eseq.view-kit :refer (nothing open-menu! menu-of find))
(import eseq.customize :refer (knob-editor short-name set-knob customize-dirty customize-save))

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

;; Send the composer's text, and the images attached to it, to the agent.
(def agent-pane-send ()
  (if (and (= draft "") (empty? (agent-pane/attachments)))
    nil
    (if (agent-pane/busy?)
      (status "The agent is still working; stop it or wait.")
      (let ((text draft))
        (set! draft "")
        (agent-pane/send text)))))

;; Secondary text: tool rows, the counters.
(def dim-text (text size wrap)
  (label text :font-size size :color :dim :wrap wrap :bg :transparent))

(def tool-view (entry key)
  (let ((status (get entry :status)))
    (v-stack :key key :width :fill :gap 0.15
      (h-stack :width :fill :gap 0.5
        (dim-text (str (get entry :name) (cond ((= status "failed") " failed") ((= status "running") " …") (else "")))
          11 false)
        (dim-text (get entry :input) 10 false))
      (if (= (get entry :output) "")
        (nothing)
        (dim-text (get entry :output) 10 true))
      (v-stack :width :fill :gap 0.2
        (each (get entry :images) |path j|
          (image :key (str key "-image-" j) :src path :width :fill :height 14))))))

(def open-link (target)
  (if (markdown-view/external-link? target)
    (markdown-view/open-external target)
    (status (str "Not a web link: " target))))

;; The agent's replies are markdown: bold, code, lists, headings.
(def reply-view (entry key)
  (box :key key :width :fill
    (markdown-view/render-markdown (get entry :text) (dict :size 12 :on-link open-link))))

;; An attached image, square and rounded.
(def thumbnail (key path width)
  (image :key key :src path :width width :aspect 1 :radius 10))

;; What you asked, in a bubble, under the images you sent with it.
(def user-view (entry key)
  (let ((images (get entry :images)))
    (v-stack :key key :width :fill :gap 0.4
      (if (empty? images)
        (nothing)
        (wrap :width :fill :gap 0.5 :row-gap 0.5
          (each images |path j|
            (thumbnail (str key "-image-" j) path 9))))
      (if (= (get entry :text) "")
        (nothing)
        (box :width :fill :padding 1.2 :corner-radius 20
          :background-color :blue :border-width 1 :border-color :transparent
          (label (get entry :text) :font-size 12 :color :white :wrap true :bg :transparent))))))

;; ── Collapsed rows ──
;;
;; Each thought, and each run of tool calls, is one grey row that opens to
;; show what it holds. Every row has its own `agent-row` instance, keyed by
;; its first entry's index, so opening one redraws only that row. New bumps
;; the epoch, so the next conversation's rows start closed.

(defstate row-epoch 0)

(def-kind agent-row
  :key (epoch start)
  :state ((open false)))

(def row-state (start) (agent-row row-epoch start))

;; Consecutive tool calls make one group; every other entry is its own.
(def group-entries (entries)
  (reverse
    (reduce |groups i|
      (let ((entry (nth entries i)))
        (if (and (= (get entry :kind) "tool")
                 (not (empty? groups))
                 (= (get (first groups) :kind) "tool"))
          (cons (merge (first groups) :entries (append (get (first groups) :entries) (list entry)))
            (rest groups))
          (cons (dict :kind (get entry :kind) :start i :entries (list entry)) groups)))
      (list) (range (len entries)))))

(def tool-summary (tools)
  (let ((n (len tools))
        (last-tool (nth tools (- n 1)))
        (failed (len (filter |t| (= (get t :status) "failed") tools)))
        (running (= (get last-tool :status) "running")))
    (str
      (if (= n 1)
        (str (get last-tool :name) "  " (get last-tool :input))
        (str "Used " n " tools" (if running (str " · " (get last-tool :name)) "")))
      (if running " …" "")
      (if (> failed 0) (str " · " failed " failed") ""))))

;; A grey row with a chevron; open, `body` shows below it.
(def collapsible (key start title body)
  (let ((row (row-state start))
        (open row.open))
    (v-stack :key key :width :fill :gap 0.25
      (label (str title (if open "  ▾" "  ▸"))
        :key (str key "-title") :font-size 11 :color :dim :hover-color :fg :wrap false :bg :transparent
        :on-click (lambda (event) (set! row.open (not row.open))))
      (if open body (nothing)))))

(def group-view (group is-last busy)
  (let ((kind (get group :kind))
        (start (get group :start))
        (key (str "agent-group-" start))
        (entry (first (get group :entries))))
    (cond
      ((= kind "thinking")
        (collapsible key start (if (and is-last busy) "Thinking" "Thought")
          (dim-text (get entry :text) 11 true)))
      ((= kind "tool")
        (collapsible key start (tool-summary (get group :entries))
          (v-stack :width :fill :gap 0.4 :padding 0.3
            (each (get group :entries) |tool j| (tool-view tool (str key "-tool-" j))))))
      ((= kind "user") (user-view entry key))
      ((= kind "notice") (label (str "! " (get entry :text)) :key key :font-size 11 :color :fg :wrap true :bg :transparent))
      (else (reply-view entry key)))))

;; A 3×3 grid of dots pulsing in a diagonal wave, beside "Working".
(defwidget working-dots
  :width 3.0 :height 1.4
  :animates true
  :shader
  (let ((phase (* itime 4.0))
        (a (+ 0.7 (* 0.3 (sin phase))))
        (b (+ 0.7 (* 0.3 (sin (- phase 0.9)))))
        (c (+ 0.7 (* 0.3 (sin (- phase 1.8)))))
        (d (+ 0.7 (* 0.3 (sin (- phase 2.7)))))
        (e (+ 0.7 (* 0.3 (sin (- phase 3.6)))))
        (blue (vec3 0.4 0.58 1.0)))
    (sdf/layer
      (sdf/fill (sdf/translate -0.4 -0.4 (sdf/circle 0.15)) (material :color (rgba blue a)))
      (sdf/fill (sdf/translate 0 -0.4 (sdf/circle 0.15)) (material :color (rgba blue b)))
      (sdf/fill (sdf/translate 0.4 -0.4 (sdf/circle 0.15)) (material :color (rgba blue c)))
      (sdf/fill (sdf/translate -0.4 0 (sdf/circle 0.15)) (material :color (rgba blue b)))
      (sdf/fill (sdf/translate 0 0 (sdf/circle 0.15)) (material :color (rgba blue c)))
      (sdf/fill (sdf/translate 0.4 0 (sdf/circle 0.15)) (material :color (rgba blue d)))
      (sdf/fill (sdf/translate -0.4 0.4 (sdf/circle 0.15)) (material :color (rgba blue c)))
      (sdf/fill (sdf/translate 0 0.4 (sdf/circle 0.15)) (material :color (rgba blue d)))
      (sdf/fill (sdf/translate 0.4 0.4 (sdf/circle 0.15)) (material :color (rgba blue e))))))

;; Under the last entry while a turn runs.
(def working-row ()
  (h-stack :key "agent-pane-working" :width :fill :gap 0.4 :align :center
    (working-dots :width 3.0 :height 1.4)
    (dim-text "Working" 11 false)))

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

;; Add <kind>: checked once the project has an instance of it (each click
;; still adds another).
(def kind-added? (id)
  (find |instance| (= (get instance :kind) id) project.instances))

(def add-kind-button (decl key)
  (let ((added (kind-added? (get decl :id))))
    (button (str "Add " (get decl :kind)) :key key
      :variant (if added :secondary :primary) :icon (if added :check nil)
      :height 1.0 :font-size 10
      :on-click |x y r| (host-command "packages-new-instance"
                          (dict :module (get decl :module) :kind (get decl :id))))))

(def artifact-card (artifact i)
  (let ((key (str "agent-artifact-" i)) (package (get artifact :package)))
    (box :key key :width :fill :padding 1.0 :corner-radius 12
      :background-color :button-ghost-bg :border-width 1 :border-color :mixer-strip-border
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
            (add-kind-button decl (str key "-add-" j)))
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

;; One tried form: its label, which opens its code (read-only, highlighted)
;; in a tile beside the pane, and closes it again.
(def tried-form (form key)
  (label (str "▸ " (get form :label))
    :key key :font-size 10 :color (if (get form :open) :accent :fg) :hover-color :accent :mono true :bg :transparent
    :on-click (lambda (event)
                (host-command "agent-pane-view-tried" (dict :label (get form :label))))))

;; What the agent is showing in the app with `try`, not yet saved: Keep
;; adds it to init.lisp, Revert takes it off.
(def tried-card (tried)
  (box :key "agent-pane-tried" :width :fill :padding 1.0 :corner-radius 12
    :background-color :button-ghost-bg :border-width 1 :border-color :dropdown-menu-border
    (v-stack :width :fill :gap 0.3
      (h-stack :width :fill :gap 0.5 :align :center
        (label "Trying in the app" :font-size 12 :color :fg :bold true :bg :transparent)
        (box :flex 1)
        (dim-text "not saved" 10 false))
      (v-stack :width :fill :gap 0.2
        (each tried |form j| (tried-form form (str "agent-pane-tried-" j))))
      (wrap :width :fill :gap 0.3 :row-gap 0.3
        (small-button "Keep in init.lisp" "agent-pane-keep" :primary
          (lambda () (host-command "agent-pane-keep-tried" (dict))))
        (small-button "Revert" "agent-pane-revert" :ghost
          (lambda () (host-command "agent-pane-revert-tried" (dict))))))))

(def artifacts-view (artifacts tried)
  (if (and (= (len artifacts) 0) (empty? tried))
    (nothing)
    (v-stack :key "agent-pane-artifacts" :width :fill :gap 0.4
      (if (empty? tried) (nothing) (tried-card tried))
      (each artifacts |artifact i| (artifact-card artifact i)))))

;; ── Knobs to tweak ──
;;
;; Each `defcustom` the agent wrote, in what it tries or in a package of
;; this conversation, as the control the Customize dialog shows: the user
;; tweaks it live. Keep keeps a tried knob at its value; a package's knobs
;; save, as the dialog's do, into init.lisp.

;; "user.try/grain" in a tried `defcustom grain`, "pond.skin/speed" in a
;; package `pond` of this conversation.
(def knob-of-conversation? (row packages tried)
  (let ((module (get row :module))
        (name (short-name (get row :name) module)))
    (if (= module "user.try")
      (find |form| (= (get form :label) (str "defcustom " name)) tried)
      (find |package| (string-starts-with? module (str package ".")) packages))))

;; The knob whose name the pointer is over, its doc shown under the knobs
;; ("" for none).
(defstate knob-doc "")

;; One cell: the name (hover for its doc), ↺ once changed, the control.
(def knob-view (row i)
  (let ((module (get row :module))
        (name (short-name (get row :name) module))
        (key (str "agent-pane-knob-" i))
        (changed (not (= (get row :value) (get row :default)))))
    (box :key key :width 19
     (h-stack :width :fill :gap 0.3 :align :center
      (label name :key (str key "-name") :font-size 10 :color (if changed :accent :fg) :wrap false :bg :transparent
        :on-hover (lambda (over)
                    (cond (over (set! knob-doc (get row :name)))
                          ((= knob-doc (get row :name)) (set! knob-doc ""))
                          (else nil))))
      (if changed
        (label "↺" :key (str key "-reset") :font-size 10 :color :dim :hover-color :fg :bg :transparent
          :on-click (lambda (event) (set-knob (get row :name) (get row :default))))
        (nothing))
      (box :flex 1)
      (knob-editor row (str key "-editor") true)))))

(def knobs-card (knobs saves)
  (let ((open (find |row| (= (get row :name) knob-doc) knobs)))
    (box :key "agent-pane-knobs" :width :fill :padding 0.8 :corner-radius 12
      :background-color :button-ghost-bg :border-width 1 :border-color :dropdown-menu-border
      (v-stack :width :fill :gap 0.4
        (h-stack :width :fill :gap 0.5 :align :center
          (label "Tweak" :font-size 12 :color :fg :bold true :bg :transparent)
          (box :flex 1)
          (if (and saves customize-dirty)
            (small-button "Save tweaks" "agent-pane-knobs-save" :primary customize-save)
            (nothing)))
        (wrap :width :fill :gap 1.0 :row-gap 0.25
          (each knobs |row i| (knob-view row i)))
        ;; Two lines held open, so a doc showing moves nothing.
        (box :width :fill :height 1.0
          (if open (dim-text (get open :doc) 9 true) (nothing)))))))

;; Redrawn when a knob moves (custom-declarations reads each value), apart
;; from the cards above; `packages` and `tried` come from the conversation.
(def knobs-view (packages tried)
  (let ((knobs (filter |row| (knob-of-conversation? row packages tried) (custom-declarations))))
    (if (empty? knobs)
      (nothing)
      (knobs-card knobs (find |row| (not (= (get row :module) "user.try")) knobs)))))

(def empty-state ()
  (v-stack :key "agent-pane-empty" :width :fill :flex 1 :gap 0.6 :padding 1.0
    (dim-text "Ask for a sequencer, a panel or an extension, or for a change to how eseq looks. The agent writes new work as a Local package and shows tweaks live in the app, for you to keep or revert. Drop images on the window to show it what you mean."
      11 true)))

;; The last row: the only one that changes as text streams, so the only
;; one that reads agent.pane-generation. It fetches just its own entries.
(def tail-view (start busy)
  (let ((generation agent.pane-generation)
        (group (first (group-entries (agent-pane/entries-from start)))))
    (if group
      (group-view (merge group :start start) true busy)
      (nothing))))

;; The conversation, redrawn when its rows change (agent.pane-shape), not as
;; text streams into the last one and not as the composer is typed into.
(def conversation ()
  (let ((shape agent.pane-shape)
        (state (agent-pane/state))
        (busy (get state :busy))
        (entries (get state :entries))
        (groups (group-entries entries))
        (last-start (if (empty? groups) -1 (get (nth groups (- (len groups) 1)) :start)))
        (settled (filter |group| (not (= (get group :start) last-start)) groups)))
    (v-stack :width :fill :flex 1 :gap 0.5
      (if (= (len entries) 0)
        (empty-state)
        (scroll :key "agent-pane-scroll" :width :fill :flex 1 :stick-to-bottom true
          ;; Not virtual-v-stack: under :stick-to-bottom it stops short of
          ;; the last reply (eseq-o16s).
          (v-stack :key "agent-pane-entries" :width :fill :gap 0.5 :padding 0.3
            ;; One subtree per settled row, reused until its inputs change.
            (each settled |group i|
              (subtree :key (str "agent-group-tree-" (get group :start))
                (group-view group false false)))
            (subtree :key (str "agent-pane-tail-" last-start) (tail-view last-start busy))
            (if busy (working-row) (nothing)))))
      ;; Its own subtree: the Add buttons read project.instances, whose
      ;; changes need not redraw the conversation. It reads the shape so a
      ;; touch (a tried form's code opened or closed) redraws it.
      (subtree :key "agent-pane-artifacts-tree"
        (let ((shape agent.pane-shape))
          (artifacts-view (get state :artifacts) (agent-pane/tried))))
      (subtree :key "agent-pane-knobs-tree"
        (knobs-view (map |artifact| (get artifact :package) (get state :artifacts)) (agent-pane/tried))))))

;; ── The composer ──
;;
;; Images dropped on the window wait above the text box (× takes one off);
;; the text box with, below it, the model and effort the next turn uses
;; (a menu) and one round button: send (shown once there is text) or, while
;; a turn runs, stop.

(def-kind agent-model-menu
  :key ()
  :state ((open false)
          (at :point :default nil)))

(def effort-label (effort)
  (cond
    ((= effort "low") "Low")
    ((= effort "medium") "Medium")
    ((= effort "high") "High")
    ((= effort "xhigh") "Extra high")
    ((= effort "max") "Max")
    (else effort)))

(def pick (action)
  (set! agent-model-menu.open false)
  (action))

(def model-menu-items (choice)
  (append
    (map (lambda (model)
           (menu-item (get model :label) :key (str "agent-model-" (get model :id))
             :checked (= (get model :id) (get choice :model))
             :on-select (lambda (e) (pick (lambda () (agent-pane/set-model (get model :id)))))))
      (agent-pane/models))
    (append
      (list (menu-separator :key "agent-model-sep"))
      (map (lambda (effort)
             (menu-item (effort-label effort) :key (str "agent-effort-" effort)
               :checked (= effort (get choice :effort))
               :on-select (lambda (e) (pick (lambda () (agent-pane/set-effort effort))))))
        (agent-pane/efforts)))))

(def model-chip (choice)
  (box :key "agent-pane-model" :padding 0.2 :corner-radius 6
    :on-click (lambda (event) (open-menu! agent-model-menu event))
    (label (str (get choice :label) " · " (effort-label (get choice :effort)) " ▾")
      :font-size 11 :color :fg :hover-color :fg :bg :transparent)))

;; The round button: stop while a turn runs, send once there is text.
(def round-button (text key action)
  (button text :key key :width 3.2 :height 1.4 :corner-radius 32 :font-size 15
    :background-color :blue :color :fg :on-click |x y r| (action)))

(def send-or-stop (busy attached)
  (cond
    (busy (round-button "■" "agent-pane-stop" agent-pane/cancel))
    ((and (= draft "") (empty? attached)) (box :width 3.2 :height 1.4))
    (else (round-button "↑" "agent-pane-send" agent-pane-send))))

;; Tokens so far and, between turns, New.
(def usage-row (choice)
  (let ((usage (get choice :usage)))
    (if (not (get choice :started))
      (nothing)
      (h-stack :key "agent-pane-usage" :gap 0.5 :align :center
        (if (get choice :busy)
          (nothing)
          (button "New" :key "agent-pane-new" :variant :ghost :height 1.2 :font-size 10
            :on-click |x y r| (do (set! row-epoch (+ row-epoch 1)) (agent-pane/reset))))
        (dim-text (str (get usage :input) " in · " (get usage :output) " out") 9 false)))))

;; One attached image: a thumbnail, × beside it to take it off.
(def attachment-view (path i)
  (let ((key (str "agent-pane-attachment-" i)))
    (h-stack :key key :gap 0.2 :align :start
      (thumbnail (str key "-image") path 7)
      (label "×" :key (str key "-remove") :font-size 12 :color :dim :hover-color :fg :bg :transparent
        :on-click (lambda (event) (agent-pane/detach i))))))

(def attachments-row (attached)
  (if (empty? attached)
    (nothing)
    (wrap :key "agent-pane-attachments" :width :fill :gap 0.6 :row-gap 0.4 :padding 0.3
      (each attached |path i| (attachment-view path i)))))

(def composer ()
  ;; The shape ties the button, the placeholder, the counts and the
  ;; attached images to the turn.
  (let ((shape agent.pane-shape)
        (choice (agent-pane/choice))
        (attached (agent-pane/attachments)))
    (box :key "agent-pane-composer" :width :fill :padding 0.5 :corner-radius 24
      :background-color :mixer-strip-selected-bg :border-width 1 :border-color :gray
      (v-stack :width :fill :gap 0.3
        (attachments-row attached)
        (textbox :key "agent-pane-input" :value draft
          :placeholder (if (get choice :started) "Refine the request" "Create something")
          :width :fill :min-lines 2 :max-lines 8 :font-size 12 :bg :transparent
          :on-change (lambda (v) (set! draft v)))
        (h-stack :width :fill :gap 0.6 :align :center
          (usage-row choice)
          (box :flex 1)
          (model-chip choice)
          (send-or-stop (get choice :busy) attached))))))

(def model-menu ()
  (let ((shape agent.pane-shape))
    (apply menu-of agent-model-menu (model-menu-items (agent-pane/choice)))))

(effect-buffer "*agent-pane*"
  (box :width :fill :height :fill :background-color :buffer-bg
    (v-stack :width :fill :height :fill :gap 0.5 :padding 1.2
      (subtree :key "agent-pane-conversation" (conversation))
      (subtree :key "agent-pane-composer-tree" (composer))
      (subtree :key "agent-pane-model-menu" (model-menu)))))

(define-mode "eseq.agent-pane/agent-pane-mode" :read-only true)
(mode-bind-key "eseq.agent-pane/agent-pane-mode" "C-c C-c" "eseq.agent-pane/agent-pane-send")
(mode-bind-key "eseq.agent-pane/agent-pane-mode" "q" "eseq.agent-pane/agent-pane-close")
(set-buffer-mode-for "*agent-pane*" "eseq.agent-pane/agent-pane-mode")
(bind-key "C-x C-a" "agent-pane-open")
