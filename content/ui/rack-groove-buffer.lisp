;; ui/rack-groove-buffer.lisp — the *groove* buffer: the selected drum rack's
;; groove, stacked under the browser (docs/rack-groove-spec.md; bead
;; eseq-yks3).
;;
;; While a drum rack is selected (its bus, or a track inside it) the sidebar
;; splits in two: the browser keeps the top 70% and this buffer takes the
;; bottom 30% (eseq.seq-layout/samples-sidebar-layout-spec). It only ever
;; shows THIS rack's groove:
;;
;;   header   groove picker (filterable; its footer extracts) · on/off
;;   amounts  Timing / Velocity / Random
;;   lanes    All + one row per pad: where each 16th lands (late up, early
;;            down), the pad's share of the groove and its include dot. With
;;            no groove the lanes show the pads' hits sitting on the grid.
;;
;; Data rides on the rack's SEQ.rack-grooves entry (:lanes, :enabled) and on
;; scalar fields for every amount, so dragging a number never rebuilds the
;; lanes. Every edit is a groove host command (src/ui/host_commands/
;; rack_grooves.rs). The ≡ menu beside the picker extracts a new groove and
;; saves, renames, duplicates or deletes the playing one.
(module eseq.rack-groove-buffer)

(import eseq.seq-core-state)
(import eseq.drum-rack-v2 :as rack)

(export selected-rack-id
        showing?
        panel
        extract-modal
        open-extract
        commit-extract
        extract-open?
        menu-actions
        select-menu-action
        action-save
        commit-rename
        cancel-rename
        rename-draft
        renaming)

;; ── Which rack ──────────────────────────────────────────────────────────
;; The rack whose bus is selected (what *fx* shows), else the rack the
;; current track belongs to; -1 when neither is a drum rack.
(def selected-rack-id ()
  (let ((by-bus (if (eseq.seq-core-state/seq-has-selected-bus?)
                  (rack/rack-of-bus eseq.seq-core-state/selected-bus)
                  -1)))
    (if (>= by-bus 0)
      (rack/group-id by-bus)
      (let ((by-track (if (> (or SEQ.num-tracks 0) 0)
                        (rack/rack-of-track SEQ.current-track)
                        -1)))
        (if (>= by-track 0) (rack/group-id by-track) -1)))))

(defcustom groove-buffer-auto-split true
  :type :bool
  :doc "Split the sidebar to show the rack's groove under the browser while a drum rack is selected.")

;; Whether the sidebar splits: the setting is on, a rack is selected and the
;; rack has a groove entry to show.
(def showing? ()
  (let ((gid (selected-rack-id)))
    (and groove-buffer-auto-split
         (>= gid 0)
         (not (= (rack/groove-state gid) nil)))))

;; Re-lay the sidebar when the answer flips. `seen` is a plain global so the
;; write does not re-trigger this observer; the first run (at load, before a
;; layout exists) only records it.
(def showing-seen nil)

(observe
  (let ((now (if (showing?) "split" "plain")))
    (do
      (if (and showing-seen
               (not (= showing-seen now))
               eseq.seq-core-state/samples-sidebar-visible)
        (eseq.seq-layout/refresh-current-layout)
        nil)
      (set! showing-seen now))))

;; ── Extract Groove modal ────────────────────────────────────────────────
(defstate extract-open? false)
(defstate extract-gid -1)
(defstate extract-name "")
(defstate extract-bars "1 bar")
(defstate extract-resolution "1/16")
(defstate extract-quantize true)

(def open-extract (gid)
  (do
    (set! extract-gid gid)
    (set! extract-name (str "Groove " (+ 1 (len (or SEQ.groove-pool (list))))))
    (set! extract-bars "1 bar")
    (set! extract-resolution "1/16")
    (set! extract-quantize true)
    (set! extract-open? true)))

(def close-extract () (set! extract-open? false))

(def commit-extract ()
  (do
    (rack/extract-groove extract-gid
      (if (= (len (string-trim extract-name)) 0) "Groove" extract-name)
      (if (= extract-bars "2 bars") 2 1)
      extract-resolution
      extract-quantize)
    (close-extract)))

