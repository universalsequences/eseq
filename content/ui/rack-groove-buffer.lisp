;; ui/rack-groove-buffer.lisp — the *groove* buffer: the selected drum rack's
;; groove, stacked under the browser (docs/rack-groove-spec.md; bead
;; eseq-yks3).
;;
;; While a drum rack is selected (its bus, or a track inside it) the sidebar
;; splits in two: the browser keeps the top 70% and this buffer takes the
;; bottom 30% (eseq.seq-layout/samples-sidebar-layout-spec). It only ever
;; shows THIS rack's groove, the one it plays now (the playing clip's own,
;; else the rack's: eseq.drum-rack-v2/playing-groove):
;;
;;   header   groove picker (filterable; its footer extracts) · on/off
;;   amounts  Timing / Velocity / Random / Scale
;;   lanes    All + one row per pad: where each 16th lands (late up, early
;;            down), the pad's share of the groove and its include dot. With
;;            no groove the lanes show the pads' hits sitting on the grid.
;;
;; Everything is a kind field (`groove`, `pad-groove`, `pool-groove`,
;; `library-groove`): the amounts and shares bind (`#'gr.timing`, `#'q.amount`),
;; so dragging a number never rebuilds the lanes, and every edit is a groove
;; setter or action of eseq.kinds. The ≡ menu beside the picker extracts a
;; new groove and saves, renames, duplicates or deletes the playing one.
(module eseq.rack-groove-buffer)

(import eseq.seq-core-state)
(import eseq.drum-rack-v2 :as rack)
(import eseq.view-kit :refer (listed? index-of nothing color-rgba))
(import eseq.kinds :refer (groups selection project set-clip-groove! use-library-groove!
                           apply-groove-to-all-clips! extract-groove! duplicate-groove!
                           delete-groove! save-groove-to-library! groove-scale-options))

(export selected-rack
        showing?
        panel
        extract-modal
        groove-extract
        groove-rename
        open-extract
        commit-extract
        close-extract
        menu-actions
        select-menu-action
        commit-rename
        cancel-rename
        edit-groove!)

;; ── Which rack ──────────────────────────────────────────────────────────
;; The rack whose bus is selected (what *fx* shows), else the rack the
;; current track belongs to; nil when neither is a drum rack.
(def selected-rack ()
  (or (rack/selected-bus-rack)
      (let ((t selection.track))
        (when t (rack/rack-of-track t)))))

(defcustom groove-buffer-auto-split true
  :type :bool
  :doc "Split the sidebar to show the rack's groove under the browser while a drum rack is selected.")

;; Whether the sidebar splits: the setting is on and a rack with a groove to
;; show is selected.
(def showing? ()
  (let ((g (selected-rack)))
    (if (and groove-buffer-auto-split g (rack/playing-groove g)) true false)))

;; Re-lay the sidebar when the answer flips. `seen` is a plain global so the
;; write does not re-trigger this observer; the first run (at load, before a
;; layout exists) only records it.
(def showing-seen nil)

(observe
  (let ((now (if (showing?) "split" "plain")))
    (when (and showing-seen
               (not (= showing-seen now))
               eseq.seq-core-state/samples-sidebar-visible)
      (eseq.seq-layout/refresh-current-layout))
    (set! showing-seen now)))

;; ── Editing ─────────────────────────────────────────────────────────────
;; Clips never share edits: an edit goes to the clip rack g plays
;; (eseq.kinds/set-clip-groove!), so one that follows the rack's groove gets
;; its own (a copy, `rc.own-groove`) and the edit in one undo entry; a rack
;; without clips, or silent, edits its own. `field` is set-clip-groove!'s;
;; with :pad p, that pad's share.
(def edit-groove! (g field v &key (pad nil))
  (set-clip-groove! g g.rack-clip field v :pad pad))

;; ── Extract Groove modal ────────────────────────────────────────────────
;; Open over `group`, the rack it extracts from.
(def-kind groove-extract
  :key ()
  :state ((open false)
          (group group :default nil)
          (name "")
          (bars "1 bar")
          (resolution "1/16")
          (quantize true)))

