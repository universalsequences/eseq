;; alez.jaki.kind — jaki without typing: the `jaki` instance kind
;; (docs/jaki-kind-spec.md).
;;
;; A jaki pattern is figures plus routed rows:
;;
;;   (jak "hello" :16
;;     . . -                  <- the figure strip:  [. . -] [+]
;;     -> 0 left              <- row 0:  [1 Kick ▾] [left] [+]
;;     -> 2 (trunc 3) right)  <- row 2:  [3 Hat ▾] [trunc 3] [right] [+]
;;
;; Each instance ("New jaki" on the package row, or "New jaki in rack" on a
;; rack) is the host's: it publishes the :generator under its own id, gets its
;; own `*jaki · <label>*` buffer and tab, and renders (jk-panel self) there.
;; Rack-owned, a row's route is a pad (rack member) instead of a track.
;;
;; STATE: the pattern is the instance's :document (per scene, saved,
;; undoable, read by the scheduler tick as `self`); nothing here is global.

(module alez.jaki.kind)

(import alez.jaki.doc)

(export jk-panel)

(def jk-max-rows 16)
(def jk-row-height 1.3)

;; ── small list helpers ─────────────────────────────────────────────────────

(def jk-update-nth (items index update)
  (map (lambda (i) (if (= i index) (update (nth items i)) (nth items i)))
       (range 0 (len items))))

(def jk-remove-nth (items index)
  (reduce (lambda (acc i) (if (= i index) acc (append acc (list (nth items i)))))
          (list) (range 0 (len items))))

(def jk-index-of (xs item)
  (reduce (lambda (acc i) (if (and (< acc 0) (= (nth xs i) item)) i acc))
          -1 (range 0 (len xs))))

;; ── document edits (each is one undoable scene-slot write) ─────────────────

(def jk-add-figure (self label)
  (let ((i (jk-index-of alez.jaki.doc/figure-option-labels label)))
    (if (>= i 0)
      (set! self.figures
        (append self.figures (list (nth (nth alez.jaki.doc/figure-options i) 1))))
      nil)))

(def jk-replace-figure (self index label)
  (let ((i (jk-index-of alez.jaki.doc/figure-option-labels label)))
    (if (>= i 0)
      (set! self.figures
        (jk-update-nth self.figures index
          (lambda (f) (nth (nth alez.jaki.doc/figure-options i) 1))))
      nil)))

(def jk-remove-figure (self index)
  (set! self.figures (jk-remove-nth self.figures index)))

;; Rows are stored only as far as the last one ever edited; the rest read the
;; default (off, no modifiers).
(def jk-edit-row (self index update)
  (let ((rows self.rows))
    (let ((padded (if (< index (len rows))
                    rows
                    (append rows (map (lambda (k) alez.jaki.doc/default-row)
                                      (range (len rows) (+ index 1)))))))
      (set! self.rows (jk-update-nth padded index update)))))

(def jk-set-route (self index route)
  (jk-edit-row self index (lambda (row) (merge row :route route))))

(def jk-new-mod (op)
  (dict :op op :args (alez.jaki.doc/mod-default-args op)))

(def jk-add-mod (self index op)
  (jk-edit-row self index
    (lambda (row) (merge row :mods (append (get row :mods) (list (jk-new-mod op)))))))

(def jk-edit-mod (self index m update)
  (jk-edit-row self index
    (lambda (row) (merge row :mods (jk-update-nth (get row :mods) m update)))))

(def jk-remove-mod (self index m)
  (jk-edit-row self index
    (lambda (row) (merge row :mods (jk-remove-nth (get row :mods) m)))))

(def jk-set-row-count (self n)
  (set! self.row-count (max 1 (min jk-max-rows (round n)))))

;; ── routes ─────────────────────────────────────────────────────────────────

;; Owned by a rack: routes are its members (pads), in member order. `self.owner`
;; is `:project` or the rack's group id; SEQ.groups is read so a member that
;; joins later shows up.
(def jk-route-tracks (self)
  (let ((owner self.owner) (groups SEQ.groups))
    (if (number? owner)
      (let ((gidx (eseq.drum-rack-v2/group-index-by-id owner)))
        (if (>= gidx 0) (eseq.drum-rack-v2/members gidx) (list)))
      (range 0 (len SEQ.track-names)))))

