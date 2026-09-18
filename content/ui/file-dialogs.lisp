;; File-menu dialogs: the project Save / Save As name modal, the About
;; modal and the Import Package confirmation. All are opened by Rust host
;; commands ("project-save-open", "about-open", "menu-import-package") that
;; first activate the tile mounting `panel`, because a modal only receives
;; pointer input through the active tile. Mounted by both step-panel buffers
;; (`*sequencer*` and `*arrangement*`), like export-song.
(module eseq.file-dialogs)
(import eseq.settings)
(import eseq.customize)
(export open-confirm panel open-save close-save save-open? save-draft commit-save
        open-unsaved-prompt close-unsaved-prompt unsaved-prompt-open?
        unsaved-prompt-save unsaved-prompt-discard
        open-about close-about about-open?
        open-package-import close-package-import package-import-open?
        package-import-install package-import-cancel
        open-package-export close-package-export package-export-open?
        package-export-identity package-export-version package-export-toggle
        package-export-commit)

(defstate save-open? false)
(defstate save-draft "")
(defstate save-title "Save project")
;; "" or a host command to run after a successful save ("new-project").
(defstate save-then "")
(defstate unsaved-prompt-open? false)
(defstate about-open? false)
(defstate about-version "")

;; `initial` seeds the name field: "" for a never-saved project, the current
;; name for Save As. Cancelling leaves the project untouched: no host command
;; runs until `commit-save`.
(def open-save (title initial then)
  (set! save-title title)
  (set! save-draft initial)
  (set! save-then then)
  (set! save-open? true))

(def close-save () (set! save-open? false))

(def commit-save ()
  (if (= (len (string-trim save-draft)) 0)
    (status "Enter a project name")
    (do
      (host-command "save-project" (dict :name save-draft :then save-then))
      (close-save))))

;; File > New Project on a project with unsaved changes. Save routes through
;; the normal save path (naming first when needed) and only then starts the
;; new project; Don't Save starts it immediately; Cancel does nothing.
(def open-unsaved-prompt () (set! unsaved-prompt-open? true))
(def close-unsaved-prompt () (set! unsaved-prompt-open? false))

(def unsaved-prompt-save ()
  (close-unsaved-prompt)
  (host-command "project-save-open" (dict :mode "save" :then "new-project")))

(def unsaved-prompt-discard ()
  (close-unsaved-prompt)
  (host-command "new-project" (dict)))

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

(def unsaved-prompt-body ()
  (v-stack :width :fill :height :fill :gap 0.5
    (label "Save changes first?" :key "unsaved-prompt-title"
      :font-size 16 :color :white :bg :transparent)
    (label "Starting a new project discards unsaved changes." :width :fill :wrap true
      :font-size 11 :color :dim :bg :transparent)
    (box :flex 1 :bg :transparent)
    (h-stack :width :fill :gap 0.5
      (button "Don't Save" :key "unsaved-prompt-discard"
        :on-click |x y r| (unsaved-prompt-discard))
      (box :flex 1 :bg :transparent)
      (button "Cancel" :key "unsaved-prompt-cancel"
        :on-click |x y r| (close-unsaved-prompt))
      (button "Save" :key "unsaved-prompt-save" :variant :primary
        :on-click |x y r| (unsaved-prompt-save)))))

