;; ui/project-browser.lisp — the Projects tab's peek (docs/rig-spec.md §8)
;;
;; With "Peek" on, a project row opens that project READ-ONLY as a tree
;; instead of loading it: its banks, kits (pads and devices), groups, tracks
;; and project sequencers, each with a pick toggle. Nothing in the open
;; project changes while browsing. The two buttons at the foot import the
;; picks into the viewed bank, or as a new bank; the host appends new
;; tracks and kits, leaves them on only in that bank, and reports anything
;; it left unconnected (shown under the buttons).

(module eseq.project-browser)

(import eseq.view-kit :refer (listed?))
(import eseq.scene-banks :refer (scene-viewed-bank-id scene-viewed-bank-label))

(export project-peek
        peek-mode?
        toggle-peek-mode!
        peek-project!
        close-peek!
        peek-open?
        picked?
        toggle-pick!
        pick-count
        import-picks!
        peek-mode-button
        peek-panel)

;; The peek: whether project rows peek (`mode`), the project peeked ("" =
;; none) and the picks: a dict by kind (:kit :group :track :bank
;; :sequencer) of `{:id :name}` rows, the source id (a track's index) and
;; the name the peek showed, which the host checks the file still has.
(def no-picks () (dict :kit (list) :group (list) :track (list) :bank (list) :sequencer (list)))

(def-kind project-peek
  :key ()
  :state ((mode false)
          (name "")
          (picks :any :default false)))

(def peek-mode? () project-peek.mode)

(def toggle-peek-mode! ()
  (set! project-peek.mode (not project-peek.mode)))

(def clear-picks! ()
  (set! project-peek.picks (no-picks)))

(def peek-project! (name)
  (clear-picks!)
  (set! project-peek.name name))

(def close-peek! ()
  (clear-picks!)
  (set! project-peek.name "")
  (seq-project-peek-close))

(def peek-open? ()
  (not (= project-peek.name "")))

(def picks-of (kind)
  (let ((picks project-peek.picks))
    (if picks (get picks kind) (list))))

(def picked? (kind id)
  (reduce |found row| (or found (= (get row :id) id)) false (picks-of kind)))

(def toggle-pick! (kind id name)
  (set! project-peek.picks
    (merge (or project-peek.picks (no-picks)) kind
      (if (picked? kind id)
        (filter (lambda (row) (not (= (get row :id) id))) (picks-of kind))
        (append (picks-of kind) (list (dict :id id :name name)))))))

(def pick-count ()
  (reduce |n kind| (+ n (len (picks-of kind))) 0
    (list :kit :group :track :bank :sequencer)))

;; `target`: :bank (the viewed bank) or :new-bank.
(def import-picks! (target)
  (if (= (pick-count) 0)
    (status "Pick a kit, group, track or bank to import")
    (do
      (host-command "import-from-project"
        (dict :name project-peek.name
              :picks (or project-peek.picks (no-picks))
              :target target
              :bank-id (scene-viewed-bank-id)))
      (clear-picks!))))

;; ── View ──

(def peek-mode-button ()
  (button (if (peek-mode?) "Peek: on" "Peek")
    :key "project-peek-mode"
    :variant (if (peek-mode?) :primary :secondary)
    :width 6.5
    :height 1.3
    :font-size 10.5
    :on-click |x y r| (toggle-peek-mode!)
    :color :white))

(def section-title (title)
  (box :key (str "peek-section-" title) :width :fill :padding 0.2
    (label title :font-size 10 :color :dimmer :bg :transparent)))

;; One pickable row: a toggle, the item's name and a dim detail line.
;; The kind as a key fragment: "kit", "group", "track", "bank", "sequencer".
(def kind-tag (kind)
  (if (= kind :kit) "kit"
    (if (= kind :group) "group"
      (if (= kind :track) "track"
        (if (= kind :bank) "bank" "sequencer")))))

(def pick-row (kind id pick-name title detail)
  (h-stack :key (str "peek-" (kind-tag kind) "-" id) :width :fill :gap 0.5 :align :center
    (toggle :value (picked? kind id)
      :on-change (lambda (value) (toggle-pick! kind id pick-name)))
    (label title :font-size 11 :color :white :bg :transparent)
    (label detail :font-size 9.5 :color :dim :bg :transparent :flex 1)))

(def device-row (kit-id i line)
  (h-stack :key (str "peek-kit-" kit-id "-pad-" i) :width :fill :gap 0 :align :center
    (box :width 2.2 :height 0.1)
    (label line :font-size 9.5 :color :dim :bg :transparent)))

(def kit-rows (kit)
  (let ((devices (get kit :devices)))
    (v-stack :key (str "peek-kit-block-" (get kit :id)) :width :fill :gap 0.05
      (pick-row :kit (get kit :id) (get kit :name) (get kit :name) (get kit :detail))
      (each (range 0 (len devices)) |i|
        (device-row (get kit :id) i (nth devices i))))))

(def peek-sections (peek)
  (v-stack :key "project-peek-sections" :width :fill :gap 0.2
    (section-title "Banks")
    (each (get peek :banks) |b|
      (pick-row :bank (get b :id) (get b :name) (str "Bank " (get b :name)) (get b :detail)))
    (section-title "Kits")
    (each (get peek :kits) |k| (kit-rows k))
    (section-title "Groups")
    (each (get peek :groups) |g|
      (pick-row :group (get g :id) (get g :name) (get g :name) (get g :detail)))
    (section-title "Tracks")
    (each (get peek :tracks) |t|
      (pick-row :track (get t :id) (get t :name) (get t :name) (get t :detail)))
    (section-title "Sequencers")
    (each (get peek :sequencers) |s|
      (pick-row :sequencer (get s :id) (get s :name) (get s :name) (get s :detail)))))

(def summary-lines ()
  (let ((summary (seq-project-import-summary)))
    (if (= (get summary :project) project-peek.name) (get summary :lines) (list))))

(def import-buttons ()
  (let ((bank (scene-viewed-bank-label)))
    (v-stack :key "project-peek-actions" :width :fill :gap 0.3
      (h-stack :width :fill :gap 0.5 :align :center
        (button (if (= bank "") "Import into this bank" (str "Import into bank " bank))
          :key "project-peek-import-bank"
          :variant :primary
          :flex 1 :height 1.3 :font-size 10.5
          :on-click |x y r| (import-picks! :bank)
          :color :white)
        (button "Import as new bank"
          :key "project-peek-import-new-bank"
          :variant :secondary
          :flex 1 :height 1.3 :font-size 10.5
          :on-click |x y r| (import-picks! :new-bank)
          :color :white))
      (each (summary-lines) |line|
        (label line :font-size 9.5 :color :gray :bg :transparent)))))

(def peek-panel ()
  (let ((peek (seq-project-peek project-peek.name)))
    (v-stack :key "project-peek-panel" :width :fill :gap 0.4 :flex 1
      (h-stack :width :fill :gap 0.5 :align :center
        (button "Back"
          :key "project-peek-back"
          :variant :ghost
          :width 5 :height 1.2 :font-size 10
          :on-click |x y r| (close-peek!)
          :color :gray)
        (label (str "Peek: " (or (get peek :name) project-peek.name) " (read-only)")
          :font-size 11 :color :white :bg :transparent :flex 1))
      (if (get peek :error)
        (box :width :fill :padding 0.5
          (label (get peek :error) :font-size 10 :color :red :bg :transparent))
        (v-stack :width :fill :gap 0.4 :flex 1
          (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0.3 :flex 1
            (scroll :key "project-peek-scroll" :width :fill :flex 1
              (peek-sections peek)))
          (import-buttons))))))