(def open-extract (g)
  (set! groove-extract.group g)
  (set! groove-extract.name (str "Groove " (+ 1 (len project.groove-pool))))
  (set! groove-extract.bars "1 bar")
  (set! groove-extract.resolution "1/16")
  (set! groove-extract.quantize true)
  (set! groove-extract.open true))

(def close-extract () (set! groove-extract.open false))

;; The rack can be gone by now (a project load, an undo): extract only from
;; a listed one.
(def commit-extract ()
  (let ((g groove-extract.group)
        (name groove-extract.name))
    (when (listed? g (groups))
      (extract-groove! g
        (if (empty? (string-trim name)) "Groove" name)
        (if (= groove-extract.bars "2 bars") 2 1)
        groove-extract.resolution
        groove-extract.quantize))
    (close-extract)))

(def extract-modal ()
  (modal :is-open groove-extract.open :on-close (lambda () (close-extract))
      :width-px 760 :height-px 520
    (box :debug-name "rack-groove-extract-panel" :width :fill :height :fill
      :padding 0.6 :bg :transparent
      (if groove-extract.open
        (v-stack :width :fill :height :fill :gap 0.5
          (label "Extract Groove" :key "rack-groove-extract-title"
            :font-size 16 :color :white :bg :transparent)
          (label "Name" :font-size 10 :color :dim :bg :transparent)
          (text-input :key "rack-groove-extract-name" :width :fill :height 1.3 :font-size 12
            :value groove-extract.name
            :auto-focus true
            :select-all-on-focus true
            :on-change (lambda (v) (set! groove-extract.name v))
            :on-submit (lambda () (commit-extract))
            :on-cancel (lambda () (close-extract)))
          (h-stack :gap 1 :align :center
            (label "Period" :width 5 :font-size 10 :color :dim :bg :transparent)
            (dropdown :key "rack-groove-extract-bars"
              :value groove-extract.bars :options '("1 bar" "2 bars")
              :width 7 :height 1.0 :font-size 9
              :on-change (lambda (v) (set! groove-extract.bars v)))
            (label "Grid" :width 3.5 :font-size 10 :color :dim :bg :transparent)
            (dropdown :key "rack-groove-extract-resolution"
              :value groove-extract.resolution :options '("1/16" "1/32")
              :width 6 :height 1.0 :font-size 9
              :on-change (lambda (v) (set! groove-extract.resolution v))))
          (h-stack :gap 0.6 :align :center
            (toggle :key "rack-groove-extract-quantize"
              :value groove-extract.quantize
              :on-change (lambda (v) (set! groove-extract.quantize v)))
            (label "Quantize source afterwards"
              :font-size 10 :color :white :bg :transparent))
          (box :flex 1 :bg :transparent)
          (h-stack :width :fill :gap 0.5
            (box :flex 1 :bg :transparent)
            (button "Cancel" :key "rack-groove-extract-cancel"
              :on-click |x y r| (close-extract))
            (button "Extract" :key "rack-groove-extract-submit" :variant :primary
              :on-click |x y r| (commit-extract))))
        (nothing)))))

;; ── Groove actions (the ≡ menu beside the picker) ───────────────────────
;; Inline rename of a pool groove: the groove and the name being typed.
(def-kind groove-rename
  :key ()
  :state ((groove pool-groove :default nil)
          (draft "")))

(def action-extract rack/groove-extract-label)
(def action-save "Save to Library")
(def action-rename "Rename…")
(def action-duplicate "Duplicate")
(def action-delete "Delete from Project")
(def action-follow "Use Rack Groove for This Clip")
(def action-all "Apply to All Clips in This Rack")

;; Extract is always there; Save…Delete act on the playing groove's pool
;; groove; a rack with clips adds Apply to All, and "Use Rack Groove" while
;; the playing clip has its own (any edit gives a clip its own groove).
(def menu-actions (g)
  (let ((rc g.rack-clip)
        (gr (rack/playing-groove g)))
    (append
      (append (list action-extract)
        (if (and gr gr.pool-groove)
          (list action-save action-rename action-duplicate action-delete)
          (list)))
      (cond
        ((= rc nil) (list))
        (rc.own-groove (list action-follow action-all))
        (else (list action-all))))))

(def begin-rename (pg)
  (set! groove-rename.draft pg.name)
  (set! groove-rename.groove pg))

