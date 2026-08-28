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

(module eseq.sound-palette)

(export renaming
        rename-draft
        open?
        track-index
        entries
        close
        apply-entry
        apply-with-mix
        open-for-clip
        toggle-open
        panel)

;; Migration aliases (module spec §10) for the three names unconverted
;; callers still spell flat: the two mount sites (arrangement.lisp,
;; effects/step-buffer.lisp), the `C-c p` bind-key handler string in
;; seq-panels.lisp — which is late-bound and resolves through this table at
;; dispatch — and the Rust state_values test that evals `(sound-palette-apply
;; …)`. `seq-toggle-sound-palette` is also the M-x-visible spelling.

(defstate renaming -1)
(defstate rename-draft "")

(def open? ()
  (not (= SEQ.sound-palette nil)))

(def track-index ()
  (get SEQ.sound-palette :track))

(def entries ()
  (if (open?) (get SEQ.sound-palette :entries) '()))

;; The gesture target the overlay's Apply/Fork act on (17.6): the override
;; pattern's entity under an active launch — what you hear — surfaced here
;; so the deliberate target is visible.
(def target-label ()
  (let ((kind (get SEQ.sound-palette :target-kind)))
    (if (= kind "take")
      (str "Take " (+ (get SEQ.sound-palette :target-id) 1))
      (if (= kind "pattern")
        (str "Pattern " (get SEQ.sound-palette :target-id))
        "scene cell"))))

(def entry-color (entry)
  (if (= (get entry :color) nil)
    (rgba 0.62 0.62 0.66 1.0)
    (rgba (get entry :color-r) (get entry :color-g) (get entry :color-b) 1.0)))

;; The :current row's faint fill, matching the variant chip's 0.11-alpha
;; treatment; gray for name-only entries (17.11 fallback).
(def entry-tint (entry)
  (if (= (get entry :color) nil)
    (rgba 0.62 0.62 0.66 0.11)
    (rgba (get entry :color-r) (get entry :color-g) (get entry :color-b) 0.11)))

(def close ()
  (do
    (set! renaming -1)
    (set! rename-draft "")
    (seq-sound-palette-close)))

(def apply-entry (entry)
  (seq-sound-apply (track-index) (get entry :patch-id)))

(def apply-with-mix (entry)
  (if (= (get entry :mix-id) nil)
    (status "This entry has no known mix pairing")
    (seq-sound-apply-with-mix (track-index)
      (get entry :patch-id) (get entry :mix-id))))

(def begin-rename (entry)
  (do
    (set! renaming (get entry :patch-id))
    (set! rename-draft (get entry :name))))

(def commit-rename (entry)
  (do
    (seq-sound-rename (track-index) "patch" (get entry :patch-id)
      rename-draft)
    (set! renaming -1)
    (set! rename-draft "")))

;; What the patch was loaded from: the sample name for a sampler patch, the
;; preset name otherwise (the host suffixes `*` when edited since). Empty
;; when neither is known.
(def entry-source (entry)
  (let ((sample (get entry :sample))
      (preset (get entry :preset)))
    (if (not (= sample nil)) sample
      (if (not (= preset nil)) preset ""))))

(def action-button (key-suffix entry text on-press)
  (button text
    :key (str key-suffix "-" (get entry :patch-id))
    :width 2.4 :height 0.85 :font-size 7.5
    :background-color :primary
    :color :white
    :on-click |x y r| (on-press entry)))

;; The preset/sample name as a chip badge (same visual family as the diff
;; badges); collapses to nothing when the patch has no known source.
(def source-badge (entry current)
  (let ((source (entry-source entry)))
    (if (= source "")
      (box :bg :transparent)
      (box :key (str "source-" (get entry :patch-id))
        :corner-radius 4
        :padding 0.12
        (label (substring source 0 22)
          :bg-color :transparent
          :font-size 7.5 :color (if current :black :dim) :bg :transparent)))))

;; Git-diff-style summary vs the current sound (17.6 amendment): "+n" params
;; higher, "-m" params lower. Empty labels when there is nothing to say (the
;; current entry itself, an identical patch, or an incompatible one).
(def diff-badges (entry)
  (let ((up (get entry :diff-up))
      (down (get entry :diff-down)))
    (h-stack :gap 0.18 :align :center
      (label (if (and (not (= up nil)) (> up 0)) (str "+" up) "")
        :key (str "diff-up-" (get entry :patch-id))
        :font-size 8.5 :color (rgba 0.45 0.82 0.55 1.0) :bg :transparent)
      (label (if (and (not (= down nil)) (> down 0)) (str "-" down) "")
        :key (str "diff-down-" (get entry :patch-id))
        :font-size 8.5 :color (rgba 0.91 0.45 0.45 1.0) :bg :transparent))))

;; One palette box (17.6 + sound-glyph spec §4): a compact card — one line
;; of name (inline-renameable), diff badges and referents above the glyph,
;; the preset/sample badge below it. Half the original card size so far
;; more sounds fit on screen at once. The gray base entry is the
;; scene-effective sound: outlined swatch, not filled — exactly the p-lock
;; "def" chip treatment. Boxes tile in a responsive grid (see `panel`
;; at the bottom of this file).
(def entry-row (entry)
  (let ((c (entry-color entry))
      (base (get entry :base))
      (current (get entry :current)))
    (box :key (str "entry-" (get entry :patch-id))
      :width :fill
      :height 9.0
      :padding 0.35
      :on-click |x y r| (apply-entry entry)
      :corner-radius 12
      :border-width 1
      :background-color (if current
        (entry-tint entry)
        (rgba 1 1 1 0.025))
      :border-width (if current 0.75 0.35)
      :border-color (if current c (rgba 1 1 1 0.10))
      (v-stack :gap 0.12 :width :fill
        ;; Name + diff badges, then the "where it is used" line — both above
        ;; the glyph, on their own lines.
        (h-stack :gap 0.24 :align :center
          (if (= renaming (get entry :patch-id))
            ;; Draft-buffered rename: :on-change only edits the draft (it
            ;; fires per keystroke); the "ok" button commits.
            (h-stack :gap 0.2 :align :center
              (text-input :key (str "rename-" (get entry :patch-id))
                :width 4.6 :height 0.85 :font-size 8
                :value rename-draft
                :on-change |name| (set! rename-draft name))
              (action-button "rename-ok" entry "ok"
                (lambda (entry) (commit-rename entry))))
            (box :key (str "name-" (get entry :patch-id))
              :bg :transparent
              :on-click |x y r| (begin-rename entry)
              (label (substring (str (get entry :name) (if base " (scene)" "")) 0 15)
                :font-size 8.5 :color (if current :black :dim) :bg :transparent))
            )
          (diff-badges entry)
          ;; TRK chip: this Patch/Mix pair IS the track's own sound
          ;; (track-sound spec 2.1; takes may SHARE the pair, 2.4.1). The
          ;; carrier pattern is hidden from pattern listings, so this chip
          ;; is what identifies the track sound's card.
          ;; Colored like the name label: on the CURRENT card the background
          ;; is the entry color, so an entry-colored chip would vanish into
          ;; it (cyan-on-cyan).
          (if (get entry :track-sound)
            (label "TRK"
              :key (str "trk-" (get entry :patch-id))
              :font-size 6.5
              :color (if current :black (entry-color entry))
              :bg :transparent)
            (box :bg :transparent))
          (box :flex 1 :bg :transparent)
          )
        (let ((refs (get entry :referents-short)))
          (label (substring (if (= refs nil) (get entry :referents) refs) 0 16)
            :font-size 7 :color (if current :black :dim) :bg :transparent))
        (box :height 0.1)
        (box :width :fill :height 0.15
          :corner-radius 2
          :background-color (if base :transparent c)
          :border-width (if base 1 0)
          :border-color c)
        ;; Center glyph region: rendered from a host-published frame; the
        ;; widget only knows the key. The tuned house styling lives in the
        ;; widget defaults (TUNING_PROPS, widget_render/sound_glyph.rs) so
        ;; every glyph surface shares it; add shader-knob props here (e.g.
        ;; :rim-gain, :height-amp, :interior-shade) to override live while
        ;; tuning, then bake the result back into the defaults.
        (box :background-color :bg :padding 0.15 :width :fill
          (sound-glyph :key (str "glyph-" (get entry :patch-id))
            :source (get entry :glyph-key)
            :height 4.2)
          )
        ;; What the sound is (preset / sample name), below the glyph.
        (h-stack :gap 0.24 :align :center
          (source-badge entry current)
          (box :flex 1 :bg :transparent)
          )
        )
      )
    )
  )

;; "SOUNDS - Track 5 (ultrakick) - Pattern 4": the header names the track
;; and its instrument so the overlay is never ambiguous about what it edits.
;; The closed guard matters: the modal's children still evaluate while
;; closed, and arithmetic on a nil track would kill the whole re-render.
(def header-title ()
  (if (open?)
    (let ((inst (get SEQ.sound-palette :instrument-name)))
      (str "Sound Pool - Track " (+ (track-index) 1)
        (if (or (= inst nil) (= inst "")) "" (str " (" inst ")"))
        " - " (target-label)))
    "Sound Pool"))

(def palette-header ()
  (box :width :fill :bg :transparent
    (h-stack :gap 0.3 :align :baseline
      (label (header-title)
        :key "header-label"
        :font-size 12 :color :dim :bg :transparent)
      (box :flex 1 :bg :transparent)
      ;; Fork the current sound (takes spec 17.3): clone the target's
      ;; Patch+Mix and repoint at the clones — the scene-strip "+" gesture.
      (button "+"
        :key "fork"
        :font-size 12
        :background-color :primary
        :color :white
        :on-click |x y r| (seq-sound-fork (track-index)))
      (button "x"
        :key "close"
        :font-size 12
        :background-color (rgba 1 1 1 0.05)
        :border-color (rgba 1 1 1 0.14) :color :dim
        :on-click |x y r| (close)))))

;; Open from a clip (17.6): resolve the clip's source to the palette target
;; — a take clip targets the take, a pattern clip the pattern, an empty clip
;; falls back to the track's binding.
(def open-for-clip (track clip-id)
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
(def toggle-open ()
  (if (open?)
    (close)
    (let ((bound SEQ.song-bound-clip))
      (if (= bound nil)
        (seq-sound-palette-open SEQ.current-track)
        (open-for-clip (nth bound 0) (nth bound 1))))))

;; The palette surface (modal spec §4): a centered modal over the whole
;; frame instead of a band prepended to the arrangement view. The open state
;; stays app-owned (SEQ.sound-palette is the :is-open binding); scrim clicks
;; and Escape fire :on-close, which requests close through the same funnel
;; as the header's "x" button. Closed, the modal contributes zero layout, so
;; the mount is always-safe to compose. Entries scroll inside the panel.
(def panel ()
  (modal :is-open (open?)
         :on-close (lambda () (close))
         ;; Stable screen-space size: resizing or opening an inspect source
         ;; pane must not stretch the palette. The modal clamps these bounds
         ;; to smaller windows while preserving its centered placement.
         :width-px 1260 :height-px 1000
    (box :debug-name "sound-palette-panel"
      :width :fill :height :fill :bg :transparent
      (v-stack :width :fill :gap 0.22
        (palette-header)
        (scroll :width :fill :flex 1
          ;; Grid of §4 boxes (not a row list): each cell gets enough area
          ;; for a legible plant glyph; the grid reflows with the panel.
          (responsive-grid :width :fill :gap 0.3
            :min-item-width 10 :min-columns 3 :max-columns 6
            ;; Explicit row height: without it the grid falls back to
            ;; slot-width * row-aspect and cells balloon to near-square.
            :row-height 9.0
            (each (entries) |entry idx|
              (entry-row entry))))))))
