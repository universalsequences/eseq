;; ui/grooves-tab.lisp — the browser's Grooves tab (docs/rack-groove-spec.md,
;; "Rev 2 UI"; bead eseq-groove.11).
;;
;; One tree beside Packages, in the same conventions: section headers
;; In use / Project / Library / Factory, a check + rack-count badge on pool
;; grooves some rack plays, and right-click menus that route to the groove
;; host commands the rack panel uses (src/ui/host_commands/rack_grooves.rs).
;; A project groove expands to its instances, one row per rack playing it
;; (`<rack> · T100 V40 R0`, percentages); clicking one focuses that rack. The
;; selected groove's pads x slots heatmap and its period/grid sit below the
;; tree. Rows come from `seq-groove-tree`
;; (src/ui/host_commands/grooves_tab.rs) over SEQ.groove-pool and
;; SEQ.groove-library.
(module eseq.grooves-tab)

(import eseq.seq-core-state)
(import eseq.drum-rack-v2 :as rack)

(export panel
        groove-context-menu
        tree-key
        offset-color
        selected-rack-id
        focus-rack
        show-groove
        select-item
        activate-item
        open-menu
        menu-actions
        select-menu-action
        commit-rename
        cancel-rename
        selected-path
        selected-key
        library-heat
        expand-instances
        rename-draft
        rename-item)

;; ── State ──
;; The selected row (its tree :path) and the groove it names: a picker key,
;; `pool:<id>`, `factory:<stem>` or `user:<stem>`. A library file's heatmap
;; is loaded once when selected; a pool groove's rides on SEQ.groove-pool.
(defstate selected-path "")
(defstate selected-key "")
(defstate library-heat nil)
;; Show every groove's instances (the tree's expand-all).
(defstate expand-instances false)
;; Right-click menu.
(def menu-open (state false))
(def menu-col (state 0))
(def menu-row (state 0))
(def menu-target (state nil))
;; Inline rename: the row being renamed (a pool or user library row).
(defstate rename-item nil)
(defstate rename-draft "")

;; Hazard (l) in browser.lisp: Rust focuses the tree by this exact key.
(def tree-key () "eseq.grooves-tab/grooves-tab-tree")

;; ── Rack targets ──
;; "Selected rack": the rack whose bus is selected (what *fx* shows), else
;; the rack the current track belongs to; -1 when neither is a drum rack.
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

;; Selecting a rack selects its bus, which is what the *fx* panel follows.
(def focus-rack (gid)
  (let ((gidx (rack/group-index-by-id gid)))
    (if (< gidx 0)
      (status "That rack no longer exists")
      (let ((bus (rack/bus-index gidx)))
        (if (< bus 0)
          (status (str (rack/group-name gidx) " has no bus to show"))
          (do
            (seq-clear-selection)
            (seq-clear-delete-target)
            (set! eseq.seq-core-state/selected-bus bus)
            (status (str "Showing " (rack/group-name gidx)))))))))