(def about-body ()
  (v-stack :width :fill :height :fill :gap 0.4
    (label "eseq" :key "about-title" :font-size 20 :color :white :bg :transparent)
    (label (str "Version " about-version) :key "about-version" :font-size 12 :color :white :bg :transparent)
    (label "Live sequencer and sound design." :font-size 11 :color :dim :bg :transparent)
    (label "https://github.com/universalsequences/eseq" :font-size 10 :color :dim :bg :transparent)
    (scroll :key "about-credit-scroll" :width :fill :flex 1
      (v-stack :key "about-sample-credits" :width :fill :gap 0.3
        (label "Salamander Grand Piano V3 — Alexander Holm" :font-size 11 :color :white :bg :transparent)
        (label "CC BY 3.0 · Velocity-8 MP3 selection" :font-size 10 :color :dim :bg :transparent)
        (label "MP3 conversion via Strudel / dough-samples" :font-size 10 :color :dim :bg :transparent)
        (label "https://creativecommons.org/licenses/by/3.0/" :font-size 10 :color :dim :bg :transparent)
        (label "https://archive.org/details/SalamanderGrandPianoV3" :font-size 10 :color :dim :bg :transparent)
        (label "Latent Sonorities Sample Pack — memeshift" :font-size 11 :color :white :bg :transparent)
        (label "PM Saron · Slenthem · Bonang · Slenthem Slendro · Kempyang · Kethuk" :font-size 10 :color :dim :bg :transparent)
        (label "Performed by Bilawa Ade Respati · Recorded by Rabih Beaini" :font-size 10 :color :dim :bg :transparent)
        (label "Source recordings: CC BY-NC 4.0 · Attribution–NonCommercial" :font-size 10 :color :dim :bg :transparent)
        (label "Physical models calibrated from measurements; no recordings embedded." :font-size 10 :color :dim :bg :transparent)
        (label "https://freesound.org/people/memeshift/packs/40343/" :font-size 10 :color :dim :bg :transparent)
        (label "https://www.latentsonorities.org/" :font-size 10 :color :dim :bg :transparent)
        (label "https://creativecommons.org/licenses/by-nc/4.0/" :font-size 10 :color :dim :bg :transparent)
        (label "Acoustic Cymbals Vol.1 — Donit" :font-size 11 :color :white :bg :transparent)
        (label "PM Crash · PM Ride · PM Hi-Hat" :font-size 10 :color :dim :bg :transparent)
        (label "Physical models calibrated from measurements; no recordings embedded." :font-size 10 :color :dim :bg :transparent)
        (label "https://youtu.be/UaVYYyqF4IY" :font-size 10 :color :dim :bg :transparent)
        (label "TR-808 — Michael Fischer · TidalCycles · CC0" :font-size 11 :color :white :bg :transparent)
        (label "VCSL — Versilian Studios LLC · CC0" :font-size 11 :color :white :bg :transparent)))
    (h-stack :width :fill :gap 0.5
      (box :flex 1 :bg :transparent)
      (button "OK" :key "about-ok" :variant :primary :on-click |x y r| (close-about)))))

(defstate confirm-open? false)
(defstate confirm-message "")
(defstate confirm-action nil)
(def open-confirm (message action)
  (set! confirm-message message)
  (set! confirm-action action)
  (set! confirm-open? true))
(def close-confirm () (set! confirm-open? false) (set! confirm-action nil))
(def accept-confirm ()
  (let ((action confirm-action))
    (close-confirm)
    (if action (action) nil)))

;; ── Import Package… ──
;;
;; Rust stages the picked folder / archive, validates it, and opens this
;; modal; the body is a view over `seq-package-import-summary` (a dict of
;; counts, or nil once the staging is gone). Install / Replace publish it,
;; Cancel discards the staging. Packages are trusted code: the modal says
;; so instead of pretending a sandbox exists.
(defstate package-import-open? false)

(def open-package-import () (set! package-import-open? true))
(def close-package-import () (set! package-import-open? false))
(def package-import-install ()
  (host-command "package-import-commit" (dict)))
(def package-import-cancel ()
  (host-command "package-import-cancel" (dict)))

(def package-import-count-row (caption count)
  (if (> count 0)
    (h-stack :width :fill :gap 0.5
      (label (str count) :font-size 12 :color :white :bg :transparent :width 2)
      (label caption :font-size 12 :color :dim :bg :transparent))
    (box :width 0 :height 0 :bg :transparent)))