;; Option 0 is Off; option k+1 is route k (a track, or pad k on a rack).
(def jk-route-options (self)
  (let ((names SEQ.track-names))
    (cons "Off"
      (map (lambda (track) (str (+ track 1) " " (nth names track)))
           (jk-route-tracks self)))))

(def jk-route-index (self row)
  (let ((route (get row :route)))
    (if (and (number? route) (>= route 0) (< route (len (jk-route-tracks self))))
      (+ route 1)
      0)))

(def jk-route-color (self row)
  (let ((route (get row :route)) (tracks (jk-route-tracks self)))
    (if (and (number? route) (>= route 0) (< route (len tracks)))
      (let ((c (nth SEQ.track-colors (nth tracks route))))
        (if (and c (>= (len c) 3)) (rgba (nth c 0) (nth c 1) (nth c 2) 1) :mixer-strip-border))
      :mixer-strip-border)))

;; ── widgets ────────────────────────────────────────────────────────────────

(def jk-add-menu (key text labels placeholder width on-pick)
  (menu-button
    :key key
    :icon text
    :options labels
    :filterable true
    :filter-placeholder placeholder
    :width width :height jk-row-height :font-size 9 :menu-min-width 14
    :corner-radius 16
    :font-size 12
    :bg-color (rgba 0.28 0.20 0.11 1)
    :text-color :process-lane-accent
    :menu-bg :dropdown-menu-bg
    :menu-border-color :dropdown-menu-border
    :hover-bg :dropdown-hover-bg
    :on-change on-pick))

(def jk-x-button (key on-click)
  (button "×" :key key
    :width 1.2 :height 1.0 :padding 0 :font-size 9
    :variant :ghost :background-color :transparent :border-color :none :color :dim
    :on-click (lambda (event) (on-click))))

;; One figure box: a dropdown to swap the figure for another, and ×.
(def jk-figure-box (self index figure)
  (box :key (str "jaki-figure-" index)
    :width 9.4 :height jk-row-height :padding 0.1
    :background-color :bg  :corner-radius 16
    (h-stack :gap 0.1 :align :center
      (dropdown
        :key (str "jaki-figure-pick-" index)
        :value-index (max 0 (jk-index-of alez.jaki.doc/figure-option-labels
                               (alez.jaki.doc/figure-label figure)))
        :options alez.jaki.doc/figure-option-labels
        :filterable true
        :badge-color :transparent :bg-color :bg :border-color :transparent
        :width 7.8 :height 1.0 :font-size 13
        :on-change (lambda (label) (jk-replace-figure self index label)))
      (jk-x-button (str "jaki-figure-remove-" index)
        (lambda () (jk-remove-figure self index))))))

(def jk-figure-strip (self)
  (let ((figures self.figures))
    (h-stack :gap 0.4 :align :center
      (label "figures" :width 5 :height jk-row-height :font-size 9 :color :dim :bg :transparent)
      (each (range 0 (len figures)) |i| (jk-figure-box self i (nth figures i)))
      (jk-add-menu "jaki-figure-add" "+  figure" alez.jaki.doc/figure-option-labels
        "Filter figures…" 7
        (lambda (label) (jk-add-figure self label))))))

(def jk-num (key value spec on-change)
  (number-picker
    :key key
    :border-color :dim :background-color :mixer-strip-bg
    :value value :min (nth spec 0) :max (nth spec 1) :step (nth spec 2) :decimals (nth spec 3)
    :width 4 :height 1.0 :font-size 9
    :on-change on-change))

