;; ui/sound-palette.lisp -- sound palette overlay (takes spec 17.6 / 18.3).
;;
;; Reads the `sound-palette` host kind (open, its track, the target Apply
;; and Fork act on, and its `sound`s: the per-track Patch entries with
;; color, name and the reverse referent index). Mounted as a panel band in
;; the *arrangement* buffer (opened from a clip) and the *step* side panel
;; (opened from the instrument panel's binding badge) — the same visual
;; language as the p-lock variant chips in track-panels.lisp.
;;
;; Rows render with `each`, never `map` (repo UI rule: map renders broken
;; live while layout tests pass). Local UI state: which sound is being
;; renamed inline (17.11: rename lives in the overlay only) plus the rename
;; draft. text-input :on-change fires per keystroke, so the draft buffers
;; edits and only the explicit "ok" button commits — :on-enter is not an
;; option here because click-to-focus dispatches it, which would commit on
;; a cursor-positioning click.

(module eseq.sound-palette)

(import eseq.kinds :refer (selection song sound-palette apply-sound! apply-sound-with-mix!
                           fork-sound! open-sound-palette! close-sound-palette!))
(import eseq.view-kit :refer (color-rgba))

(export sound-rename
        open?
        entries
        close
        apply-entry
        apply-with-mix
        begin-rename
        commit-rename
        open-for-clip
        toggle-open
        panel)

;; The inline rename: the sound being renamed (nil while none) and its
;; draft.
(def-kind sound-rename
  :key ()
  :state ((sound sound :default nil)
          (draft "")))

(def open? ()
  sound-palette.open)

;; The palette's sounds (none while closed).
(def entries ()
  sound-palette.sounds)

;; The gesture target the overlay's Apply/Fork act on (17.6): the override
;; pattern's entity under an active launch — what you hear — surfaced here
;; so the deliberate target is visible.
(def target-label ()
  (match sound-palette.target
    "take" (str "Take " (+ sound-palette.target-id 1))
    "pattern" (str "Pattern " sound-palette.target-id)
    _ "scene cell"))

;; A sound's palette color at `alpha`, gray for a name-only sound (17.11
;; fallback); the :current row's faint fill is alpha 0.11, matching the
;; variant chip's treatment.
(def entry-rgba (s alpha)
  (if s.colored (color-rgba s.color alpha) (rgba 0.62 0.62 0.66 alpha)))

(def close ()
  (do
    (set! sound-rename.sound nil)
    (set! sound-rename.draft "")
    (close-sound-palette!)))

(def apply-entry (s)
  (apply-sound! s))

(def apply-with-mix (s)
  (if (< s.mix-id 0)
    (status "This entry has no known mix pairing")
    (apply-sound-with-mix! s)))

(def begin-rename (s)
  (do
    (set! sound-rename.sound s)
    (set! sound-rename.draft s.name)))

(def commit-rename (s)
  (do
    (set! s.name sound-rename.draft)
    (set! sound-rename.sound nil)
    (set! sound-rename.draft "")))

;; What the patch was loaded from: the sample name for a sampler patch, the
;; preset name otherwise (the host suffixes `*` when edited since). Empty
;; when neither is known.
(def entry-source (entry)
  (if (= entry.sample "") entry.preset entry.sample))

(def action-button (key-suffix entry text on-press)
  (button text
    :key (str key-suffix "-" entry.patch-id)
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
      (box :key (str "source-" entry.patch-id)
        :corner-radius 4
        :padding 0.12
        (label (substring source 0 22)
          :bg-color :transparent
          :font-size 7.5 :color (if current :black :dim) :bg :transparent)))))

;; Git-diff-style summary vs the current sound (17.6 amendment): "+n" params
;; higher, "-m" params lower. Empty labels when there is nothing to say (the
;; current entry itself, an identical patch, or an incompatible one).
(def diff-badges (entry)
  (let ((up entry.diff-up)
      (down entry.diff-down))
    (h-stack :gap 0.18 :align :center
      (label (if (> up 0) (str "+" up) "")
        :key (str "diff-up-" entry.patch-id)
        :font-size 8.5 :color (rgba 0.45 0.82 0.55 1.0) :bg :transparent)
      (label (if (> down 0) (str "-" down) "")
        :key (str "diff-down-" entry.patch-id)
        :font-size 8.5 :color (rgba 0.91 0.45 0.45 1.0) :bg :transparent))))