(def cancel-rename () (set! groove-rename.groove nil))

;; The groove can be gone by now (deleted elsewhere): rename only a listed one.
(def commit-rename ()
  (let ((pg groove-rename.groove)
        (name (string-trim groove-rename.draft)))
    (cancel-rename)
    (when (and pg (listed? pg project.groove-pool) (not (empty? name)))
      (set! pg.name name))))

(def select-menu-action (g chosen)
  (let ((rc g.rack-clip)
        (gr (rack/playing-groove g))
        (pg (when gr gr.pool-groove)))
    (cond
      ((= chosen action-extract) (open-extract g))
      ((= chosen action-follow) (when rc (set! rc.own-groove false)))
      ((= chosen action-all) (when gr (apply-groove-to-all-clips! gr)))
      ((= pg nil) nil)
      ((= chosen action-save) (save-groove-to-library! pg))
      ((= chosen action-rename) (begin-rename pg))
      ((= chosen action-duplicate) (duplicate-groove! pg))
      ((= chosen action-delete) (delete-groove! pg)))))

(def actions-menu (g)
  (menu-button
    :key (str "rack-groove-actions-" g.gid)
    :debug-name "rack-groove-actions"
    :icon "≡"
    :options (menu-actions g)
    :width 1.9 :height 1.3 :font-size 13
    :bg-color :mixer-strip-bg
    :text-color :dim
    :menu-bg :dropdown-menu-bg
    :menu-border-color :dropdown-menu-border
    :hover-bg :dropdown-hover-bg
    :on-change (lambda (item) (select-menu-action g item))))

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
;; The picker's rows, in order: No groove, the pool under *This project*, the
;; factory files under *Factory* and the user's under *Library* (picking a
;; file copies it into the pool). Each row is `(dict :label :detail)` plus
;; what it picks: `:off`, `:pool pg`, `:library lg`, or `:header` (picks
;; nothing). Labels are unique, so a picked label names one row: a pool
;; groove's as eseq.drum-rack-v2/pool-groove-labels, a file's name else its
;; name with "(library)", else with its picker key.

;; The dim text beside a pool groove: where else it plays ("on Tape Kit",
;; "on 2 racks"), else its grid.
(def pool-detail (g pg)
  (let ((others (filter (lambda (r) (not (= r g))) pg.racks))
        (one (first others)))
    (cond
      ((= one nil) pg.grid)
      ((= (len others) 1) (str "on " one.name))
      (else (str "on " (len others) " racks")))))

(def library-label (lg labels)
  (let ((taken (lambda (l) (or (listed? l labels) (listed? l rack/groove-reserved-labels))))
        (named (if (taken lg.name) (str lg.name " (library)") lg.name)))
    (if (listed? named labels) (str lg.name " (" lg.choice ")") named)))

(def row-labels (rows) (map (lambda (row) (get row :label)) rows))

(def library-rows (rows tier header)
  (let ((files (filter (lambda (lg) (= lg.tier tier)) project.groove-library)))
    (if (empty? files)
      rows
      (reduce |acc lg|
        (append acc
          (list (dict :label (library-label lg (row-labels acc)) :detail "" :library lg)))
        (append rows (list (dict :label header :detail "" :header true)))
        files))))

(def picker-rows (g)
  (let ((pool project.groove-pool)
        (labels (rack/pool-groove-labels))
        (head (append
                (list (dict :label rack/groove-off-label :detail "" :off true))
                (if (empty? pool)
                  (list)
                  (list (dict :label "This project" :detail "" :header true)))))
        (pooled (append head
                  (map (lambda (i)
                         (let ((pg (nth pool i)))
                           (dict :label (nth labels i) :detail (pool-detail g pg) :pool pg)))
                    (range 0 (len pool))))))
    (library-rows (library-rows pooled "factory" "Factory") "user" "Library")))

;; The picked row's edit of the groove rack g plays. A pool groove can be
;; gone by now (deleted elsewhere): pick only a listed one.
(def pick-row (g row)
  (let ((pg (get row :pool))
        (lg (get row :library))
        (gr (rack/playing-groove g)))
    (cond
      ((get row :off) (edit-groove! g "pool-groove" nil))
      (pg (when (listed? pg project.groove-pool) (edit-groove! g "pool-groove" pg)))
      ((and lg gr) (use-library-groove! gr lg :clip g.rack-clip)))))