;; One modifier box: `[left ×]`, `[trunc (3) ×]`, `[every (4) (rev ▾) ×]`,
;; `[split (last ▾) ×]`. The number (if any) is args[0], the choice the last.
(def jk-mod-box (self index m mod)
  (let ((op (get mod :op))
        (args (get mod :args))
        (spec (alez.jaki.doc/mod-arg-spec (get mod :op)))
        (choices (alez.jaki.doc/mod-choices (get mod :op)))
        (key (str "jaki-mod-" index "-" m)))
    (let ((label-width (max 1.6 (* 0.6 (len op)))))
      (box :key key
        :width (+ label-width 2.2 (if spec 4.2 0) (if choices 5.6 0))
        :height jk-row-height :padding 0.15
        :background-color :bg  :corner-radius 16
        (h-stack :gap 0.2 :align :center
          (label op :width label-width :height 1.0 :font-size 9
            :h-align :center :color :foreground :bg :transparent)
          (if spec
            (jk-num (str key "-n") (nth args 0) spec
              (lambda (v)
                (jk-edit-mod self index m
                  (lambda (x) (merge x :args (cons v (rest (get x :args))))))))
            nil)
          (if choices
            (dropdown
              :key (str key "-word")
              :value-index (max 0 (jk-index-of choices (nth args (- (len args) 1))))
              :options choices
              :badge-color :transparent :bg-color :mixer-strip-bg
              :border-color :mixer-strip-selected-bg
              :width 5.4 :height 1.0 :font-size 9
              :on-change (lambda (word)
                           (jk-edit-mod self index m
                             (lambda (x)
                               (let ((xs (get x :args)))
                                 (merge x :args
                                   (append (jk-remove-nth xs (- (len xs) 1)) (list word))))))))
            nil)
          (jk-x-button (str key "-x") (lambda () (jk-remove-mod self index m))))))))

(def jk-row (self index row)
  (let ((mods (get row :mods))
        (live (alez.jaki.doc/row-live? row)))
    (box :key (str "jaki-row-" index)
      :height jk-row-height :padding 0 :background-color :transparent
      (h-stack :gap 0.4 :align :center
        (box :width 0.3 :height jk-row-height :corner-radius 2
          :background-color (jk-route-color self row))
        (label (str index) :width 1.4 :height jk-row-height :font-size 9
          :h-align :center :color (if live :foreground :dim) :bg :transparent)
        (dropdown
          :key (str "jaki-route-" index)
          :value-index (jk-route-index self row)
          :options (jk-route-options self)
          :badge-color :transparent :bg-color :mixer-strip-bg
          :border-color :mixer-strip-selected-bg
          :width 8 :height jk-row-height :font-size 9
          :on-change (lambda (label)
                       (jk-set-route self index
                         (- (max 0 (jk-index-of (jk-route-options self) label)) 1))))
        (each (range 0 (len mods)) |m| (jk-mod-box self index m (nth mods m)))
        (jk-add-menu (str "jaki-mod-add-" index) "+" alez.jaki.doc/mod-labels
          "Filter modifiers…" 2.4
          (lambda (op) (jk-add-mod self index op)))))))

(def jk-code (self figures rows row-count)
  (let ((b (alez.jaki.doc/body figures rows row-count)))
    (if b
      (source (append (list 'jak self.label :16) b))
      "route a row to hear it")))

(def jk-panel (self)
  (let ((figures self.figures)
        (rows self.rows)
        (row-count self.row-count))
    (box :padding 0.85
      (box
        :width 96 :padding 1
        :background-color :mixer-strip-bg :border-color :mixer-strip-border :corner-radius 16
        (v-stack :gap 0.6
          (h-stack :gap 0.6 :align :center
            (label "jaki" :width 4 :height jk-row-height :font-size 11 :color :foreground :bg :transparent)
            (label "rows" :width 3.5 :height jk-row-height :font-size 9 :h-align :right
              :color :dim :bg :transparent)
            (number-picker
              :key "jaki-row-count"
              :border-color :dim :background-color :mixer-strip-bg
              :value row-count :min 1 :max jk-max-rows :step 1 :decimals 0
              :width 4 :height jk-row-height :font-size 9
              :on-change (lambda (v) (jk-set-row-count self v))))
          (jk-figure-strip self)
          (v-stack :gap 0.25
            (each (range 0 row-count) |i| (jk-row self i (alez.jaki.doc/row-at rows i))))
          (box :width :fill :padding 0.4 :background-color :bg :corner-radius 6
            (label (jk-code self figures rows row-count)
              :width 92 :height 1.0 :font-size 8 :color :dim :bg :transparent)))))))

;; The kind (docs/instance-kinds-spec.md, docs/jaki-kind-spec.md). Each
;; instance publishes the :generator under its own id; its tick reads the
;; instance's document through `self`.
(def-kind jaki
  :generator (:resolution :16
              :requires (alez.jaki.doc)
              :tick (alez.jaki.doc/tick self.figures self.rows self.row-count))
  :document ((figures (list (list :dot :dot :dot :dot)))
             (rows (list (dict :route 0 :mods (list))))
             (row-count 8))
  :view jk-panel)
