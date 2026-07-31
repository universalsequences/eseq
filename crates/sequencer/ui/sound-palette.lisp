;; ui/sound-palette.lisp -- sound palette overlay (takes spec 17.6 / 18.3).
;;
;; Reads SEQ.sound-palette (nil = closed; else a dict with :track,
;; :target-kind/:target-id and :entries — the per-track Patch entries with
;; color, name and the reverse referent index). Mounted as a panel band in
;; the *arrangement* buffer (opened from a clip) and the *step* side panel
;; (opened from the instrument panel's binding badge) — the same visual
;; language as the p-lock variant chips in track-panels.lisp.
;;
;; Rows render with `each`, never `map` (repo UI rule: map renders broken
;; live while layout tests pass). Local UI state: which entry is being
;; renamed inline (17.11: rename lives in the overlay only) plus the rename
;; draft. text-input :on-change fires per keystroke, so the draft buffers
;; edits and only the explicit "ok" button commits — :on-enter is not an
;; option here because click-to-focus dispatches it, which would commit on
;; a cursor-positioning click.

(defstate sound-palette-renaming -1)
(defstate sound-palette-rename-draft "")

(def sound-palette-open? ()
  (not (= SEQ.sound-palette nil)))

(def sound-palette-track ()
  (get SEQ.sound-palette :track))

(def sound-palette-entries ()
  (if (sound-palette-open?) (get SEQ.sound-palette :entries) '()))

;; The gesture target the overlay's Apply/Fork act on (17.6): the override
;; pattern's entity under an active launch — what you hear — surfaced here
;; so the deliberate target is visible.
(def sound-palette-target-label ()
  (let ((kind (get SEQ.sound-palette :target-kind)))
    (if (= kind "take")
      (str "Take " (+ (get SEQ.sound-palette :target-id) 1))
      (if (= kind "pattern")
        (str "Pattern " (get SEQ.sound-palette :target-id))
        "scene cell"))))

(def sound-palette-entry-color (entry)
  (if (= (get entry :color) nil)
    (rgba 0.62 0.62 0.66 1.0)
    (rgba (get entry :color-r) (get entry :color-g) (get entry :color-b) 1.0)))

;; The :current row's faint fill, matching the variant chip's 0.11-alpha
;; treatment; gray for name-only entries (17.11 fallback).
(def sound-palette-entry-tint (entry)
  (if (= (get entry :color) nil)
    (rgba 0.62 0.62 0.66 0.11)
    (rgba (get entry :color-r) (get entry :color-g) (get entry :color-b) 0.11)))

(def sound-palette-close ()
  (do
    (set! sound-palette-renaming -1)
    (set! sound-palette-rename-draft "")
    (seq-sound-palette-close)))

(def sound-palette-apply (entry)
  (seq-sound-apply (sound-palette-track) (get entry :patch-id)))

(def sound-palette-apply-with-mix (entry)
  (if (= (get entry :mix-id) nil)
    (status "This entry has no known mix pairing")
    (seq-sound-apply-with-mix (sound-palette-track)
      (get entry :patch-id) (get entry :mix-id))))

(def sound-palette-fork ()
  (seq-sound-fork (sound-palette-track)))

(def sound-palette-begin-rename (entry)
  (do
    (set! sound-palette-renaming (get entry :patch-id))
    (set! sound-palette-rename-draft (get entry :name))))

(def sound-palette-commit-rename (entry)
  (do
    (seq-sound-rename (sound-palette-track) "patch" (get entry :patch-id)
      sound-palette-rename-draft)
    (set! sound-palette-renaming -1)
    (set! sound-palette-rename-draft "")))

(def sound-palette-action-button (key-suffix entry text on-press)
  (button text
    :key (str "sound-palette-" key-suffix "-" (get entry :patch-id))
    :width 3.4 :height 0.95 :font-size 7.5
    :background-color (rgba 1 1 1 0.05)
    :border-color (rgba 1 1 1 0.14)
    :color :dim
    :on-click |x y r| (on-press entry)))

;; One palette box (17.6 + sound-glyph spec §4): top-left swatch + name
;; (inline-renameable), the plant glyph as the center region, referents +
;; Apply / Apply-with-mix along the bottom. The gray base entry is the
;; scene-effective sound: outlined swatch, not filled — exactly the p-lock
;; "def" chip treatment. Boxes tile in a responsive grid (see
;; sound-palette-panel) so the glyph gets real area instead of a row sliver.
(def sound-palette-entry-row (entry)
  (let ((c (sound-palette-entry-color entry))
      (base (get entry :base))
      (current (get entry :current)))
    (box :key (str "sound-palette-entry-" (get entry :patch-id))
      :width :fill
      :padding 0.25
      :corner-radius 5
      :background-color (if current
        (sound-palette-entry-tint entry)
        (rgba 1 1 1 0.025))
      :border-width (if current 0.75 0.35)
      :border-color (if current c (rgba 1 1 1 0.10))
      (v-stack :gap 0.12 :width :fill
        (h-stack :gap 0.28 :align :center
          (box :width 0.36 :height 0.9
            :corner-radius 2
            :background-color (if base :transparent c)
            :border-width (if base 1 0)
            :border-color c)
          (if (= sound-palette-renaming (get entry :patch-id))
            ;; Draft-buffered rename: :on-change only edits the draft (it
            ;; fires per keystroke); the "ok" button commits.
            (h-stack :gap 0.2 :align :center
              (text-input :key (str "sound-palette-rename-" (get entry :patch-id))
                :width 6.0 :height 0.9 :font-size 8.5
                :value sound-palette-rename-draft
                :on-change |name| (set! sound-palette-rename-draft name))
              (sound-palette-action-button "rename-ok" entry "ok"
                (lambda (entry) (sound-palette-commit-rename entry))))
            (box :key (str "sound-palette-name-" (get entry :patch-id))
              :bg :transparent
              :on-click |x y r| (sound-palette-begin-rename entry)
              (label (str (substring (get entry :name) 0 9) (if base " (scene)" ""))
                :font-size 9 :color (if current :white :dim) :bg :transparent)))
          (box :flex 1 :bg :transparent))
        ;; Center glyph region (sound-glyph spec §4/P2): the plant rendered
        ;; from a host-published frame; the widget only knows the key.
        (sound-glyph :key (str "sound-palette-glyph-" (get entry :patch-id))
          :source (get entry :glyph-key)
          :height 6.5)
        (h-stack :gap 0.2 :align :center
          ;; Reverse referent index ("used by ..."), truncated so long lists
          ;; never push the action buttons out of the box.
          (label (substring (get entry :referents) 0 14)
            :key (str "sound-palette-referents-" (get entry :patch-id))
            :font-size 8 :color :dim :bg :transparent)
          (box :flex 1 :bg :transparent)
          (sound-palette-action-button "apply" entry "apply"
            (lambda (entry) (sound-palette-apply entry)))
          (sound-palette-action-button "apply-mix" entry "+mix"
            (lambda (entry) (sound-palette-apply-with-mix entry))))))))

(def sound-palette-header ()
  (box :width :fill :bg :transparent
    (h-stack :gap 0.3 :align :center
      (label (str "SOUNDS - " (sound-palette-target-label))
        :key "sound-palette-header-label"
        :font-size 8.5 :color :dim :bg :transparent)
      (box :flex 1 :bg :transparent)
      (button "fork"
        :key "sound-palette-fork"
        :width 3.4 :height 0.95 :font-size 7.5
        :background-color (rgba 1 1 1 0.05)
        :border-color (rgba 1 1 1 0.14) :color :dim
        :on-click |x y r| (sound-palette-fork))
      (button "clean"
        :key "sound-palette-cleanup"
        :width 3.4 :height 0.95 :font-size 7.5
        :background-color (rgba 1 1 1 0.05)
        :border-color (rgba 1 1 1 0.14) :color :dim
        :on-click |x y r| (seq-sound-cleanup-unused (sound-palette-track)))
      (button "x"
        :key "sound-palette-close"
        :width 1.4 :height 0.95 :font-size 7.5
        :background-color (rgba 1 1 1 0.05)
        :border-color (rgba 1 1 1 0.14) :color :dim
        :on-click |x y r| (sound-palette-close)))))

;; Open from a clip (17.6): resolve the clip's source to the palette target
;; — a take clip targets the take, a pattern clip the pattern, an empty clip
;; falls back to the track's binding.
(def sound-palette-open-for-clip (track clip-id)
  (if (>= track (len SEQ.song-lanes))
    (seq-sound-palette-open track)
    (let ((matches (filter (lambda (clip) (= (get clip :clip-id) clip-id))
                     (nth SEQ.song-lanes track))))
      (if (= (len matches) 0)
        (seq-sound-palette-open track)
        (let ((clip (nth matches 0)))
          (if (= (get clip :take-id) nil)
            (if (= (get clip :pattern-id) nil)
              (seq-sound-palette-open track)
              (seq-sound-palette-open track "pattern" (get clip :pattern-id)))
            (seq-sound-palette-open track "take" (get clip :take-id))))))))

;; Global toggle (bound in main.lisp): the selected/bound clip when the
;; timeline has one, else the current track's binding (badge semantics).
(def seq-toggle-sound-palette ()
  (if (sound-palette-open?)
    (sound-palette-close)
    (let ((bound SEQ.song-bound-clip))
      (if (= bound nil)
        (seq-sound-palette-open SEQ.current-track)
        (sound-palette-open-for-clip (nth bound 0) (nth bound 1))))))

;; The palette surface (modal spec §4): a centered modal over the whole
;; frame instead of a band prepended to the arrangement view. The open state
;; stays app-owned (SEQ.sound-palette is the :is-open binding); scrim clicks
;; and Escape fire :on-close, which requests close through the same funnel
;; as the header's "x" button. Closed, the modal contributes zero layout, so
;; the mount is always-safe to compose. Entries scroll inside the panel.
(def sound-palette-panel ()
  (modal :is-open (sound-palette-open?)
         :on-close (lambda () (sound-palette-close))
    (box :debug-name "sound-palette-panel"
      :width :fill :height :fill :bg :transparent
      (v-stack :width :fill :gap 0.22
        (sound-palette-header)
        (scroll :width :fill :flex 1
          ;; Grid of §4 boxes (not a row list): each cell gets enough area
          ;; for a legible plant glyph; the grid reflows with the panel.
          (responsive-grid :width :fill :gap 0.35
            :min-item-width 14 :min-columns 2 :max-columns 4
            ;; Explicit row height: without it the grid falls back to
            ;; slot-width * row-aspect and cells balloon to near-square.
            :row-height 10.4
            (each (sound-palette-entries) |entry idx|
              (sound-palette-entry-row entry))))))))
