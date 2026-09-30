;; alez.jaki.kind — jaki without typing: the `jaki` instance kind
;; (docs/jaki-kind-spec.md).
;;
;; A jaki pattern is figures plus routed rows:
;;
;;   (jak "hello" :16
;;     . . -                  <- the figure strip:  [. . -] [+]
;;     -> 0 left              <- row 0:  [1 Kick ▾]  left  +
;;     -> 2 (trunc 3) right)  <- row 2:  [3 Hat ▾]  (trunc 3)  right  +
;;
;; A row's modifiers are one sexp-slot (docs/sexp-slot-spec.md): type
;; `(every 2 (rev swap))` on its `+`, scrub numbers, pick words; any argument
;; may be a list, one per cycle.
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
          (lambda (f) (alez.jaki.doc/with-times
                        (nth (nth alez.jaki.doc/figure-options i) 1)
                        (alez.jaki.doc/figure-times f)))))
      nil)))

;; How many times the figure plays back to back (×1 … ×16).
(def jk-set-figure-times (self index n)
  (set! self.figures
    (jk-update-nth self.figures index
      (lambda (f) (alez.jaki.doc/with-times f (max 1 (min 16 n)))))))

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

(def jk-set-mods (self index mods)
  (jk-edit-row self index (lambda (row) (merge row :mods mods))))

;; How the pattern is clocked (docs/jaki-trig-modes-spec.md): loop on the
;; transport, or wait for a sequencer (a neuron routed here) to gate it.
(def jk-set-mode (self label)
  (let ((i (jk-index-of alez.jaki.doc/mode-labels label)))
    (if (>= i 0) (set! self.mode (nth alez.jaki.doc/modes i)) nil)))

(def jk-mode-index (self)
  (max 0 (jk-index-of alez.jaki.doc/modes (alez.jaki.doc/mode-of self.mode))))

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

(def jk-times-labels (map (lambda (n) (list n (str "×" n))) (range 1 17)))