(def package-import-body ()
  (let ((summary (seq-package-import-summary)))
    (if (= summary nil)
      (v-stack :width :fill :height :fill :padding 1 :gap 1
        (label "Nothing staged for import." :font-size 12 :color :dim :bg :transparent)
        (h-stack :width :fill :gap 0.5
          (box :flex 1 :bg :transparent)
          (button "Close" :key "package-import-close" :on-click |x y r| (close-package-import))))
      (let ((installed? (get summary :installed?)))
        (v-stack :width :fill :height :fill :padding 1 :gap 0.6
          (label (str "Import " (get summary :identity) " " (get summary :version))
            :key "package-import-title" :font-size 15 :color :white :bg :transparent)
          (label (get summary :path) :font-size 10 :color :dim :bg :transparent)
          (v-stack :width :fill :gap 0.2 :padding 0.4
            (package-import-count-row "Lisp modules" (get summary :modules))
            (package-import-count-row "instruments" (get summary :instruments))
            (package-import-count-row "effects" (get summary :effects))
            (package-import-count-row "MIDI effects" (get summary :midi-fx))
            (package-import-count-row "preset banks" (get summary :presets))
            (package-import-count-row "samples" (get summary :samples))
            (package-import-count-row "themes" (get summary :themes)))
          (if installed?
            (label (str "A package named " (get summary :identity) " is already installed; importing replaces it.")
              :key "package-import-replace-note" :font-size 11 :color :accent :bg :transparent)
            (box :width 0 :height 0 :bg :transparent))
          (label "Packages are trusted code: installing runs its Lisp."
            :font-size 11 :color :dim :bg :transparent)
          (h-stack :width :fill :gap 0.5 :padding-top 0.6
            (box :flex 1 :bg :transparent)
            (button "Cancel" :key "package-import-cancel" :on-click |x y r| (package-import-cancel))
            (button (if installed? "Replace" "Install")
              :key "package-import-install" :variant :primary
              :on-click |x y r| (package-import-install))))))))