(def pick (g rows label)
  (if (= label rack/groove-extract-label)
    (open-extract g)
    (let ((row (first (filter (lambda (r) (= (get r :label) label)) rows))))
      (when (and row (not (get row :header)))
        (pick-row g row)))))

;; The playing groove's row label: its pool groove's, else No groove.
(def active-label (rows gr)
  (let ((pg gr.pool-groove)
        (row (when pg (first (filter (lambda (r) (= (get r :pool) pg)) rows)))))
    (if row (get row :label) rack/groove-off-label)))

(def picker (g gr)
  (let ((pg gr.pool-groove))
    (if (and pg (= groove-rename.groove pg))
      (text-input :key (str "rack-groove-rename-" g.gid)
        :debug-name "rack-groove-rename"
        :flex 1 :height 1.3 :font-size 10
        :value groove-rename.draft
        :auto-focus true
        :select-all-on-focus true
        :on-change (lambda (v) (set! groove-rename.draft v))
        :on-submit (lambda () (commit-rename))
        :on-cancel (lambda () (cancel-rename)))
      (let ((rows (picker-rows g)))
        (box :flex 1 :height 1.3 :padding 0 :bg :transparent
          (dropdown :key (str "rack-groove-picker-" g.gid)
            :debug-name "rack-groove-picker"
            :value (active-label rows gr)
            :detail gr.grid
            :options (row-labels rows)
            :headers (filter (lambda (i) (get (nth rows i) :header)) (range 0 (len rows)))
            :badge-color :transparent
            :bg-color :mixer-strip-bg
            :details (map (lambda (row) (get row :detail)) rows)
            :filterable true
            :filter-placeholder "Filter grooves…"
            :footer rack/groove-extract-label
            :width :fill :height 1.3 :font-size 10
            :on-change (lambda (label) (pick g rows label))))))))

(def header (g gr grooved)
  (subtree :key "rack-groove-header"
    (h-stack :width :fill :gap 0.4 :align :center :height 1.6 :padding 0.4
      (picker g gr)
      (actions-menu g)
      (if grooved
        (toggle :key (str "rack-groove-enabled-" g.gid)
          :debug-name "rack-groove-enabled"
          :value #'gr.enabled
          :on-change (lambda (v) (edit-groove! g "enabled" v)))
        (nothing)))))

;; ── Amounts ─────────────────────────────────────────────────────────────
;; A chip per amount, styled like *step*'s param pickers
;; (effects/track-panels.lisp `step-param-picker`): name left, value right.
;; `amount` is the groove field, `value` its binding.
(def amount-picker (g amount title value max-value grooved)
  (box :flex 1 :corner-radius 16 :padding 0.2 :background-color :mixer-strip-bg
    (h-stack :align :center :gap 0.24
      (box :width 0.3)
      (label title :font-size 9 :color :dim :bg :transparent :v-align :center :flex 1)
      (number-picker :key (str "rack-groove-" amount "-" g.gid)
        :debug-name (str "rack-groove-" amount)
        :value value
        :min 0 :max max-value :value-scale 100 :decimals 0 :unit "%"
        :noui true
        :font-size 9
        :text-color (if grooved :white :dim)
        :on-change (lambda (v) (edit-groove! g amount v))
        :width 3.4
        :height 1.1))))

