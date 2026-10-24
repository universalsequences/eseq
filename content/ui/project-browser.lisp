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

(import eseq.view-kit :refer (listed? bare-text))
(import eseq.scene-banks :refer (scene-viewed-bank-id scene-viewed-bank-label))
(import eseq.track-collapse :refer (instrument-icon))

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

;; Whether the import makes new banks: a picked bank always comes as a new
;; bank (one scene per source scene), whatever the target.
(def import-makes-bank? (target)
  (or (= target :new-bank) (> (len (picks-of :bank)) 0)))

;; `target`: :bank (the viewed bank) or :new-bank. An import that makes a
;; bank views it (the host, once it succeeded): what it brought is on only
;; there, and reads off (parked) in every existing bank.
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

;; The Projects tab's mode toggle: with it on, a project row opens for
;; importing instead of loading.
(def peek-mode-button ()
  (button (if (peek-mode?) "Importing" "Import")
    :key "project-peek-mode"
    :variant (if (peek-mode?) :primary :secondary)
    :width 6.5
    :height 1.3
    :font-size 10.5
    :on-click |x y r| (toggle-peek-mode!)
    :color :white))

;; Section heading: small label with a little air above it.
(def section-title (title)
  (box :key (str "peek-section-" title) :width :fill :padding-top 0.4 :padding-bottom 0.1
    (label title :font-size 9.5 :color :dimmer :bg :transparent)))

;; The kind as a key fragment: "kit", "group", "track", "bank", "sequencer".
(def kind-tag (kind)
  (if (= kind :kit) "kit"
    (if (= kind :group) "group"
      (if (= kind :track) "track"
        (if (= kind :bank) "bank" "sequencer")))))

;; Whether this session already imported item `id` of `kind` from the
;; peeked project (`imported`: the host's ids by kind, read once).
(def imported-pick? (imported kind id)
  (listed? id (or (get imported (kind-tag kind)) (list))))

;; One pickable row: a toggle, the name over a dim one-line detail, and a
;; checkmark once imported.
(def pick-row (imported kind id pick-name title detail)
  (let ((key (str "peek-" (kind-tag kind) "-" id)))
    (h-stack :key key :width :fill :gap 0.5 :align :center
      (toggle :value (picked? kind id)
        :on-change (lambda (value) (toggle-pick! kind id pick-name)))
      (v-stack :flex 1 :gap 0
        ;; Imported: a green check right after the name (on the name's
        ;; line, where a long detail line cannot push it out of the card).
        (h-stack :gap 0.25 :align :center
          (label title :font-size 11 :color :white :bg :transparent)
          (when (imported-pick? imported kind id)
            (apply badge ""
              (append
                (list :key (str key "-imported")
                      :icon :check
                      :icon-color :green
                      :width 1.2 :height 1 :padding 0)
                bare-text))))
        (label detail :font-size 9 :color :dim :bg :transparent)))))

;; A rounded card around one kit or group.
(def pick-card (key body)
  (box :key key :width :fill :background-color :mixer-strip-bg :corner-radius 6 :padding 0.35
    body))

;; A kit pad: what plays on it as an icon beside the pad's name (one badge,
;; as a sequencer track header draws its device glyph); under it, the
;; device when it says more than the name, and a rack's slot count.
(def pad-detail (text slots)
  (if (> slots 1)
    (if (= text "") (str slots " slots") (str text " · " slots " slots"))
    text))

(def device-row (kit-id i device)
  (let ((detail (pad-detail (get device :label) (get device :slots)))
        (key (str "peek-kit-" kit-id "-pad-" i)))
    (h-stack :key key :width :fill :gap 0 :align :top
      (box :width 2.4 :height 0.1)
      (v-stack :flex 1 :gap 0
        (apply badge (get device :pad)
          (append
            (list :key (str key "-name")
                  :icon (instrument-icon (get device :kind))
                  :icon-color :dimmer
                  :font-size 9.5 :width :fill :height 1 :padding 0
                  :h-align :left
                  :color :dim)
            bare-text))
        (when (not (= detail ""))
          (h-stack :gap 0
            (box :width 2.1 :height 0.1)
            (label detail :font-size 8.5 :color :dimmer :bg :transparent)))))))

