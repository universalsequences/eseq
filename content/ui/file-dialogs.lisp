;; File-menu dialogs: the project Save / Save As name modal and the About
;; modal. Both are opened by Rust host commands ("project-save-open",
;; "about-open") that first activate the tile mounting `panel`, because a
;; modal only receives pointer input through the active tile. Mounted by both
;; step-panel buffers (`*sequencer*` and `*arrangement*`), like export-song.
(module eseq.file-dialogs)
(export panel open-save close-save save-open? save-draft commit-save
        open-about close-about about-open?)

(defstate save-open? false)
(defstate save-draft "")
(defstate save-title "Save project")
(defstate about-open? false)
(defstate about-version "")

;; `initial` seeds the name field: "" for a never-saved project, the current
;; name for Save As. Cancelling leaves the project untouched: no host command
;; runs until `commit-save`.
(def open-save (title initial)
  (set! save-title title)
  (set! save-draft initial)
  (set! save-open? true))

(def close-save () (set! save-open? false))

(def commit-save ()
  (if (= (len (string-trim save-draft)) 0)
    (status "Enter a project name")
    (do
      (host-command "save-project" (dict :name save-draft))
      (close-save))))

(def open-about (version)
  (set! about-version version)
  (set! about-open? true))

(def close-about () (set! about-open? false))

(def save-body ()
  (v-stack :width :fill :height :fill :gap 0.5
    (label save-title :key "project-save-title" :font-size 16 :color :white :bg :transparent)
    (label "Project name" :font-size 11 :color :dim :bg :transparent)
    (text-input :key "project-save-name" :width :fill :height 1.3 :font-size 13
      :value save-draft
      :placeholder "Untitled"
      :auto-focus true
      :select-all-on-focus true
      :on-change (lambda (v) (set! save-draft v))
      :on-submit (lambda () (commit-save))
      :on-cancel (lambda () (close-save)))
    (label "Saved into the projects folder as <name>.json" :font-size 10 :color :dim :bg :transparent)
    (box :flex 1 :bg :transparent)
    (h-stack :width :fill :gap 0.5
      (box :flex 1 :bg :transparent)
      (button "Cancel" :key "project-save-cancel" :on-click |x y r| (close-save))
      (button "Save" :key "project-save-submit" :variant :primary
        :disabled (= (len (string-trim save-draft)) 0)
        :on-click |x y r| (commit-save)))))

(def about-body ()
  (v-stack :width :fill :height :fill :gap 0.4
    (label "eseq" :key "about-title" :font-size 20 :color :white :bg :transparent)
    (label (str "Version " about-version) :key "about-version" :font-size 12 :color :white :bg :transparent)
    (label "Live sequencer and sound design." :font-size 11 :color :dim :bg :transparent)
    (label "https://github.com/universalsequences/eseq" :font-size 10 :color :dim :bg :transparent)
    (box :flex 1 :bg :transparent)
    (h-stack :width :fill :gap 0.5
      (box :flex 1 :bg :transparent)
      (button "OK" :key "about-ok" :variant :primary :on-click |x y r| (close-about)))))

(def panel ()
  (v-stack :width 0 :height 0 :bg :transparent
    (modal :is-open save-open? :on-close (lambda () (close-save)) :width-px 520 :height-px 380
      (box :debug-name "project-save-panel" :width :fill :height :fill :padding 0.6 :bg :transparent
        (if save-open? (save-body) (box :width 0 :height 0 :bg :transparent))))
    (modal :is-open about-open? :on-close (lambda () (close-about)) :width-px 480 :height-px 340
      (box :debug-name "about-panel" :width :fill :height :fill :padding 0.6 :bg :transparent
        (if about-open? (about-body) (box :width 0 :height 0 :bg :transparent))))))