(def extract-modal ()
  (modal :is-open extract-open? :on-close (lambda () (close-extract))
      :width-px 760 :height-px 520
    (box :debug-name "rack-groove-extract-panel" :width :fill :height :fill
      :padding 0.6 :bg :transparent
      (if extract-open?
        (v-stack :width :fill :height :fill :gap 0.5
          (label "Extract Groove" :key "rack-groove-extract-title"
            :font-size 16 :color :white :bg :transparent)
          (label "Name" :font-size 10 :color :dim :bg :transparent)
          (text-input :key "rack-groove-extract-name" :width :fill :height 1.3 :font-size 12
            :value extract-name
            :auto-focus true
            :select-all-on-focus true
            :on-change (lambda (v) (set! extract-name v))
            :on-submit (lambda () (commit-extract))
            :on-cancel (lambda () (close-extract)))
          (h-stack :gap 1 :align :center
            (label "Period" :width 5 :font-size 10 :color :dim :bg :transparent)
            (dropdown :key "rack-groove-extract-bars"
              :value extract-bars :options '("1 bar" "2 bars")
              :width 7 :height 1.0 :font-size 9
              :on-change (lambda (v) (set! extract-bars v)))
            (label "Grid" :width 3.5 :font-size 10 :color :dim :bg :transparent)
            (dropdown :key "rack-groove-extract-resolution"
              :value extract-resolution :options '("1/16" "1/32")
              :width 6 :height 1.0 :font-size 9
              :on-change (lambda (v) (set! extract-resolution v))))
          (h-stack :gap 0.6 :align :center
            (toggle :key "rack-groove-extract-quantize"
              :value extract-quantize
              :on-change (lambda (v) (set! extract-quantize v)))
            (label "Quantize source afterwards"
              :font-size 10 :color :white :bg :transparent))
          (box :flex 1 :bg :transparent)
          (h-stack :width :fill :gap 0.5
            (box :flex 1 :bg :transparent)
            (button "Cancel" :key "rack-groove-extract-cancel"
              :on-click |x y r| (close-extract))
            (button "Extract" :key "rack-groove-extract-submit" :variant :primary
              :on-click |x y r| (commit-extract))))
        (box :width 0 :height 0 :bg :transparent)))))

;; ── Groove actions (the ≡ menu beside the picker) ───────────────────────
;; Inline rename of the active pool groove: its id, or -1.
(defstate renaming -1)
(defstate rename-draft "")

(def active-groove-id (gid)
  (let ((state (rack/groove-state gid)))
    (if (= state nil) -1 (get state :active-groove-id))))

(def active-groove-name (gid)
  (let ((id (active-groove-id gid))
        (hits (filter (lambda (entry) (= (get entry :id) id)) (or SEQ.groove-pool (list)))))
    (if (> (len hits) 0) (get (nth hits 0) :name) "")))

(def action-extract "Extract from this rack’s clip…")
(def action-save "Save to Library")
(def action-rename "Rename…")
(def action-duplicate "Duplicate")
(def action-delete "Delete from Project")
(def action-follow "Use Rack Groove for This Clip")
(def action-all "Apply to All Clips in This Rack")

;; Extract is always there; Save…Delete act on the playing groove; a rack
;; with clips adds Apply to All, and "Use Rack Groove" while the clip has its
;; own (any edit gives a clip its own groove).
(def menu-actions (gid)
  (let ((clip (rack/active-clip gid))
        (groove (if (< (active-groove-id gid) 0)
                  (list)
                  (list action-save action-rename action-duplicate action-delete))))
    (append
      (append (list action-extract) groove)
      (if (< clip 0)
        (list)
        (if (rack/clip-owns-groove? gid clip)
          (list action-follow action-all)
          (list action-all))))))

(def begin-rename (gid)
  (do
    (set! rename-draft (active-groove-name gid))
    (set! renaming (active-groove-id gid))))

(def cancel-rename () (set! renaming -1))