;; One palette box (17.6 + sound-glyph spec §4): a compact card — one line
;; of name (inline-renameable), diff badges and referents above the glyph,
;; the preset/sample badge below it. Half the original card size so far
;; more sounds fit on screen at once. The gray base entry is the
;; scene-effective sound: outlined swatch, not filled — exactly the p-lock
;; "def" chip treatment. Boxes tile in a responsive grid (see `panel`
;; at the bottom of this file).
(def entry-row (entry)
  (let ((c (entry-rgba entry 1.0))
      (base entry.base)
      (current entry.current))
    (box :key (str "entry-" entry.patch-id)
      :width :fill
      :height 9.0
      :padding 0.35
      :on-click |x y r| (apply-entry entry)
      :corner-radius 12
      :border-width 1
      :background-color (if current
        (entry-rgba entry 0.11)
        (rgba 1 1 1 0.025))
      :border-width (if current 0.75 0.35)
      :border-color (if current c (rgba 1 1 1 0.10))
      (v-stack :gap 0.12 :width :fill
        ;; Name + diff badges, then the "where it is used" line — both above
        ;; the glyph, on their own lines.
        (h-stack :gap 0.24 :align :center
          (if (= sound-rename.sound entry)
            ;; Draft-buffered rename: :on-change only edits the draft (it
            ;; fires per keystroke); the "ok" button commits.
            (h-stack :gap 0.2 :align :center
              (text-input :key (str "rename-" entry.patch-id)
                :width 4.6 :height 0.85 :font-size 8
                :value sound-rename.draft
                :on-change |name| (set! sound-rename.draft name))
              (action-button "rename-ok" entry "ok"
                (lambda (entry) (commit-rename entry))))
            (box :key (str "name-" entry.patch-id)
              :bg :transparent
              :on-click |x y r| (begin-rename entry)
              (label (substring (str entry.name (if base " (scene)" "")) 0 15)
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
          (if entry.track-sound
            (label "TRK"
              :key (str "trk-" entry.patch-id)
              :font-size 6.5
              :color (if current :black (entry-rgba entry 1.0))
              :bg :transparent)
            (box :bg :transparent))
          (box :flex 1 :bg :transparent)
          )
        (label (substring entry.referents-short 0 16)
          :font-size 7 :color (if current :black :dim) :bg :transparent)
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
          (sound-glyph :key (str "glyph-" entry.patch-id)
            :source entry.glyph-key
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
  (let ((t sound-palette.track))
    (if (and (open?) t)
      (let ((inst sound-palette.instrument))
        (str "Sound Pool - Track " (+ t.index 1)
          (if (= inst "") "" (str " (" inst ")"))
          " - " (target-label)))
      "Sound Pool")))

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
        :on-click |x y r| (fork-sound! sound-palette.track))
      (button "x"
        :key "close"
        :font-size 12
        :background-color (rgba 1 1 1 0.05)
        :border-color (rgba 1 1 1 0.14) :color :dim
        :on-click |x y r| (close)))))

;; Open from a clip (17.6): a take clip targets the take, a pattern clip
;; the pattern, a clip with neither falls back to the track's binding.
(def open-for-clip (c)
  (let ((t c.track)
        (cl c.cell))
    (if (>= c.take 0)
      (open-sound-palette! t :target "take" :id c.take)
      (if cl
        (open-sound-palette! t :target "pattern" :id cl.pid)
        (open-sound-palette! t)))))

;; Global toggle (bound in main.lisp): the selected/bound clip when the
;; timeline has one, else the current track's binding (badge semantics).
(def toggle-open ()
  (if (open?)
    (close)
    (let ((c song.bound-clip)
          (t selection.track))
      (if c
        (open-for-clip c)
        (if t (open-sound-palette! t) nil)))))

;; The palette surface (modal spec §4): a centered modal over the whole
;; frame instead of a band prepended to the arrangement view. The open state
;; stays app-owned (sound-palette.open); scrim clicks and Escape fire
;; :on-close, which requests close through the same funnel as the header's
;; "x" button. Closed, the modal contributes zero layout, so the mount is
;; always-safe to compose. Entries scroll inside the panel.
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
