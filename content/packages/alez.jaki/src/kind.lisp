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
;; An instance holds any number of patterns, each its own figure strip and
;; rows — one jaki voice each, cycling on its own beside the others (kick and
;; snare on `. . -`, the hat on `. -`). `+ pattern` adds one.
;;
;; A row's modifiers are one sexp-slot (docs/sexp-slot-spec.md): type
;; `(every 2 (rev swap))` on its `+`, scrub numbers, pick words; any argument
;; may be a list, one per cycle.
;;
;; A row's `rules` toggle, before its words, opens a box of its rules under it
;; (docs/jaki-row-processes-spec.md §9, §15), one line each: TRIGGER ->
;; STAGES… ACTIONS…, both sexp-slots; `+ rule` adds one, and the row's seed
;; (rarely touched: it rerolls the coins) sits at the right of that line.
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
;; Pattern 0 is the document's own figures/rows/row-count; pattern k > 0 is
;; entry k - 1 of self.patterns (alez.jaki.doc, the document).

(def jk-patterns (self)
  (alez.jaki.doc/pattern-list self.figures self.rows self.row-count self.patterns))

(def jk-put (self k field value)
  (if (= k 0)
    (if (= field :figures)
      (set! self.figures value)
      (if (= field :rows) (set! self.rows value) (set! self.row-count value)))
    (set! self.patterns
      (jk-update-nth self.patterns (- k 1) (lambda (p) (merge p field value))))))

(def jk-field (self k field) (get (nth (jk-patterns self) k) field))

(def jk-add-figure (self k label)
  (let ((i (jk-index-of alez.jaki.doc/figure-option-labels label)))
    (if (>= i 0)
      (jk-put self k :figures
        (append (jk-field self k :figures)
                (list (nth (nth alez.jaki.doc/figure-options i) 1))))
      nil)))

(def jk-replace-figure (self k index label)
  (let ((i (jk-index-of alez.jaki.doc/figure-option-labels label)))
    (if (>= i 0)
      (jk-put self k :figures
        (jk-update-nth (jk-field self k :figures) index
          (lambda (f) (alez.jaki.doc/with-times
                        (nth (nth alez.jaki.doc/figure-options i) 1)
                        (alez.jaki.doc/figure-times f)))))
      nil)))

;; How many times the figure plays back to back (×1 … ×16).
(def jk-set-figure-times (self k index n)
  (jk-put self k :figures
    (jk-update-nth (jk-field self k :figures) index
      (lambda (f) (alez.jaki.doc/with-times f (max 1 (min 16 n)))))))

(def jk-remove-figure (self k index)
  (jk-put self k :figures (jk-remove-nth (jk-field self k :figures) index)))

;; Rows are stored only as far as the last one ever edited; the rest read the
;; default (off, no modifiers).
(def jk-edit-row (self k index update)
  (let ((rows (jk-field self k :rows)))
    (let ((padded (if (< index (len rows))
                    rows
                    (append rows (map (lambda (j) alez.jaki.doc/default-row)
                                      (range (len rows) (+ index 1)))))))
      (jk-put self k :rows (jk-update-nth padded index update)))))

(def jk-set-route (self k index route)
  (jk-edit-row self k index (lambda (row) (merge row :route route))))

(def jk-set-mods (self k index mods)
  (jk-edit-row self k index (lambda (row) (merge row :mods mods))))

;; ── row rules (docs/jaki-row-processes-spec.md §9; stored as :procs) ───────

(def jk-set-procs (self k index procs)
  (jk-edit-row self k index (lambda (row) (merge row :procs procs))))

(def jk-set-chain (self k index c chain)
  (jk-set-procs self k index
    (jk-update-nth (alez.jaki.doc/row-procs (alez.jaki.doc/row-at (jk-field self k :rows) index))
                   c (lambda (old) chain))))

;; A row without :seed plays its route index as the seed (alez.jaki.core
;; seed-route); the picker shows that, and any edit pins the number.
(def jk-set-seed (self k index v)
  (jk-edit-row self k index (lambda (row) (merge row :seed (max 0 (round v))))))

;; which row's rules box is open: "k:index", or "" (a view cell)
(def jk-rules-key (k index) (str k ":" index))
(def jk-rules-open? (self k index) (= self.rules-open (jk-rules-key k index)))
(def jk-toggle-rules (self k index)
  (set! self.rules-open (if (jk-rules-open? self k index) "" (jk-rules-key k index))))