;; One figure box: a dropdown to swap the figure for another, its repeat
;; count, and ×.
(def jk-figure-box (self index figure)
  (box :key (str "jaki-figure-" index)
    :width 13.6 :height jk-row-height :padding 0.1
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
      (number-picker
        :key (str "jaki-figure-times-" index)
        :value (alez.jaki.doc/figure-times figure) :min 1 :max 16 :step 1 :decimals 0
        :value-labels jk-times-labels
        :border-color :transparent :background-color :bg
        :width 3.8 :height 1.0 :font-size 11
        :on-change (lambda (v) (jk-set-figure-times self index v)))
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

;; ── hit strip with playhead ───────────────────────────────────────────────
;; Two cycles of the figure strip's pattern as its symbols — one cell per
;; dot, one double-width cell per dash — figures and cycles set apart. While
;; the transport plays, the symbol under the playhead lights up: the tick
;; stamps (gen-mark (+ tick 1)) at its audio time (alez.jaki.doc/tick), the
;; host publishes the latest sounded one as SEQ.generator-mark-<id> (0 when
;; stopped), and the strip locates that tick in the pattern. The strip is its
;; own subtree so the playhead re-runs only it.

(def jk-preview-cycles 2)
(def jk-preview-width 80)

(def jk-last (xs) (nth xs (- (len xs) 1)))

;; hits → symbol cells (dict :e first-hit :units 1|2 :gap spacer-before); a
;; dash's second hit folds into its cell
(def jk-symbol-cells (hits)
  (reduce
    (lambda (acc e)
      (if (= (get e :hit) 2)
        acc
        (let ((prev (if (empty? acc) nil (get (jk-last acc) :e))))
          (append acc
            (list (dict :e e
                        :units (if (= (get e :sym) :dash) 2 1)
                        :gap (if (= prev nil)
                               0
                               (if (not (= (get prev :cycle) (get e :cycle)))
                                 1.2
                                 (if (not (= (get prev :fig) (get e :fig))) 0.4 0)))))))))
    (list) hits))

;; the playing (cycle unit) of pattern p, or nil when stopped
(def jk-playhead (self p)
  (let ((mark (reactive-value (bind-seq (str "generator-mark-" self.id)))))
    (if (and (number? mark) (> mark 0))
      (let ((tick (- mark 1)))
        (let ((loc (alez.jaki.core/locate p tick)))
          (list (nth loc 0) (- tick (nth loc 1)))))
      nil)))

(def jk-cell-playing? (cell ph)
  (if (= ph nil)
    false
    (let ((e (get cell :e)))
      (let ((u0 (/ (nth (get e :off) 0) (nth (get e :off) 1))) (u (nth ph 1)))
        (and (= (get e :cycle) (nth ph 0))
             (<= u0 u) (< u (+ u0 (get cell :units))))))))

(def jk-symbol-cell (key cell w playing?)
  (let ((width (+ (* w (get cell :units)) (* 0.08 (- (get cell :units) 1)))))
    (box :key key :width width :height 1.0 :padding 0 :corner-radius 6
      :background-color (if playing? :process-lane-accent :mixer-strip-bg)
      (label (if (= (get (get cell :e) :sym) :dash) "-" ".")
        :width width :height 1.0 :font-size 11 :h-align :center
        :color (if playing? :bg :dim) :bg :transparent))))

(def jk-preview-strip (self figures)
  (let ((body (alez.jaki.doc/figures-body figures)))
    (let ((ph (jk-playhead self (alez.jaki.core/from-list body))))
      (let ((base (if (= ph nil) 0 (- (nth ph 0) (mod (nth ph 0) jk-preview-cycles)))))
        (let ((cells (jk-symbol-cells (alez.jaki.core/preview body base jk-preview-cycles))))
          (let ((units (reduce (lambda (acc c) (+ acc (get c :units))) 0 cells))
                (gaps (reduce (lambda (acc c) (+ acc (get c :gap))) 0 cells)))
            (let ((w (min 1.1 (/ (- jk-preview-width gaps) (max 1 units)))))
              (h-stack :gap 0.08 :align :center
                (each (range 0 (len cells)) |i|
                  (let ((c (nth cells i)))
                    (h-stack :gap 0 :align :center
                      (box :width (get c :gap) :height 1.0 :padding 0 :background-color :transparent)
                      (jk-symbol-cell (str "jaki-hit-" i) c w (jk-cell-playing? c ph)))))))))))))

(def jk-preview (self figures)
  (h-stack :gap 0.4 :align :center
    (label "hits" :width 5 :height jk-row-height :font-size 9 :color :dim :bg :transparent)
    (if (empty? figures)
      (box :width 1 :height 1.0 :background-color :transparent)
      (subtree :key (str "jaki-hits-" self.id)
        (jk-preview-strip self figures)))))

;; Row i's route in the generator: its place among the live rows before it
;; (alez.jaki.doc/body leaves Off rows out).
(def jk-route-slot (rows i)
  (reduce (lambda (n j) (if (alez.jaki.doc/row-live? (alez.jaki.doc/row-at rows j)) (+ n 1) n))
          0 (range 0 i)))

(def jk-row (self index row route-slot)
  (let ((live (alez.jaki.doc/row-live? row)))
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
        (sexp-slot
          :key (str "jaki-mods-" index)
          :schema alez.jaki.doc/row-schema
          :value (alez.jaki.doc/row-mods row)
          :height jk-row-height :font-size 9
          ;; one line always: the row is one line tall, a wrapped second
          ;; line would draw over the next row
          :wrap false
          :head-color :process-lane-accent
          ;; an (on SEL w) item's selector reads apart from its word
          :tint-args '(("on" 0))
          ;; the items applied to the sounding hit — (on …) and (every …) —
          ;; ring up: the tick stamps a bitmask per route at each hit's audio
          ;; time (jaki-sequencer-spec §7.3); a render binding, no Lisp rerun
          :lit (if live (bind-seq (str "generator-mark-" self.id "-" route-slot)) 0)
          ;; and the member of each (seq …) / cycle list the hit played (§7.4)
          :lit-values (if live
                        (map (lambda (k) (bind-seq (str "generator-mark-" self.id "-" route-slot "." k)))
                             (range 0 (len (get row :mods))))
                        (list))
          :lit-color :process-lane-accent
          :on-change (lambda (mods) (jk-set-mods self index mods)))))))

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
         :padding 1
;        :background-color :mixer-strip-bg :border-color :mixer-strip-border :corner-radius 16
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
              :on-change (lambda (v) (jk-set-row-count self v)))
            (label "mode" :width 3.5 :height jk-row-height :font-size 9 :h-align :right
              :color :dim :bg :transparent)
            (dropdown
              :key "jaki-mode"
              :value-index (jk-mode-index self)
              :options alez.jaki.doc/mode-labels
              :badge-color :transparent :bg-color :mixer-strip-bg
              :border-color :mixer-strip-selected-bg
              :width 7 :height jk-row-height :font-size 9
              :on-change (lambda (label) (jk-set-mode self label))))
          (jk-figure-strip self)
          (jk-preview self figures)
          (v-stack :gap 0.25
            (each (range 0 row-count) |i|
              (jk-row self i (alez.jaki.doc/row-at rows i) (jk-route-slot rows i))))
          (box :width :fill :padding 0.4 :background-color :bg :corner-radius 6
            (label (jk-code self figures rows row-count)
              :width 92 :height 1.0 :font-size 8 :color :dim :bg :transparent)))))))

;; The kind (docs/instance-kinds-spec.md, docs/jaki-kind-spec.md). Each
;; instance publishes the :generator under its own id; its tick reads the
;; instance's document through `self`.
(def-kind jaki
  :generator (:resolution :16
              :requires (alez.jaki.doc)
              :tick (alez.jaki.doc/tick self.figures self.rows self.row-count self.mode))
  :document ((figures (list (list :dot :dot :dot :dot)))
             (rows (list (dict :route 0 :mods (list))))
             (row-count 8)
             (mode :loop))
  :view jk-panel)