(def commit-rename ()
  (let ((id renaming)
        (name (string-trim rename-draft)))
    (do
      (set! renaming -1)
      (if (or (< id 0) (= (len name) 0))
        nil
        (host-command "rename-rack-groove" (dict :groove-id id :name name))))))

(def select-menu-action (gid chosen)
  (let ((groove (active-groove-id gid)))
    (if (= chosen action-extract) (open-extract gid)
      (if (= chosen action-save)
        (host-command "save-groove-to-library" (dict :groove-id groove))
        (if (= chosen action-rename) (begin-rename gid)
          (if (= chosen action-duplicate)
            (host-command "duplicate-pool-groove" (dict :groove-id groove))
            (if (= chosen action-delete)
              (host-command "delete-rack-groove" (dict :groove-id groove))
              (if (= chosen action-follow)
                (host-command "set-rack-clip-own-groove"
                  (dict :group-id gid :clip-id (rack/active-clip gid) :own false))
                (if (= chosen action-all)
                  (host-command "apply-rack-groove-to-all-clips"
                    (dict :group-id gid :clip-id (rack/groove-clip-id gid)))
                  nil)))))))))

(def actions-menu (gid)
  (menu-button
    :key (str "rack-groove-actions-" gid)
    :debug-name "rack-groove-actions"
    :icon "≡"
    :options (menu-actions gid)
    :width 1.9 :height 1.3 :font-size 13
    :bg-color :mixer-control-bg
    :text-color :dim
    :menu-bg :dropdown-menu-bg
    :menu-border-color :dropdown-menu-border
    :hover-bg :dropdown-hover-bg
    :on-change (lambda (item) (select-menu-action gid item))))

;; ── Sizes ───────────────────────────────────────────────────────────────
(def name-width 9.0)
;; The track name gets a fixed column, then the note and any role badge:
;; every lane starts at the same x however long a member's track name is.
(def name-chars 7)
(def note-width 4.2)
(def amount-width 3.6)
(def dot-width 1.4)
(def row-height 1.1)

;; ── Header ──────────────────────────────────────────────────────────────
;; The picker's footer row: an action, not a groove.
(def extract-label "Extract from this rack’s clip…")

(def picker (gid state)
  (if (and (>= renaming 0) (= renaming (get state :active-groove-id)))
    (text-input :key (str "rack-groove-rename-" gid)
      :debug-name "rack-groove-rename"
      :flex 1 :height 1.3 :font-size 10
      :value rename-draft
      :auto-focus true
      :select-all-on-focus true
      :on-change (lambda (v) (set! rename-draft v))
      :on-submit (lambda () (commit-rename))
      :on-cancel (lambda () (cancel-rename)))
    (box :flex 1 :height 1.3 :padding 0 :bg :transparent
      (dropdown :key (str "rack-groove-picker-" gid)
        :debug-name "rack-groove-picker"
        :value (get state :active-label)
        :detail (get state :active-grid)
        :options (get state :picker-labels)
        :headers (or (get state :picker-headers) (list))
        :details (or (get state :picker-details) (list))
        :filterable true
        :filter-placeholder "Filter grooves…"
        :footer extract-label
        :width :fill :height 1.3 :font-size 10
        :on-change (lambda (label)
                     (if (= label extract-label)
                       (open-extract gid)
                       (rack/set-groove gid label)))))))

(def header (gid state)
  (let ((grooved (>= (get state :active-groove-id) 0)))
    (h-stack :key "rack-groove-header" :width :fill :gap 0.4 :align :center :height 1.6
      (picker gid state)
      (actions-menu gid)
      (if grooved
        (toggle :key (str "rack-groove-enabled-" gid)
          :debug-name "rack-groove-enabled"
          :value (get state :enabled)
          :on-change (lambda (v)
                       (host-command "set-rack-groove-enabled"
                         (dict :group-id gid :clip-id (rack/groove-edit-clip-id gid) :enabled v))))
        (box :width 0 :bg :transparent)))))