(def jk-add-rule (self k index)
  (jk-set-procs self k index
    (append (alez.jaki.doc/row-procs (alez.jaki.doc/row-at (jk-field self k :rows) index))
            (list alez.jaki.doc/new-chain))))

(def jk-add-pattern (self)
  (set! self.patterns (append self.patterns (list alez.jaki.doc/new-pattern))))

;; Removing the first pattern moves the second into its place.
(def jk-remove-pattern (self k)
  (if (> k 0)
    (set! self.patterns (jk-remove-nth self.patterns (- k 1)))
    (if (empty? self.patterns)
      nil
      (let ((next (first self.patterns)))
        (do
          (set! self.figures (get next :figures))
          (set! self.rows (get next :rows))
          (set! self.row-count (get next :row-count))
          (set! self.patterns (rest self.patterns)))))))

;; How the pattern is clocked (docs/jaki-trig-modes-spec.md): loop on the
;; transport, or wait for a sequencer (a neuron routed here) to gate it.
(def jk-set-mode (self label)
  (let ((i (jk-index-of alez.jaki.doc/mode-labels label)))
    (if (>= i 0) (set! self.mode (nth alez.jaki.doc/modes i)) nil)))

(def jk-mode-index (self)
  (max 0 (jk-index-of alez.jaki.doc/modes (alez.jaki.doc/mode-of self.mode))))

(def jk-set-row-count (self k n)
  (jk-put self k :row-count (max 1 (min jk-max-rows (round n)))))

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

;; The track a row plays (a rack's pad → its member track), or nil when the
;; row is Off: the sexp-slot's :dyn-context, so `(plock NAME V)` completes
;; and checks NAME against that track's parameters (jaki-plock-spec §5.3).
(def jk-row-track (self row)
  (let ((route (get row :route)) (tracks (jk-route-tracks self)))
    (if (and (number? route) (>= route 0) (< route (len tracks)))
      (nth tracks route)
      nil)))

;; ── plock names after a reroute (jaki-plock-spec §6) ──────────────────────
;; The slot draws a name its track does not have in the error color and keeps
;; it; hovering the item says why. One hovered row at a time, so one global:
;; (list instance-id pattern row text), or nil.
(defstate jk-hover-reason nil)

;; the NAME of a (plock NAME V) item, bare or under (on SEL …); nil otherwise
(def jk-plock-name (item)
  ;; a slot item is a word string, a number, or a list
  (if (and item (not (string? item)) (not (number? item)) (not (empty? item)))
    (if (and (= (first item) "plock") (> (len item) 1) (string? (nth item 1)))
      (nth item 1)
      (if (and (= (first item) "on") (> (len item) 2))
        (jk-plock-name (nth item 2))
        nil))
    nil))

(def jk-plock-reason (item track)
  (let ((name (jk-plock-name item)))
    (if (and (string? name) (number? track)
             (= (dyn-word-valid? "param" track name) false))
      (str "not on " (nth SEQ.track-names track))
      nil)))

(def jk-hover-row (self k index item track)
  (let ((text (jk-plock-reason item track)))
    (if text
      (set! jk-hover-reason (list self.id k index text))
      (if (and jk-hover-reason
               (= (nth jk-hover-reason 0) self.id)
               (= (nth jk-hover-reason 1) k)
               (= (nth jk-hover-reason 2) index))
        (set! jk-hover-reason nil)
        nil))))

(def jk-row-reason (self k index)
  (if (and jk-hover-reason
           (= (nth jk-hover-reason 0) self.id)
           (= (nth jk-hover-reason 1) k)
           (= (nth jk-hover-reason 2) index))
    (nth jk-hover-reason 3)
    nil))

;; ── widgets ────────────────────────────────────────────────────────────────

;; a widget key in pattern k (pattern 0 keeps the plain key)
(def jk-key (k name) (if (= k 0) name (str name "-p" k)))

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
(def jk-figure-box (self k index figure)
  (box :key (jk-key k (str "jaki-figure-" index))
    :width 13.6 :height jk-row-height :padding 0.1
    :background-color :bg  :corner-radius 16
    (h-stack :gap 0.1 :align :center
      (dropdown
        :key (jk-key k (str "jaki-figure-pick-" index))
        :value-index (max 0 (jk-index-of alez.jaki.doc/figure-option-labels
                               (alez.jaki.doc/figure-label figure)))
        :options alez.jaki.doc/figure-option-labels
        :filterable true
        :badge-color :transparent :bg-color :bg :border-color :transparent
        :width 7.8 :height 1.0 :font-size 13
        :on-change (lambda (label) (jk-replace-figure self k index label)))
      (number-picker
        :key (jk-key k (str "jaki-figure-times-" index))
        :value (alez.jaki.doc/figure-times figure) :min 1 :max 16 :step 1 :decimals 0
        :value-labels jk-times-labels
        :border-color :transparent :background-color :bg
        :width 3.8 :height 1.0 :font-size 11
        :on-change (lambda (v) (jk-set-figure-times self k index v)))
      (jk-x-button (jk-key k (str "jaki-figure-remove-" index))
        (lambda () (jk-remove-figure self k index))))))

(def jk-figure-strip (self k figures)
  (h-stack :gap 0.4 :align :center
    (label "figures" :width 5 :height jk-row-height :font-size 9 :color :dim :bg :transparent)
    (each (range 0 (len figures)) |i| (jk-figure-box self k i (nth figures i)))
    (jk-add-menu (jk-key k "jaki-figure-add") "+  figure" alez.jaki.doc/figure-option-labels
      "Filter figures…" 7
      (lambda (label) (jk-add-figure self k label)))))

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

(def jk-preview-strip (self k figures)
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
                      (jk-symbol-cell (jk-key k (str "jaki-hit-" i)) c w (jk-cell-playing? c ph)))))))))))))