;; Scale: the groove's time scale (eseq.kinds' groove-scale-options, whose
;; labels these are, in order). 2× plays a 1 bar · 1/16 groove as 2 bars ·
;; 1/8, so a pattern moved to 1/8 steps at double tempo keeps the same
;; pocket; ½× the reverse.
(def scale-options '("½×" "1×" "2×"))

(def scale-value (label)
  (nth groove-scale-options (index-of scale-options label)))

(def scale-label (scale)
  (let ((at (index-of groove-scale-options scale)))
    (nth scale-options (if (< at 0) 1 at))))

(def scale-picker (g gr grooved)
  (box :flex 1 :corner-radius 16 :padding 0.2 :background-color :mixer-strip-bg
    (h-stack :align :center :gap 0.24
      (box :width 0.3)
      (label "Scale" :font-size 9 :color :dim :bg :transparent :v-align :center :flex 1)
      (dropdown :key (str "rack-groove-scale-" g.gid)
        :debug-name "rack-groove-scale"
        :value (scale-label gr.scale)
        :options scale-options
        :width 4.2 :height 1.1 :font-size 9
        :bg-color :mixer-strip-bg
        :text-color (if grooved :white :dim)
        :on-change (lambda (label) (edit-groove! g "scale" (scale-value label)))))))

;; Two rows of two chips: Timing | Velocity, Random | Scale.
(def amounts (g gr grooved)
  (subtree :key "rack-groove-amounts"
    (v-stack :width :fill :gap 0.25
      (h-stack :width :fill :gap 0.3 :align :center
        (amount-picker g "timing" "Timing" #'gr.timing 1.5 grooved)
        (amount-picker g "velocity" "Velocity" #'gr.velocity 1.5 grooved))
      (h-stack :width :fill :gap 0.3 :align :center
        (amount-picker g "random" "Random" #'gr.random 1.0 grooved)
        (scale-picker g gr grooved)))))

;; ── Lanes ───────────────────────────────────────────────────────────────
;; Track t's hits over the first `slots` steps.
(def track-hits (t slots)
  (let ((steps t.steps))
    (map (lambda (i) (let ((s (nth steps i))) (if (and s s.active) true false)))
      (range 0 slots))))

;; Longer names end in an ellipsis inside `name-chars`.
(def clip-name (name)
  (if (> (len name) name-chars)
    (str (substring name 0 (- name-chars 1)) "…")
    name))

;; A role the user set on the pad (right-click a rack pad ▸ Role) shows as
;; its drum-machine tag (BD, SD, CH…), styled like the browser's selected
;; tag chips. Roles guessed from the pad's note never label a lane: a lane is
;; labelled by its member track's name.
(def role-badge (p)
  (if (empty? p.role)
    (nothing)
    (box :debug-name "rack-groove-role"
      :width 2.1 :height 0.85 :padding 0 :corner-radius 13
      :background-color :mixer-strip-selected-bg
      :border-color :dim
      :h-align :center :v-align :center
      (label p.role-tag :font-size 7 :color :fg :bg :transparent
        :h-align :center :v-align :center))))

;; The All row's hits: a 16th hit when any pad plays it.
(def union-hits (shares slots)
  (reduce |acc q|
    (let ((hits (track-hits q.pad.track slots)))
      (map (lambda (i) (or (nth acc i) (nth hits i))) (range 0 slots)))
    (map (lambda (i) false) (range 0 slots))
    shares))

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

(def all-row (g gr shares grooved)
  (subtree :key "rack-groove-all"
    (let ((slots gr.slots))
      (h-stack :key (str "rack-groove-all-" g.gid) :debug-name "rack-groove-all"
        :width :fill :gap 0.3 :align :center :height row-height
        :background-color '(rgba 1 1 1 0.03)
        (box :width 0.35 :height 0.9 :padding 0 :bg :transparent
          (box :width 0.3 :height 0.9 :background-color '(rgba 0.4 0.4 0.44 1.0) :corner-radius 3))
        (label "All" :width (+ name-width 0.3) :height row-height :font-size 9 :color :white :bg :transparent :v-align :center)
        (lane (str "rack-groove-all-lane-" g.gid)
          gr.cells gr.measured
          (if grooved (list) (union-hits shares slots))
          slots true)
        (box :width amount-width :bg :transparent)
        (box :width dot-width :bg :transparent)
        (box :width 0.6 :bg :transparent)))))

;; Pad q.pad's row; the selected track's lane, subtly lit.
(def pad-row (g q slots grooved)
  (let ((p q.pad)
        (t p.track)
        (note p.note)
        (enabled q.enabled))
    (subtree :key (str "rack-groove-pad-" g.gid "-" note)
      (box :width :fill :height row-height :padding 0 :corner-radius 6
        :selected #'t.selected
        :background-color '(rgba 1 1 1 0.0)
        :selected-background-color '(rgba 1 1 1 0.07)
      (h-stack :key (str "rack-groove-pad-row-" g.gid "-" note) :debug-name "rack-groove-pad"
        :width :fill :gap 0.3 :align :center :height row-height
        (box :width 0.35 :height 0.9 :padding 0 :bg :transparent
          (box :width 0.3 :height 0.9 :background-color (color-rgba t.color 1.0) :corner-radius 3))
        (box :width (- name-width note-width) :height row-height :padding 0 :bg :transparent
          (label (clip-name t.name) :height row-height :font-size 9 :bg :transparent :v-align :center
            :color (if (and grooved (not enabled)) :dim :white)))

        ;; The note gets a fixed slot ("F4" and "F#4" alike), so every role
        ;; badge starts at the same x.
        (h-stack :width note-width :height row-height :gap 0.2 :align :center
          (box :width 1.9 :height row-height :padding 0 :bg :transparent
            (label p.label :height row-height :font-size 7 :color :dim :bg :transparent :v-align :center))
          (role-badge p))
        (lane (str "rack-groove-pad-lane-" g.gid "-" note)
          q.cells q.measured
          (if grooved (list) (track-hits t slots))
          slots (or (not grooved) enabled))
        (if (and grooved enabled)
          (number-picker :key (str "rack-groove-pad-amount-" g.gid "-" note)
            :debug-name "rack-groove-pad-amount"
            :value #'q.amount
            :min 0 :max 1 :value-scale 100 :decimals 0 :unit "%"
            :noui true
            :font-size 8.5 :text-color :white
            :width amount-width :height 1.0
            :on-change (lambda (v) (edit-groove! g "pad-amount" v :pad p)))
          (label (if grooved "off" "") :width amount-width :height row-height :font-size 8.5
            :color :dim :bg :transparent :h-align :right :v-align :center))
        ;; The include dot: a small filled circle while the pad plays the
        ;; groove, a hollow one while it stays straight.
        (if grooved
          (box :width dot-width :height row-height :padding 0 :bg :transparent
            :h-align :center :v-align :center
            (button ""
              :key (str "rack-groove-pad-enabled-" g.gid "-" note)
              :debug-name "rack-groove-pad-enabled"
              :width 0.9 :height 0.42 :padding 0
              :corner-radius 16
              :background-color (if enabled :blue '(rgba 1 1 1 0.0))
              :border-color (if enabled :blue :dim)
              :shadow-color '(rgba 0 0 0 0.0)
              :highlight-color '(rgba 1 1 1 0.0)
              :on-click |x y r| (edit-groove! g "pad-enabled" (not enabled) :pad p)))
          (box :width dot-width :bg :transparent))
        (box :width 0.6 :bg :transparent))))))

(def lanes-view (g gr grooved)
  (subtree :key "rack-groove-lanes"
    (let ((shares (filter (lambda (q) q.pad) gr.pads))
          (slots gr.slots))
      (v-stack :width :fill :gap 0 :flex 1
        (column-header)
        (all-row g gr shares grooved)
        (box :width :fill :flex 1 :padding 0 :bg :transparent
          (scroll :key (str "rack-groove-lanes-scroll-" g.gid) :debug-name "rack-groove-lanes-scroll"
            :width :fill :flex 1
            (v-stack :width :fill :gap 0
              (each shares |q|
                (pad-row g q slots grooved)))))))))

;; ── Panel ───────────────────────────────────────────────────────────────
(def panel ()
  (let ((g (selected-rack))
        (gr (when g (rack/playing-groove g))))
    (if gr
      (let ((grooved (if gr.pool-groove true false)))
        (v-stack :key (str "rack-groove-buffer-" g.gid) :debug-name "rack-groove-buffer"
          :width :fill :flex 1 :gap 0.25
          (header g gr grooved)
          (amounts g gr grooved)
          (lanes-view g gr grooved)))
      (box :key "rack-groove-empty" :width :fill :flex 1 :bg :transparent))))

(def root-widget ()
  (v-stack :width :fill :height :fill :gap 0 :padding 0.15
    (panel)
    (extract-modal)))

;; Widget-only buffer: take the shared sequencer keymap, like *samples*.
(set-buffer-mode-for "*groove*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*groove*"
  (root-widget))