;; ── Amounts ─────────────────────────────────────────────────────────────
;; A chip per amount, styled like *step*'s param pickers
;; (effects/track-panels.lisp `step-param-picker`): name left, value right.
(def amount-picker (gid amount title max-value grooved)
  (box :flex 1 :corner-radius 16 :padding 0.2 :background-color :mixer-strip-bg
    (h-stack :align :center :gap 0.24
      (box :width 0.3)
      (label title :font-size 9 :color :dim :bg :transparent :v-align :center :flex 1)
      (number-picker :key (str "rack-groove-" amount "-" gid)
        :debug-name (str "rack-groove-" amount)
        :value (bind-seq (rack/groove-amount-field amount gid))
        :min 0 :max max-value :value-scale 100 :decimals 0 :unit "%"
        :noui true
        :font-size 9
        :text-color (if grooved :white :dim)
        :on-change (lambda (v) (rack/set-groove-amount gid amount v))
        :width 3.4
        :height 1.1))))

;; Scale: the groove's time scale. 2× plays a 1 bar · 1/16 groove as
;; 2 bars · 1/8, so a pattern moved to 1/8 steps at double tempo keeps the
;; same pocket; ½× the reverse.
(def scale-options '("½×" "1×" "2×"))

(def scale-value (label)
  (if (= label "½×") 0.5 (if (= label "2×") 2 1)))

(def scale-picker (gid state grooved)
  (box :flex 1 :corner-radius 16 :padding 0.2 :background-color :mixer-strip-bg
    (h-stack :align :center :gap 0.24
      (box :width 0.3)
      (label "Scale" :font-size 9 :color :dim :bg :transparent :v-align :center :flex 1)
      (dropdown :key (str "rack-groove-scale-" gid)
        :debug-name "rack-groove-scale"
        :value (or (get state :scale-label) "1×")
        :options scale-options
        :width 4.2 :height 1.1 :font-size 9
        :bg-color :mixer-strip-bg
        :text-color (if grooved :white :dim)
        :on-change (lambda (label)
                     (host-command "set-rack-groove-scale"
                       (dict :group-id gid :clip-id (rack/groove-edit-clip-id gid)
                             :scale (scale-value label))))))))

;; Two rows of two chips: Timing | Velocity, Random | Scale.
(def amounts (gid state grooved)
  (v-stack :key "rack-groove-amounts" :width :fill :gap 0.25
    (h-stack :width :fill :gap 0.3 :align :center
      (amount-picker gid "timing" "Timing" 1.5 grooved)
      (amount-picker gid "velocity" "Velocity" 1.5 grooved))
    (h-stack :width :fill :gap 0.3 :align :center
      (amount-picker gid "random" "Random" 1.0 grooved)
      (scale-picker gid state grooved))))

;; ── Lanes ───────────────────────────────────────────────────────────────
(def track-steps (track)
  (if (and (>= track 0) (< track (len (or SEQ.track-steps (list)))))
    (nth SEQ.track-steps track)
    (list)))

(def track-color (track)
  (if (and (>= track 0) (< track (len (or SEQ.track-colors (list)))))
    (let ((c (nth SEQ.track-colors track)))
      (rgba (nth c 0) (nth c 1) (nth c 2) 1.0))
    '(rgba 0.35 0.35 0.38 1.0)))

;; Longer names end in an ellipsis inside `name-chars`.
(def clip-name (name)
  (if (> (len name) name-chars)
    (str (substring name 0 (- name-chars 1)) "…")
    name))

;; A lane is labelled by its member track's name, never by a role guessed
;; from the pad's note; an explicitly set role shows as a badge instead.
(def pad-name (pad)
  (let ((track (get pad :track)))
    (if (and (>= track 0) (< track (len (or SEQ.track-names (list)))))
      (nth SEQ.track-names track)
      (get pad :label))))

;; A role the user set on the pad (right-click a rack pad ▸ Role) shows as
;; its drum-machine tag (BD, SD, CH…), styled like the browser's selected
;; tag chips. Roles guessed from the pad's note never label a lane.
(def role-badge (pad)
  (let ((tag (or (get pad :role-tag) "")))
    (if (= tag "")
      (box :width 0 :bg :transparent)
      (box :debug-name "rack-groove-role"
        :width 2.1 :height 0.85 :padding 0 :corner-radius 13
        :background-color :mixer-strip-selected-bg
        :border-color :dim
        :h-align :center :v-align :center
        (label tag :font-size 7 :color :fg :bg :transparent
          :h-align :center :v-align :center)))))

;; The All row's hits: a 16th hit when any pad plays it.
(def union-hits (pads slots)
  (map (lambda (i)
         (> (len (filter (lambda (pad)
                           (let ((steps (track-steps (get pad :track))))
                             (and (< i (len steps)) (nth steps i))))
                         pads))
            0))
       (range 0 slots)))

(def column-header ()
  (h-stack :key "rack-groove-columns" :width :fill :gap 0.3 :align :center :height 1.0
    (box :width 0.35 :bg :transparent)
    (label "PAD" :width (+ name-width 0.3) :height 1.0 :font-size 7.5 :color :dim :bg :transparent :v-align :center)
    (h-stack :flex 1 :gap 0 :height 1.0
      (each (range 0 4) |beat|
        (label (str (+ 1 beat)) :flex 1 :height 1.0 :font-size 7.5 :color :dim :bg :transparent :v-align :center)))
    (label "AMT" :width amount-width :height 1.0 :font-size 7.5 :color :dim :bg :transparent :h-align :right :v-align :center)
    (box :width dot-width :bg :transparent)
    (box :width 0.6 :bg :transparent)))

(def lane-late '(rgba 0.29 0.56 1.0 1.0))
(def lane-early '(rgba 0.90 0.66 0.24 1.0))
(def lane-off '(rgba 0.45 0.45 0.5 0.55))

;; An excluded pad keeps its lane, greyed, so the feel it would get stays
;; readable.
(def lane (key cells measured hits slots enabled)
  (groove-lane :key key
    :flex 1 :height (- row-height 0.15)
    :cells cells :measured measured :hits hits :slots slots
    :late-color (if enabled lane-late lane-off)
    :early-color (if enabled lane-early lane-off)))

(def all-row (gid lanes grooved)
  (let ((slots (get lanes :slots)))
    (h-stack :key (str "rack-groove-all-" gid) :debug-name "rack-groove-all"
      :width :fill :gap 0.3 :align :center :height row-height
      :background-color '(rgba 1 1 1 0.03)
      (box :width 0.35 :height 0.9 :padding 0 :bg :transparent
        (box :width 0.3 :height 0.9 :background-color '(rgba 0.4 0.4 0.44 1.0) :corner-radius 3))
      (label "All" :width (+ name-width 0.3) :height row-height :font-size 9 :color :white :bg :transparent :v-align :center)
      (lane (str "rack-groove-all-lane-" gid)
        (get lanes :all-cells) (get lanes :all-measured)
        (if grooved (list) (union-hits (get lanes :pads) slots))
        slots true)
      (box :width amount-width :bg :transparent)
      (box :width dot-width :bg :transparent)
      (box :width 0.6 :bg :transparent))))

(def pad-row (gid pad slots grooved)
  (let ((note (get pad :pad-note))
        (enabled (get pad :enabled))
        (current (= (get pad :track) SEQ.current-track)))
    (subtree :key (str "rack-groove-pad-" gid "-" note)
      ;; The selected track's lane, subtly lit.
      (box :width :fill :height row-height :padding 0 :corner-radius 6
        :background-color (if current '(rgba 1 1 1 0.07) '(rgba 1 1 1 0.0))
      (h-stack :key (str "rack-groove-pad-row-" gid "-" note) :debug-name "rack-groove-pad"
        :width :fill :gap 0.3 :align :center :height row-height
        (box :width 0.35 :height 0.9 :padding 0 :bg :transparent
          (box :width 0.3 :height 0.9 :background-color (track-color (get pad :track)) :corner-radius 3))
        (box :width (- name-width note-width) :height row-height :padding 0 :bg :transparent
          (label (clip-name (pad-name pad)) :height row-height :font-size 9 :bg :transparent :v-align :center
            :color (if (and grooved (not enabled)) :dim :white)))

        ;; The note gets a fixed slot ("F4" and "F#4" alike), so every role
        ;; badge starts at the same x.
        (h-stack :width note-width :height row-height :gap 0.2 :align :center
          (box :width 1.9 :height row-height :padding 0 :bg :transparent
            (label (get pad :label) :height row-height :font-size 7 :color :dim :bg :transparent :v-align :center))
          (role-badge pad))
        (lane (str "rack-groove-pad-lane-" gid "-" note)
          (get pad :cells) (get pad :measured)
          (if grooved (list) (track-steps (get pad :track)))
          slots (or (not grooved) enabled))
        (if (and grooved enabled)
          (number-picker :key (str "rack-groove-pad-amount-" gid "-" note)
            :debug-name "rack-groove-pad-amount"
            :value (bind-seq (get pad :amount-field))
            :min 0 :max 1 :value-scale 100 :decimals 0 :unit "%"
            :noui true
            :font-size 8.5 :text-color :white
            :width amount-width :height 1.0
            :on-change (lambda (v)
                         (host-command "set-rack-groove-pad-amount"
                           (dict :group-id gid :clip-id (rack/groove-edit-clip-id gid)
                                 :pad-note note :value v))))
          (label (if grooved "off" "") :width amount-width :height row-height :font-size 8.5
            :color :dim :bg :transparent :h-align :right :v-align :center))
        ;; The include dot: a small filled circle while the pad plays the
        ;; groove, a hollow one while it stays straight.
        (if grooved
          (box :width dot-width :height row-height :padding 0 :bg :transparent
            :h-align :center :v-align :center
            (button ""
              :key (str "rack-groove-pad-enabled-" gid "-" note)
              :debug-name "rack-groove-pad-enabled"
              :width 0.9 :height 0.42 :padding 0
              :corner-radius 16
              :background-color (if enabled :blue '(rgba 1 1 1 0.0))
              :border-color (if enabled :blue :dim)
              :shadow-color '(rgba 0 0 0 0.0)
              :highlight-color '(rgba 1 1 1 0.0)
              :on-click |x y r| (host-command "set-rack-groove-pad-enabled"
                                  (dict :group-id gid :clip-id (rack/groove-edit-clip-id gid)
                                        :pad-note note :enabled (not enabled)))))
          (box :width dot-width :bg :transparent))
        (box :width 0.6 :bg :transparent))))))

(def lanes-view (gid state)
  (let ((lanes (get state :lanes))
        (grooved (>= (get state :active-groove-id) 0)))
    (v-stack :key "rack-groove-lanes" :width :fill :gap 0 :flex 1
      (column-header)
      (all-row gid lanes grooved)
      (box :width :fill :flex 1 :padding 0 :bg :transparent
        (scroll :key (str "rack-groove-lanes-scroll-" gid) :debug-name "rack-groove-lanes-scroll"
          :width :fill :flex 1
          (v-stack :width :fill :gap 0
            (each (get lanes :pads) |pad|
              (pad-row gid pad (get lanes :slots) grooved))))))))

;; ── Panel ───────────────────────────────────────────────────────────────
(def panel ()
  (let ((gid (selected-rack-id))
        (state (if (>= gid 0) (rack/groove-state gid) nil)))
    (if (= state nil)
      (box :key "rack-groove-empty" :width :fill :flex 1 :bg :transparent)
      (v-stack :key (str "rack-groove-buffer-" gid) :debug-name "rack-groove-buffer"
        :width :fill :flex 1 :gap 0.25
        (header gid state)
        (amounts gid state (>= (get state :active-groove-id) 0))
        (lanes-view gid state)))))

(def root-widget ()
  (v-stack :width :fill :height :fill :gap 0 :padding 0.15
    (panel)
    (extract-modal)))

;; Widget-only buffer: take the shared sequencer keymap, like *samples*.
(set-buffer-mode-for "*groove*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*groove*"
  (root-widget))