;; ── Export Package… ──
;;
;; Picks user-tier instruments and effects (factory content must be forked
;; first: only the user's library exports) into an `author/name` pack. The
;; picked set lives in Rust (`seq-package-export-toggle`); `generation`
;; re-renders the list after every toggle. Commit opens the native save
;; panel and writes `<author.name>-<version>.eseqpack`.
(defstate package-export-open? false)
(defstate package-export-identity "")
(defstate package-export-version "1.0")
(defstate package-export-generation 0)

(def open-package-export ()
  (seq-package-export-clear)
  (set! package-export-generation (+ package-export-generation 1))
  (set! package-export-open? true))
(def close-package-export () (set! package-export-open? false))
(def package-export-toggle (kind name)
  (seq-package-export-toggle kind name)
  (set! package-export-generation (+ package-export-generation 1)))
(def package-export-commit ()
  (host-command "package-export-commit"
    (dict :identity package-export-identity :version package-export-version)))

(def package-export-row (item)
  (let ((kind (get item :kind)) (name (get item :name)))
    (h-stack :key (str "package-export-row-" kind "-" name) :width :fill :gap 0.5 :align :center
      (toggle :value (get item :selected?)
        :on-change (lambda (value) (package-export-toggle kind name)))
      (label name :font-size 12 :color :white :bg :transparent :flex 1))))

(def package-export-column (title kind hint)
  (let ((items (seq-package-export-candidates kind)))
    (v-stack :flex 1 :gap 0.3
      (label title :key (str "package-export-column-" kind) :font-size 12 :color :white :bg :transparent)
      (label hint :width :fill :wrap true :font-size 10 :color :dim :bg :transparent)
      (scroll :key (str "package-export-list-" kind) :width :fill :height 10
        (v-stack :width :fill :gap 0.15
          (if (= (len items) 0)
            (label "Nothing here yet." :font-size 11 :color :dim :bg :transparent)
            (each items |item| (package-export-row item))))))))

(def package-export-body ()
  (let ((epoch package-export-generation)
        (selected (seq-package-export-selected-count)))
    (v-stack :width :fill :height :fill :padding 1 :gap 0.5
      (label "Export Package" :key "package-export-title" :font-size 16 :color :white :bg :transparent)
      (h-stack :width :fill :gap 0.6
        (v-stack :flex 2 :gap 0.2
          (label "Package name (author/name)" :font-size 10 :color :dim :bg :transparent)
          (text-input :key "package-export-identity" :width :fill :height 1.3 :font-size 13
            :value package-export-identity
            :placeholder "alec/acid-tools"
            :auto-focus true
            :on-change (lambda (v) (set! package-export-identity v))
            :on-cancel (lambda () (close-package-export))))
        (v-stack :flex 1 :gap 0.2
          (label "Version" :font-size 10 :color :dim :bg :transparent)
          (text-input :key "package-export-version" :width :fill :height 1.3 :font-size 13
            :value package-export-version
            :placeholder "1.0"
            :on-change (lambda (v) (set! package-export-version v))
            :on-cancel (lambda () (close-package-export)))))
      (h-stack :width :fill :gap 0.8 :padding-top 0.4
        (package-export-column "Instruments" "instrument"
          "From your library. Fork a factory synth to export it.")
        (package-export-column "Effects" "effect"
          "Custom effects from your library.")
        (package-export-column "Presets" "presets"
          "Your saved presets for factory instruments."))
      (label "Library macros are inlined; absolute paths are reported after export."
        :font-size 10 :color :dim :bg :transparent)
      (h-stack :width :fill :gap 0.5 :padding-top 0.4
        (label (str selected " selected") :font-size 11 :color :dim :bg :transparent)
        (box :flex 1 :bg :transparent)
        (button "Cancel" :key "package-export-cancel" :on-click |x y r| (close-package-export))
        (button "Export…" :key "package-export-submit" :variant :primary
          :disabled (or (= selected 0) (= (len (string-trim package-export-identity)) 0))
          :on-click |x y r| (package-export-commit))))))

(def panel ()
  (v-stack :width 0 :height 0 :bg :transparent
    (eseq.settings/panel)
    (eseq.customize/panel)
    (modal :is-open package-export-open? :on-close (lambda () (close-package-export))
        :width-px 1200 :height-px 820
      (box :debug-name "package-export-panel" :width :fill :height :fill :padding 0.6 :bg :transparent
        (if package-export-open? (package-export-body) (box :width 0 :height 0 :bg :transparent))))
    (modal :is-open package-import-open? :on-close (lambda () (package-import-cancel))
        :width-px 600 :height-px 480
      (box :debug-name "package-import-panel" :width :fill :height :fill :padding 0.6 :bg :transparent
        (if package-import-open? (package-import-body) (box :width 0 :height 0 :bg :transparent))))
    (modal :is-open confirm-open? :on-close close-confirm :width-px 520 :height-px 220
      (v-stack :width :fill :height :fill :padding 1 :gap 1
        (label confirm-message :key "menu-confirm-message" :font-size 14 :bg :transparent)
        (h-stack :gap 1
          (button "Cancel" :key "menu-confirm-cancel" :on-click (lambda (event) (close-confirm)))
          (button "Continue" :key "menu-confirm-accept" :on-click (lambda (event) (accept-confirm))))))
    (modal :is-open save-open? :on-close (lambda () (close-save)) :width-px 520 :height-px 380
      (box :debug-name "project-save-panel" :width :fill :height :fill :padding 0.6 :bg :transparent
        (if save-open? (save-body) (box :width 0 :height 0 :bg :transparent))))
    (modal :is-open unsaved-prompt-open? :on-close (lambda () (close-unsaved-prompt))
        :width-px 560 :height-px 300
      (box :debug-name "unsaved-prompt-panel" :width :fill :height :fill :padding 0.6 :bg :transparent
        (if unsaved-prompt-open? (unsaved-prompt-body) (box :width 0 :height 0 :bg :transparent))))
    (modal :is-open about-open? :on-close (lambda () (close-about)) :width-px 720 :height-px 600
      (box :debug-name "about-panel" :width :fill :height :fill :padding 0.6 :bg :transparent
        (if about-open? (about-body) (box :width 0 :height 0 :bg :transparent))))))