(def jk-preview (self k figures)
  (h-stack :gap 0.4 :align :center
    (label "hits" :width 5 :height jk-row-height :font-size 9 :color :dim :bg :transparent)
    (if (empty? figures)
      (box :width 1 :height 1.0 :background-color :transparent)
      (subtree :key (jk-key k (str "jaki-hits-" self.id))
        (jk-preview-strip self k figures)))))

;; Row i's route in the generator: its place among the live rows before it
;; (alez.jaki.doc/body leaves Off rows out), after every live row of the
;; sounding patterns before it (alez.jaki.doc/patterns-body).
(def jk-live-rows-before (rows i)
  (reduce (lambda (n j) (if (alez.jaki.doc/row-live? (alez.jaki.doc/row-at rows j)) (+ n 1) n))
          0 (range 0 i)))

(def jk-route-base (pats k)
  (reduce (lambda (n j)
            (let ((pat (nth pats j)))
              (if (alez.jaki.doc/pattern-sounds? pat)
                (+ n (jk-live-rows-before (get pat :rows) (get pat :row-count)))
                n)))
          0 (range 0 k)))

;; `sounding`: the row is routed and its pattern plays, so its route has marks
(def jk-row (self k index row route-slot sounding)
  (let ((live (alez.jaki.doc/row-live? row))
        (track (jk-row-track self row))
        (reason (jk-row-reason self k index)))
    (box :key (jk-key k (str "jaki-row-" index))
      :height jk-row-height :padding 0 :background-color :transparent
      (h-stack :gap 0.4 :align :center
        (box :width 0.3 :height jk-row-height :corner-radius 2
          :background-color (jk-route-color self row))
        (label (str index) :width 1.4 :height jk-row-height :font-size 9
          :h-align :center :color (if live :foreground :dim) :bg :transparent)
        (dropdown
          :key (jk-key k (str "jaki-route-" index))
          :value-index (jk-route-index self row)
          :options (jk-route-options self)
          :badge-color :transparent :bg-color :mixer-strip-bg
          :border-color :mixer-strip-selected-bg
          :width 8 :height jk-row-height :font-size 9
          :on-change (lambda (label)
            (jk-set-route self k index
              (- (max 0 (jk-index-of (jk-route-options self) label)) 1))))
        (jk-rules-toggle self k index row)
        (sexp-slot
          :key (jk-key k (str "jaki-mods-" index))
          :schema alez.jaki.doc/row-schema
          :value (alez.jaki.doc/row-mods row)
          :height jk-row-height :font-size 9
          ;; one line always: the row is one line tall, a wrapped second
          ;; line would draw over the next row
          :wrap false
          :head-color :process-lane-accent
          ;; an (on SEL w) item's selector reads apart from its word
          :tint-args '(("on" 0))
          :font-size 11
          ;; the items applied to the sounding hit — (on …) and (every …) —
          ;; ring up: the tick stamps a bitmask per route at each hit's audio
          ;; time (jaki-sequencer-spec §7.3); a render binding, no Lisp rerun
          :lit (if sounding (bind-seq (str "generator-mark-" self.id "-" route-slot)) 0)
          ;; and the member of each (seq …) / cycle list the hit played (§7.4)
          :lit-values (if sounding
            (map (lambda (k) (bind-seq (str "generator-mark-" self.id "-" route-slot "." k)))
              (range 0 (len (get row :mods))))
            (list))
          :lit-color :process-lane-accent
          ;; `(dyn "param")` names complete and validate against this track
          :dyn-context track
          :on-hover (lambda (item) (jk-hover-row self k index item track))
          :on-change (lambda (mods) (jk-set-mods self k index mods)))
        (if reason
          (label reason :width 12 :height jk-row-height :font-size 9
            :color :error :bg :transparent)
          (box :width 0 :height 0 :background-color :transparent))))))