;; ── Selection + preview ──
(def pool-entry (key)
  (let ((hits (filter (lambda (entry) (= (get entry :key) key))
                (or SEQ.groove-pool (list)))))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def pool-key? (key)
  (string-starts-with? key "pool:"))

(def item-groove-key (item)
  (let ((kind (get item :kind)))
    (if (= kind "instance")
      (str "pool:" (get item :groove-id))
      (if (or (= kind "pool") (= kind "library")) (get item :key) ""))))

;; Select the groove `key` (any section) for the preview; the Rack panel's
;; "Grooves tab" link lands here.
(def show-groove (key path)
  (do
    (set! selected-key key)
    (set! selected-path path)
    (set! library-heat
      (if (or (= key "") (pool-key? key)) nil (seq-groove-library-heatmap key)))))

;; A click (or cursor move) selects the row's groove; an instance row also
;; focuses its rack.
(def select-item (item)
  (let ((key (item-groove-key item)))
    (if (= key "")
      nil
      (do
        (if (not (= key selected-key))
          (show-groove key (get item :path))
          (set! selected-path (get item :path)))
        (if (= (get item :kind) "instance")
          (focus-rack (get item :group-id))
          nil)))))

;; Enter / double-click: a groove applies to the selected rack, an instance
;; focuses its rack.
(def activate-item (item)
  (let ((kind (get item :kind)))
    (if (= kind "instance")
      (focus-rack (get item :group-id))
      (if (or (= kind "pool") (= kind "library"))
        (apply-to-selected-rack item)
        nil))))

(def apply-to-selected-rack (item)
  (let ((gid (selected-rack-id)))
    (if (< gid 0)
      (status "Select a drum rack to apply a groove to")
      (host-command "set-rack-groove" (dict :group-id gid :key (get item :key))))))

;; ── Context menu ──
(def action (id key label)
  (dict :id id :key key :label label))

;; Pool groove: Apply, Rename, Duplicate, Save to Library, Delete (the host
;; confirms, listing the racks, when some rack plays it). Library file:
;; Apply (copy-on-apply) and, for the user's own files, Rename and Delete.
;; Instance: Show Rack, Turn Off.
(def menu-actions ()
  (let ((item menu-target))
    (if (= item nil)
      (list)
      (let ((kind (get item :kind)))
        (if (= kind "pool")
          (list (action :apply "apply" "Apply to Selected Rack")
                (action :rename "rename" "Rename")
                (action :duplicate "duplicate" "Duplicate")
                (action :save "save" "Save to Library")
                (action :delete "delete" "Delete"))
          (if (= kind "library")
            (if (get item :read-only?)
              (list (action :apply "apply" "Apply to Selected Rack"))
              (list (action :apply "apply" "Apply to Selected Rack")
                    (action :rename "rename" "Rename")
                    (action :delete-library "delete" "Delete")))
            (if (= kind "instance")
              (list (action :focus "focus" "Show Rack")
                    (action :off "off" (str "Turn Off on " (get item :rack-name))))
              (list))))))))

(def open-menu (event)
  (let ((item (get event :item)))
    (do
      (set! menu-target item)
      (if (> (len (menu-actions)) 0)
        (do
          (select-item item)
          (set! menu-col (get event :col))
          (set! menu-row (get event :row))
          (set! menu-open true))
        (set! menu-target nil)))))

(def begin-rename (item)
  (do
    (set! rename-draft (if (= (get item :kind) "pool") (get item :groove-name) (get item :label)))
    (set! rename-item item)))

(def cancel-rename ()
  (set! rename-item nil))

(def commit-rename ()
  (let ((item rename-item)
        (name (string-trim rename-draft)))
    (do
      (set! rename-item nil)
      (if (or (= item nil) (= (len name) 0))
        nil
        (if (= (get item :kind) "pool")
          (host-command "rename-rack-groove" (dict :groove-id (get item :groove-id) :name name))
          (host-command "rename-library-groove" (dict :stem (get item :stem) :name name)))))))

(def select-menu-action (chosen)
  (let ((item menu-target)
        (id (get chosen :id)))
    (do
      (set! menu-open false)
      (if (= id :apply) (apply-to-selected-rack item)
        (if (= id :rename) (begin-rename item)
          (if (= id :duplicate)
            (host-command "duplicate-pool-groove" (dict :groove-id (get item :groove-id)))
            (if (= id :save)
              (host-command "save-groove-to-library" (dict :groove-id (get item :groove-id)))
              (if (= id :delete)
                (host-command "delete-rack-groove" (dict :groove-id (get item :groove-id)))
                (if (= id :delete-library)
                  (host-command "delete-library-groove" (dict :stem (get item :stem)))
                  (if (= id :focus) (focus-rack (get item :group-id))
                    (if (= id :off)
                      (host-command "set-rack-groove"
                        (dict :group-id (get item :group-id) :key "off"))
                      nil)))))))))))

(def groove-context-menu ()
  (context-menu :is-open menu-open
    :anchor-col menu-col
    :anchor-row menu-row
    :on-close (lambda () (set! menu-open false))
    (each (menu-actions) |chosen|
      (menu-item (get chosen :label)
        :key (str "groove-menu-" (get chosen :key))
        :on-select (lambda (event) (select-menu-action chosen))))))

;; ── Heatmap ──
;; Offsets are in slots; extraction keeps them within half a slot and 0.4
;; slot saturates. Measured cells at full strength, filled (guessed) ones
;; dimmed, so the eye reads what was played versus what was guessed. Shared
;; with the rack panel's map (effects/rack-groove.lisp).
(def heat-base '(0.13 0.14 0.155))
(def heat-late '(0.30 0.62 1.0))
(def heat-early '(1.0 0.64 0.24))

(def mix (a b t) (+ a (* (- b a) t)))

(def offset-color (offset measured)
  (let ((m (min 1 (* 2.5 (abs offset))))
        (hue (if (< offset 0) heat-early heat-late))
        (strength (if measured 1.0 0.38)))
    (rgba (mix (nth heat-base 0) (nth hue 0) (* m strength))
          (mix (nth heat-base 1) (nth hue 1) (* m strength))
          (mix (nth heat-base 2) (nth hue 2) (* m strength))
          (if measured 1.0 0.7))))

;; The sidebar is 34-42 columns wide, the tab rail takes about 13 of them,
;; so the map gets ~20 columns: a narrow label column and cells that share
;; whatever width is left (:flex 1), so a 1 bar 1/16 map keeps all 16 slots
;; on its backdrop at any sidebar width.
(def heat-label-width 4.6)
(def heat-row-height 0.95)
;; A surface lighter than :buffer-bg, like the rack panel's, so filled cells
;; (offset-color's dim base) stay visible.
(def heat-backdrop '(rgba 0.18 0.2 0.22 1.0))

(def heat-row (row index)
  (let ((cells (get row :cells))
        (measured (get row :measured)))
    (h-stack :key (str "groove-tab-heat-row-" index) :width :fill :gap 0.06 :align :center
      (box :width heat-label-width :height heat-row-height :padding 0
        :v-align :center :h-align :start :bg :transparent
        (label (get row :label)
          :font-size 7.5 :v-align :center
          :color (if (= (get row :pad-note) nil) :dim :white) :bg :transparent))
      (each (range 0 (len cells)) |i|
        (box :key (str "groove-tab-heat-" index "-" i)
          :flex 1 :height heat-row-height :padding 0 :bg :transparent :v-align :center
          (box :width :fill :height (- heat-row-height 0.08)
            :corner-radius 1
            :background-color (offset-color (nth cells i) (nth measured i))))))))

;; Beat numbers under the map, one per quarter note, on the rows' columns.
(def heat-ruler (slots resolution)
  (let ((per-beat (max 1 (round (/ 1 resolution)))))
    (h-stack :key "groove-tab-heat-ruler" :width :fill :gap 0.06
      (box :width heat-label-width :height 0.6 :bg :transparent)
      (each (range 0 slots) |i|
        (box :key (str "groove-tab-heat-ruler-" i) :flex 1 :height 0.6 :padding 0
          :bg :transparent
          (label (if (= (mod i per-beat) 0) (str (+ 1 (floor (/ i per-beat)))) "")
            :height 0.6 :font-size 7
            :color :dim :bg :transparent))))))

(def selected-name ()
  (if (pool-key? selected-key)
    (let ((entry (pool-entry selected-key)))
      (if (= entry nil) "" (get entry :name)))
    (let ((hits (filter (lambda (entry) (= (get entry :key) selected-key))
                  (or SEQ.groove-library (list)))))
      (if (> (len hits) 0) (get (nth hits 0) :name) ""))))

(def selected-heat ()
  (if (pool-key? selected-key)
    (let ((entry (pool-entry selected-key)))
      (if (= entry nil) nil (get entry :heatmap)))
    library-heat))

(def preview ()
  (let ((heat (selected-heat)))
    (box :key "groove-tab-preview" :debug-name "groove-tab-preview"
      :width :fill :padding 0.35 :background-color :buffer-bg :corner-radius 8
      (if (= heat nil)
        (label "Select a groove to see its feel"
          :font-size 9 :color :dim :bg :transparent)
        (v-stack :width :fill :gap 0.15
          (h-stack :width :fill :gap 0.5 :align :center
            (label (selected-name) :font-size 10 :color :white :bg :transparent)
            (box :flex 1 :height 0.1 :bg :transparent)
            (label (get heat :grid) :key "groove-tab-grid"
              :font-size 8.5 :color :dim :bg :transparent))
          (let ((slots (max 1 (get heat :slots))))
            ;; offset-color's zero-offset base is close to :buffer-bg: on the
            ;; preview's own surface every filled (unmeasured) slot vanished.
            (box :key "groove-tab-heat-backdrop" :width :fill :padding 0.2 :corner-radius 6
              :background-color heat-backdrop
              (v-stack :key "groove-tab-heatmap" :debug-name "groove-tab-heatmap"
                :width :fill :gap 0
                (each (range 0 (len (get heat :rows))) |index|
                  (heat-row (nth (get heat :rows) index) index))
                (heat-ruler slots (get heat :resolution-beats)))))
          (label "amber = early, blue = late; dim cells were filled in"
            :font-size 7 :color :dim :bg :transparent))))))

;; ── Rename field ──
(def rename-panel ()
  (box :key "groove-rename-panel" :width :fill :padding 0.25
    (v-stack :width :fill :gap 0.4
      (text-input
        :key "groove-rename-name"
        :width :fill
        :value rename-draft
        :placeholder "groove name..."
        :auto-focus true
        :select-all-on-focus true
        :on-change (lambda (value) (set! rename-draft value))
        :on-submit (lambda () (commit-rename))
        :on-cancel (lambda () (cancel-rename))
        :height 1.5
        :font-size 12)
      (if (= (get rename-item :kind) "library")
        (label "Renames the file in your library; this cannot be undone."
          :font-size 8 :color :dim :bg :transparent)
        (box :width :fill :height 0))
      (h-stack :width :fill :gap 0.5 :align :center
        (button "Rename"
          :key "groove-rename-confirm"
          :variant :primary
          :flex 1 :height 1.2 :font-size 10
          :on-click |x y r| (commit-rename)
          :color :white)
        (button "Cancel"
          :key "groove-rename-cancel"
          :variant :ghost
          :flex 1 :height 1.2 :font-size 10
          :on-click |x y r| (cancel-rename)
          :color :gray)))))

;; ── Panel ──
(def panel (query)
  (let ((items (seq-groove-tree query
                 (or SEQ.groove-pool (list))
                 (or SEQ.groove-library (list)))))
    (v-stack :key "grooves-tab-panel" :width :fill :gap 0.5 :flex 1
      (box :width :fill :padding 0.25
        (h-stack :width :fill :gap 0.5 :align :center
          (label (if (>= (selected-rack-id) 0)
                   (str "Apply to: " (rack/group-name (rack/group-index-by-id (selected-rack-id))))
                   "Select a drum rack to apply grooves")
            :flex 1 :font-size 9 :color :dim :bg :transparent)
          (button (if expand-instances "Collapse" "Instances")
            :key "groove-expand-button"
            :variant :secondary
            :width 8
            :height 1.3
            :font-size 10.5
            :on-click |x y r| (set! expand-instances (not expand-instances))
            :color :white)))
      (if (= rename-item nil)
        (box :width :fill :height 0)
        (rename-panel))
      (box :width :fill :background-color :buffer-bg :corner-radius 8 :padding 0 :flex 1
        (scroll :key "grooves-tab-scroll" :width :fill :flex 1
          (tree
            :key "grooves-tab-tree"
            :debug-name "grooves-tab-tree"
            :width :fill
            :background-color :buffer-bg
            :items items
            :font-size 12
            ;; A narrow sidebar drops a row's "1 bar · 1/16" before
            ;; truncating the groove's name.
            :detail-yields true
            :selected-path selected-path
            :expand-all (or expand-instances (not (= query "")))
            :focusable true
            :on-select (lambda (item) (select-item item))
            :on-activate (lambda (item) (activate-item item))
            :on-right-click (lambda (event) (open-menu event)))))
      (preview))))
