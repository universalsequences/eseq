;; ui/effects/pad-variants.lisp — The instrument panel's `pads` tab
;; (docs/rack-pad-variants-spec.md §6.1).
;;
;; Shown only for a drum-rack member. It draws the rack's pad grid: this
;; member's pads bright (its plain pad and its variants, each in its variant
;; colour), other members' pads dimmed. Clicking one of this member's pads
;; focuses it; with a VARIANT focused, the panel's knobs write that variant's
;; values instead of the patch (param-controls, `fx-set-instrument-value`).
;; Clicking an empty cell adds a variant of this member on that note.

(module eseq.effects.pad-variants)

(import eseq.kinds :refer (selection))
(import eseq.effects.state :refer (instrument-view))

(export pad-variant-view
        pads-tab-available?
        pads-tab-active?
        pads-authoring-pad
        pad-variant-value-of
        pad-variant-rgb
        pads-tab-panel)

;; The focused pad (by note, -999: none), the grid page (-99: the page of the
;; member's plain pad) and the name being typed (only while `editing`).
(def-kind pad-variant-view
  :key ()
  :state ((note -999)
          (page -99)
          (draft "")
          (editing false)))

;; Mirrors `crate::plock_variants::VARIANT_PALETTE`: the sequencer colours a
;; step by the same entry, so a step and its pad cell match.
(def pad-variant-palette
  (list (list 0.2706 0.7843 0.8627)
        (list 0.9098 0.6431 0.3098)
        (list 0.6627 0.4941 0.9098)
        (list 0.4353 0.8078 0.5412)
        (list 0.9098 0.4157 0.4157)
        (list 0.8510 0.7882 0.3529)))

(def pad-variant-rgb (variant)
  (nth pad-variant-palette (mod (- variant 1) (len pad-variant-palette))))

(def pads-tab-index 2)

;; The track the panel edits is a drum-rack member.
(def pads-tab-available? ()
  (let ((t selection.track))
    (if (and t t.pad) true false)))

(def pads-tab-active? ()
  (and (= instrument-view.tab pads-tab-index) (pads-tab-available?)))

;; The current track's pad on `note`, or nil.
(def member-pad-at (note)
  (let ((t selection.track))
    (if (and t t.pad)
      (first (filter (lambda (p) (and (= p.note note) (= p.track t))) t.pad.group.pads))
      nil)))

;; The variant pad the knobs write while the pads tab is open, or nil (no
;; focus, or the plain pad: that one edits the patch as usual).
(def pads-authoring-pad ()
  (if (pads-tab-active?)
    (let ((p (member-pad-at pad-variant-view.note)))
      (if (and p (> p.variant 0)) p nil))
    nil))

;; Variant pad p's value for the param named `name`, or nil.
(def pad-variant-value-of (p name)
  (let ((i (index-in p.value-names name)))
    (if (>= i 0) (nth p.value-values i) nil)))

(def index-in (xs x)
  (reduce (lambda (acc i) (if (and (< acc 0) (= (nth xs i) x)) i acc))
          -1 (range 0 (len xs))))

;; ── grid ──
;; The pad pages of eseq.drum-rack-v2, inlined: param-controls imports this
;; module, and must stay loadable without the rack modules' import chain.
;; Pages -3..3 (C1..D#8, the host's pad-note domain); a page starts at a C.

(def pad-clamp-pad-page (page) (max -3 (min 3 page)))

(def pad-clamp-note (note) (max -36 (min 51 note)))

(def pad-page-of-note (note) (pad-clamp-pad-page (floor (/ (pad-clamp-note note) 12))))

;; Cell 0 is the top-left: row 0 carries the page's highest four notes.
(def pad-cell-note (page cell)
  (+ (* 12 (pad-clamp-pad-page page))
     (+ (* 4 (- 3 (floor (/ cell 4)))) (mod cell 4))))

(def pad-note-names '("C" "C#" "D" "D#" "E" "F" "F#" "G" "G#" "A" "A#" "B"))

(def pad-note-label (note)
  (let ((n (pad-clamp-note note)))
    (str (nth pad-note-names (mod (+ (mod n 12) 12) 12)) (+ 4 (floor (/ n 12))))))

(def grid-page (plain)
  (if (= pad-variant-view.page -99)
    (pad-page-of-note plain.note)
    (pad-clamp-pad-page pad-variant-view.page)))

(def shift-page (plain delta)
  (set! pad-variant-view.page (pad-clamp-pad-page (+ (grid-page plain) delta))))

(def focus! (note)
  (do (set! pad-variant-view.note note)
      (set! pad-variant-view.editing false)))

(def add-variant! (t g note)
  (do (host-command "add-pad-variant"
        (dict :group-id g.gid :track-id t.tid :note note))
      (focus! note)))

(def pad-at (g note)
  (first (filter (lambda (p) (= p.note note)) g.pads)))

(def pad-cell-label (p)
  (if (> p.variant 0)
    (str (if (= p.name "") (str "v" p.variant) (substring p.name 0 5)) " " p.label)
    p.label))

(def pad-cell-width 3.55)
(def pad-cell-height 1.3)

(def pad-cell (t g note)
  (let ((p (pad-at g note))
        (focused (= note pad-variant-view.note)))
    (if (= p nil)
      ;; empty: a new variant of this member lands here
      (box :key (str "pad-variant-cell-" note)
        :width pad-cell-width :height pad-cell-height :corner-radius 4 :padding 0
        :background-color :mixer-control-bg :border-color :transparent
        :on-click |x y r| (add-variant! t g note)
        (label (str "+ " (pad-note-label note)) :font-size 7.5 :color :dimmer :bg :transparent
          :h-align :center :v-align :center :width :fill))
      (if (= p.track t)
        (box :key (str "pad-variant-cell-" note)
          :width pad-cell-width :height pad-cell-height :corner-radius 4 :padding 0
          :background-color (if (> p.variant 0) (pad-variant-rgb p.variant) :mixer-strip-selected-border)
          :border-color (if focused :fg :transparent)
          :on-click |x y r| (focus! note)
          ;; lights white while this pad sounds (bound: no re-render per hit)
          (label (pad-cell-label p) :font-size 8 :color :black :bg :transparent
            :active #'p.triggered :active-color :white
            :h-align :center :v-align :center :width :fill))
        ;; another member's pad: shown for orientation, not editable here
        (box :key (str "pad-variant-cell-" note)
          :width pad-cell-width :height pad-cell-height :corner-radius 4 :padding 0
          :background-color :fx-panel-header-bg :border-color :transparent
          (label (str (substring p.track.name 0 4) " " p.label) :font-size 7 :color :dimmer
            :active #'p.triggered :active-color :white
            :bg :transparent :h-align :center :v-align :center :width :fill))))))

(def pad-grid (t g page)
  (v-stack :key "pad-variant-grid" :gap 0.15
    (each (list 0 1 2 3) |row ri|
      (h-stack :key (str "pad-variant-row-" row) :gap 0.15
        (each (list 0 1 2 3) |col ci|
          (pad-cell t g (pad-cell-note page (+ (* row 4) col))))))))

;; The focused variant's name field and actions; a hint otherwise.
(def focus-strip (p)
  (if (and p (> p.variant 0))
    (h-stack :key "pad-variant-focus" :gap 0.3 :align :center :width :fill
      (text-input :key (str "pad-variant-name-" p.note) :width 6 :height 1.1 :font-size 9.5
        :value (if pad-variant-view.editing pad-variant-view.draft p.name)
        :placeholder (str "v" p.variant " " p.label)
        :on-change (lambda (v) (do (set! pad-variant-view.draft v)
                                   (set! pad-variant-view.editing true)))
        ;; Enter with nothing typed keeps the name; an emptied field clears it
        :on-submit (lambda ()
          (if pad-variant-view.editing
            (do (set! p.name pad-variant-view.draft)
                (set! pad-variant-view.editing false))
            nil)))
      (button "clear" :key "pad-variant-clear" :height 1.0 :padding 0.3 :font-size 9
        :on-click |x y r| (host-command "clear-pad-variant-values"
          (dict :group-id p.group.gid :note p.note)))
      (button "delete" :key "pad-variant-delete" :height 1.0 :padding 0.3 :font-size 9
        :on-click |x y r| (do (host-command "remove-pad-variant"
            (dict :group-id p.group.gid :note p.note))
          (focus! -999))))
    )
  )

(def pads-tab-panel (inst)
  (let ((t selection.track))
    (if (not (and t t.pad))
      (box :width 0 :height 0)
      (let ((plain t.pad) (g t.pad.group))
        (let ((page (grid-page plain))
              (focused (member-pad-at pad-variant-view.note)))
          (box :width 16.4 :background-color :black :corner-radius 16 :padding 1
            (v-stack :debug-name "pad-variant-control-panel" :width 15.4 :gap 0.3 :padding 0.3
              (h-stack :key "pad-variant-header" :gap 0.35 :height 1.2 :align :center :width :fill
                (button "<" :width 1.6 :height 1.1 :padding 0 :font-size 10
                  :on-click |x y r| (shift-page plain -1))
                (label (str (pad-note-label (pad-cell-note page 12)) "–"
                            (pad-note-label (pad-cell-note page 3)))
                  :font-size 10 :width 5 :h-align :center :color :dim :bg :transparent)
                (button ">" :width 1.6 :height 1.1 :padding 0 :font-size 10
                  :on-click |x y r| (shift-page plain 1)))
              (pad-grid t g page)
              (focus-strip focused))))))))