;; ── row rules: a box of lines under the row ───────────────────────────────

;; where the row's words start (color bar, index, route dropdown, gaps), so
;; the rules box lines up under them
(def jk-rule-indent 10.9)

(def jk-rules-toggle (self k index row)
  (let ((n (len (alez.jaki.doc/row-procs row))) (open (jk-rules-open? self k index)))
    (button (if (> n 0) (str "rules " n) "rules")
      :key (jk-key k (str "jaki-rules-toggle-" index))
      :width 4.8 :height jk-row-height :padding 0.1 :font-size 9 :corner-radius 16
      :background-color (if open :process-lane-accent :transparent)
      :border-color (if (> n 0) :process-lane-accent :mixer-strip-border)
      :color (if open :bg (if (> n 0) :process-lane-accent :dim))
      :on-click (lambda (event) (jk-toggle-rules self k index)))))

(def jk-add-rule-button (self k index)
  (button "+  rule" :key (jk-key k (str "jaki-rule-add-" index))
    :width 6 :height jk-row-height :padding 0 :font-size 11 :corner-radius 16
    :background-color (rgba 0.28 0.20 0.11 1) :border-color :none
    :color :process-lane-accent
    :on-click (lambda (event) (jk-add-rule self k index))))

(def jk-code (text)
  (label text :height jk-row-height :font-size 11 :v-align :center :color :dim :bg :transparent))

(def jk-rule-line (self k index procs c)
  (let ((chain (nth procs c)))
    (h-stack :gap 0.3 :align :center
      (sexp-slot
        :key (jk-key k (str "jaki-rule-trigger-" index "-" c))
        :schema alez.jaki.doc/proc-trigger-schema
        :value (alez.jaki.doc/chain-trigger-items chain)
        :height jk-row-height :font-size 11 :wrap false
        :head-color :process-lane-accent
        :on-change (lambda (items)
          (jk-set-chain self k index c (alez.jaki.doc/chain-with-trigger chain items))))
      (jk-code "->")
      (sexp-slot
        :key (jk-key k (str "jaki-rule-body-" index "-" c))
        :schema alez.jaki.doc/proc-body-schema
        :value (rest chain)
        :height jk-row-height :font-size 11 :wrap false
        :head-color :process-lane-accent
        :on-change (lambda (items)
          (jk-set-chain self k index c (cons (first chain) items))))
      (jk-x-button (jk-key k (str "jaki-rule-remove-" index "-" c))
        (lambda () (jk-set-procs self k index (jk-remove-nth procs c)))))))

(def jk-rules (self k index row route-slot)
  (let ((procs (alez.jaki.doc/row-procs row))
        (seed (let ((s (get row :seed))) (if (and (number? s) (>= s 0)) s route-slot))))
    (h-stack :gap 0 :align :top
      (box :width jk-rule-indent :height 1.0 :padding 0 :background-color :transparent)
      (box :key (jk-key k (str "jaki-rules-" index))
        :padding 0.5 :border-color :mixer-strip-border
        :background-color :mixer-strip-bg :corner-radius 12
      (v-stack :gap 0.15
        (each (range 0 (len procs)) |c| (jk-rule-line self k index procs c))
        (h-stack :gap 0.3 :align :center
          (jk-add-rule-button self k index)
          ;; seed is a rarely touched reroll: out of the way, at the right
          (box :width 40 :height 1.0 :padding 0 :background-color :transparent)
          (label "seed" :width 2.4 :height jk-row-height :font-size 9 :h-align :right
            :v-align :center :color :dim :bg :transparent)
          (number-picker
            :key (jk-key k (str "jaki-rule-seed-" index))
            :value seed :min 0 :max 999 :step 1 :decimals 0
            :border-color :transparent :background-color :transparent
            :width 3 :height jk-row-height :font-size 9
            :on-change (lambda (v) (jk-set-seed self k index v)))))))))