(def kit-rows (imported kit)
  (let ((devices (get kit :devices))
        (id (get kit :id)))
    (pick-card (str "peek-kit-block-" id)
      (v-stack :width :fill :gap 0.15
        (pick-row imported :kit id (get kit :name) (get kit :name) (get kit :detail))
        (each (range 0 (len devices)) |i|
          (device-row id i (nth devices i)))))))

;; A section only when the peek has rows of that kind.
(def peek-section (title rows row-fn)
  (when (> (len rows) 0)
    (v-stack :key (str "peek-block-" title) :width :fill :gap 0.3
      (section-title title)
      (each rows |row| (row-fn row)))))

(def peek-sections (peek imported)
  (v-stack :key "project-peek-sections" :width :fill :gap 0.1
    (peek-section "Banks" (get peek :banks)
      |b| (pick-row imported :bank (get b :id) (get b :name) (str "Bank " (get b :name)) (get b :detail)))
    (peek-section "Kits" (get peek :kits) |k| (kit-rows imported k))
    (peek-section "Groups" (get peek :groups)
      |g| (pick-card (str "peek-group-block-" (get g :id))
            (pick-row imported :group (get g :id) (get g :name) (get g :name) (get g :detail))))
    (peek-section "Tracks" (get peek :tracks)
      |t| (pick-row imported :track (get t :id) (get t :name) (get t :name) (get t :detail)))
    (peek-section "Sequencers" (get peek :sequencers)
      |q| (pick-row imported :sequencer (get q :id) (get q :name) (get q :name) (get q :detail)))))

(def import-button (key text variant target)
  (button text
    :key key
    :variant variant
    :width :fill :height 1.4 :font-size 10.5
    :on-click |x y r| (import-picks! target)
    :color :white))

;; Stacked full-width so the labels fit a narrow browser pane. A picked bank
;; comes as a new bank either way: then only that button. The result is a
;; toast, and a checkmark on what came in.
(def import-buttons ()
  (let ((bank (scene-viewed-bank-label))
        (n (pick-count)))
    (v-stack :key "project-peek-actions" :width :fill :gap 0.3
      (when (> n 0)
        (label (str "Import " n (if (= n 1) " pick" " picks"))
          :font-size 9.5 :color :dim :bg :transparent))
      (when (not (import-makes-bank? :bank))
        (import-button "project-peek-import-bank"
          (if (= bank "") "Into this bank" (str "Into bank " bank)) :primary :bank))
      (import-button "project-peek-import-new-bank" "As a new bank" :secondary :new-bank))))

(def peek-panel ()
  (let ((peek (seq-project-peek project-peek.name))
        (imported (seq-project-imported project-peek.name)))
    (v-stack :key "project-peek-panel" :width :fill :gap 0.4 :flex 1
      (h-stack :width :fill :gap 0.5 :align :center
        (button "Back"
          :key "project-peek-back"
          :variant :secondary
          :width 4 :height 1.2 :font-size 10
          :on-click |x y r| (close-peek!)
          :color :white)
        (label (or (get peek :name) project-peek.name)
          :font-size 11.5 :color :white :bg :transparent :flex 1)
        (badge "Import"
          :key "project-peek-mode-badge"
          :font-size 9 :width 4.2 :height 1.1 :padding 0
          :color :accent
          :background-color (mix :accent :buffer-bg 0.8)
          :border-color :transparent
          :highlight-color :transparent
          :shadow-color :transparent
          :corner-radius 5))
      (if (get peek :error)
        (box :width :fill :padding 0.5
          (label (get peek :error) :font-size 10 :color :red :bg :transparent))
        (v-stack :width :fill :gap 0.4 :flex 1
          (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0.3 :flex 1
            (scroll :key "project-peek-scroll" :width :fill :flex 1
              (peek-sections peek imported)))
          (import-buttons))))))