;; The pattern as the jak form it plays; several patterns are one voice line
;; each, a line apiece.
(def jk-code-lines (self pats)
  (let ((b (alez.jaki.doc/patterns-body pats)))
    (if (= b nil)
      (list "route a row to hear it")
      (if (> (len (filter alez.jaki.doc/pattern-sounds? pats)) 1)
        (let ((n (len b)))
          (cons (str "(jak " (source self.label) " :16")
                (map (lambda (i) (str "  " (source (nth b i)) (if (= i (- n 1)) ")" "")))
                     (range 0 n))))
        (list (source (append (list 'jak self.label :16) b)))))))

(def jk-pattern-header (self k pat count)
  (h-stack :gap 0.6 :align :center
    (label (str "pattern " (+ k 1)) :width 5.5 :height jk-row-height :font-size 10
      :color :foreground :bg :transparent)
    (label "rows" :width 3 :height jk-row-height :font-size 9 :h-align :right
      :color :dim :bg :transparent)
    (number-picker
      :key (jk-key k "jaki-row-count")
      :border-color :dim :background-color :mixer-strip-bg
      :value (get pat :row-count) :min 1 :max jk-max-rows :step 1 :decimals 0
      :width 4 :height jk-row-height :font-size 9
      :on-change (lambda (v) (jk-set-row-count self k v)))
    (if (> count 1)
      (jk-x-button (jk-key k "jaki-pattern-remove") (lambda () (jk-remove-pattern self k)))
      (box :width 1.2 :height 1.0 :background-color :transparent))))

(def jk-pattern (self pats k)
  (let ((pat (nth pats k)))
    (let ((figures (get pat :figures))
        (rows (get pat :rows))
        (sounds (alez.jaki.doc/pattern-sounds? pat))
        (base (jk-route-base pats k)))
      (box :padding 1 :corner-radius 16 :background-color :mixer-strip-bg 
        (v-stack :gap 0.6
          (jk-pattern-header self k pat (len pats))
          (jk-figure-strip self k figures)
          (jk-preview self k figures)
          (v-stack :gap 0.25
            (each (range 0 (get pat :row-count)) |i|
              (let ((row (alez.jaki.doc/row-at rows i))
                  (slot (+ base (jk-live-rows-before rows i))))
                (v-stack :gap 0.25
                  (jk-row self k i row slot (and sounds (alez.jaki.doc/row-live? row)))
                  (if (jk-rules-open? self k i)
                    (jk-rules self k i row slot)
                    nil)))))))))
  )

(def jk-panel (self)
  (let ((pats (jk-patterns self)))
    (box :padding 0.85
      (box
         :padding 1
        (v-stack :gap 0.6
          (h-stack :gap 0.6 :align :center
            (label "mode" :v-align :center :width 3.5 :height jk-row-height :font-size 9 :h-align :right
              :color :dim :bg :transparent)
            (dropdown
              :key "jaki-mode"
              :value-index (jk-mode-index self)
              :options alez.jaki.doc/mode-labels
              :badge-color :transparent :bg-color :mixer-strip-bg
              :border-color :mixer-strip-selected-bg
              :width 7 :height jk-row-height :font-size 9
              :on-change (lambda (label) (jk-set-mode self label))))
          (v-stack :gap 1.2
            (each (range 0 (len pats)) |k|
              (box :key (str "jaki-pattern-" k) :padding 0 :background-color :transparent
                (jk-pattern self pats k))))
          (button "+  pattern" :key "jaki-pattern-add"
            :width 7 :height jk-row-height :padding 0 :font-size 12 :corner-radius 16
            :background-color (rgba 0.28 0.20 0.11 1) :border-color :none
            :color :process-lane-accent
            :on-click (lambda (event) (jk-add-pattern self)))
          (box :width :fill :padding 0.4 :background-color :bg :corner-radius 6
            (v-stack :gap 0.1
              (each (jk-code-lines self pats) |line|
                (label line :width 92 :height 1.0 :font-size 8 :color :dim :bg :transparent)))))))))

;; The kind (docs/instance-kinds-spec.md, docs/jaki-kind-spec.md). Each
;; instance publishes the :generator under its own id; its tick reads the
;; instance's document through `self`.
(def-kind jaki
  :generator (:resolution :16
              :requires (alez.jaki.doc)
              :tick (alez.jaki.doc/tick-patterns self.figures self.rows self.row-count
                                                 self.mode self.patterns))
  :document ((figures (list (list :dot :dot :dash)))
             (rows (list (dict :route 0 :mods (list))))
             (row-count 8)
             (mode :loop)
             (patterns (list)))
  ;; view only: the row whose rules box is open ("k:index")
  :state ((rules-open ""))
  :view jk-panel)
