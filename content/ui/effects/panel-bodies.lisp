;; Instrument, MIDI FX, and audio FX panel body selection.
(module eseq.effects.panel-bodies)

(import eseq.kinds :refer (stamp-key-variant!))
(import eseq.effects.state :as st :refer (instrument-view key-lock-view))
(import eseq.effects.devices :as dv)
(import eseq.view-kit :refer (rgb-part color-rgba listed?))
(import eseq.effects.param-controls :as pc)
(import eseq.effects.param-grid :as pg)
(import eseq.effects.effect-modulation :as em)
(import eseq.effects.instrument-modulation :as im)
(import eseq.effects.builtin.audio-fx :as afx)

(export instrument-key-active-notes
        instrument-key-note-variant-row
        instrument-key-lock-variant-items
        instrument-key-lock-chip-current?
        instrument-key-lock-chip-click
        instrument-key-select-note
        instrument-key-unselect-all
        instrument-synth-panel-body
        midi-fx-panel-body
        audio-fx-panel-body
        fx-panel-selected?)

;; Migration aliases (module spec §10). Every name keeps its spelling — the
;; `instrument-key-` prefix is the piano-key domain, not a module prefix, so
;; nothing strips. Callers: effects/instrument-panel.lisp (unconverted,
;; instrument-synth-panel-body) plus the Rust test harnesses in
;; src/ui/state_values/tests.rs that eval the flat spellings.
;; instrument-key-active-notes is special: ui/capture-fixtures/
;; instrument-keys-activity-panel.lisp REDEFINES it headerless to light keys
;; in a capture — the alias covers writes too (last-writer-wins into this
;; module's slot), and the fixture compiles long after this file loads.
;; Converted callers (eseq.effects.effect-panels for midi-fx-panel-body,
;; audio-fx-panel-body, fx-panel-selected?) import this module instead —
;; no alias for names only they reference. Deleted as callers convert.

;; Keys tab: a multi-octave `piano-keyboard` for choosing which notes the
;; synth knobs key-lock, plus the variant chips that stamp a lock set onto
;; the selected keys. Live note activity rides the track's active-notes
;; INSIDE the piano's own subtree, so playback only rebuilds the keyboard —
;; the old per-key buttons read the active notes from the panel body and
;; rebuilt the whole instrument panel on every note. The key locks and their
;; variants are the instrument device's (eseq.kinds).
(def instrument-key-white-width 1.3)
(def instrument-key-piano-height 4.0)
(def instrument-key-panel-padding 0.35)
(def instrument-key-min-piano-width 24)
(def instrument-key-max-octaves 7)
(def instrument-key-unlocked-mark-color (list 0.62 0.62 0.66))

(def instrument-key-start-note ()
  (* (+ key-lock-view.octave 1) 12))

;; Whole octaves plus the closing C, so the view reads C3..C6.
(def instrument-key-key-count ()
  (+ (* key-lock-view.octave-count 12) 1))

(def instrument-key-piano-width ()
  (max instrument-key-min-piano-width
    (* (+ (* key-lock-view.octave-count 7) 1) instrument-key-white-width)))

(def instrument-key-panel-width ()
  (+ (instrument-key-piano-width) (* 2 instrument-key-panel-padding) 1.1))

(def instrument-key-max-start-octave ()
  (- 9 key-lock-view.octave-count))

(def instrument-key-shift-octave (delta)
  (set! key-lock-view.octave
    (max -1 (min (instrument-key-max-start-octave) (+ key-lock-view.octave delta)))))

(def instrument-key-set-octave-count (v)
  (do
    (set! key-lock-view.octave-count
      (max 1 (min instrument-key-max-octaves (round v))))
    (instrument-key-shift-octave 0)))

(def instrument-key-range-label ()
  (str "C" key-lock-view.octave
       "–C" (+ key-lock-view.octave key-lock-view.octave-count)))

;; Whether `note` is one of the keys selected for key locks.
(def instrument-key-note-selected? (note)
  (listed? note key-lock-view.notes))

;; inst's key locks, resolved once per render: its instrument's variants,
;; each variant-stamped note as a `(note variant)` row, and its locked notes.
(def key-locks-of (inst)
  (let ((d (dv/inst-device inst)))
    (let ((variants (if d d.variants '())))
      (dict :variants variants
            :by-note (reduce (lambda (rows v) (append rows (map (lambda (n) (list n v)) v.notes)))
                       '() variants)
            :locked (if d d.key-locked-notes '())))))

;; The variant of `keys` (key-locks-of) stamped on `note`, or nil.
(def variant-at (keys note)
  (let ((row (first (filter (lambda (row) (= (first row) note)) (get keys :by-note)))))
    (when row (nth row 1))))

;; The key-lock variant stamped on `note`, or nil.
(def instrument-key-note-variant-row (inst note)
  (variant-at (key-locks-of inst) note))

(def color-list (c)
  (list (rgb-part c 0) (rgb-part c 1) (rgb-part c 2)))

;; One `{:note :color}` per key-locked note: its variant color, or neutral
;; gray for locks that belong to no variant.
(def instrument-key-note-marks (keys)
  (append
    (map (lambda (row) (let ((v (nth row 1))) (dict :note (first row) :color (color-list v.color))))
      (get keys :by-note))
    (map (lambda (note) (dict :note note :color instrument-key-unlocked-mark-color))
      (filter |note| (= (variant-at keys note) nil) (get keys :locked)))))

;; Notes currently sounding on the instrument's track, as the piano's
;; single-source `:notes-by-track`. Capture fixtures override this to light
;; keys without a running note source.
(def instrument-key-active-notes (inst)
  (let ((t (dv/track-at (get inst :track))))
    (if t t.active-notes '())))

(def instrument-key-activity-color (inst)
  (let ((t (dv/track-at (get inst :track))))
    (if t (color-list t.color) (list 1.0 0.72 0.10))))

;; The chips of `keys`: the base (no variant, `def`), then each variant.
(def key-lock-chips (keys)
  (cons (dict :kind "def" :label "def" :variant nil)
    (map (lambda (v) (dict :kind "variant" :label v.label :variant v))
      (get keys :variants))))

(def instrument-key-lock-variant-items (inst)
  (key-lock-chips (key-locks-of inst)))

(def instrument-key-lock-chip-color (chip alpha)
  (let ((v (get chip :variant)))
    (if v
      (color-rgba v.color alpha)
      (rgba (nth THEME.plock_base 0) (nth THEME.plock_base 1) (nth THEME.plock_base 2) alpha))))

(def instrument-key-lock-chip-label (chip)
  (let ((v (get chip :variant)))
    (if v (substring v.name 0 6) "base")))

;; Whether the chip is the selected keys' variant (every selected key
;; stamped with it); with no key selected, the base chip.
(def key-lock-chip-current? (keys chip)
  (if (pc/instrument-key-lock-has-selection?)
    (empty? (filter |note| (not (= (variant-at keys note) (get chip :variant)))
              key-lock-view.notes))
    (= (get chip :kind) "def")))

(def instrument-key-lock-chip-current? (inst chip)
  (key-lock-chip-current? (key-locks-of inst) chip))

;; Stamp the chip's variant (the base chip: none) onto the selected keys of
;; inst's instrument.
(def instrument-key-lock-chip-click (inst chip)
  (let ((d (dv/inst-device inst)))
    (do
      ;; cool-off-follow is owned by the unconverted ui/seq-core-state.lisp —
      ;; bare, the stage-3 heal covers the read.
      (eseq.seq-core-state/cool-off-follow)
      (if d (stamp-key-variant! d key-lock-view.notes (get chip :variant)) nil))))

;; Same boxy chip as the *step* buffer's p-lock variants (track-panels
;; plock-chip): fixed width, color tick, dark label on the current chip.
(def instrument-key-lock-chip (inst keys chip)
  (let ((current (key-lock-chip-current? keys chip))
      (def-chip (= (get chip :kind) "def"))
      (c (instrument-key-lock-chip-color chip 1.0)))
    (box :key (str "instrument-key-lock-chip-" (get chip :kind) "-" (get chip :label))
      :height 1.0
      :width 3.5
      :align :baseline
      :padding 0.014
      :background-color (if current
        (instrument-key-lock-chip-color chip 0.11)
        :mixer-strip-bg)
      :border-width (if current 0.75 0.35)
      :border-color (if current c :mixer-strip-selected-bg)
      :corner-radius 4
      :on-click |x y r| (instrument-key-lock-chip-click inst chip)
      (h-stack :gap 0.16 :align :baseline
        (box :width 0.18 :height 0.28
          :corner-radius 2
          :background-color (if def-chip :transparent c)
          :border-width (if def-chip 1 0)
          :border-color c)
        (box :width 0.2)
        (label (instrument-key-lock-chip-label chip)
          :align :center :flex 1
          :font-size 10.0 :color (if current :black :dim) :bg :transparent)
        (box :width 0.2)))))

(def instrument-key-audition (note)
  (if (and key-lock-view.audition (instrument-key-note-selected? note))
    (host-command "audition-instrument-key" (dict :note note))
    false))

;; Plain click selects just this key (clicking the sole selected key clears
;; it); cmd toggles it into the selection; shift selects the range from the
;; last plain/cmd-clicked key.
(def instrument-key-select-note (note additive extend)
  (let ((selected key-lock-view.notes)
        (anchor key-lock-view.anchor)
        (already (instrument-key-note-selected? note)))
    (do
      (if (and extend (>= anchor 0))
        (set! key-lock-view.notes
          (range (min anchor note) (+ (max anchor note) 1)))
        (do
          (set! key-lock-view.anchor note)
          (set! key-lock-view.notes
            (if additive
              (if already
                (filter |n| (not (= n note)) selected)
                (append selected (list note)))
              (if (and already (= (len selected) 1))
                '()
                (list note))))))
      (instrument-key-audition note))))

(def instrument-key-unselect-all ()
  (do
    (set! key-lock-view.notes '())
    (set! key-lock-view.anchor -1)))

(defwidget instrument-key-audition-icon
  :width 1.6 :height 1.1
  :paint-margin 0.2
  :state (active)
  :shader
  (let ((badge-col (if (= active 1) (rgba 0.95 0.74 0.22 1.0) (rgba 0.22 0.23 0.25 1.0)))
        (glyph-col (if (= active 1) (rgba 0.08 0.08 0.09 1.0) (rgba 0.66 0.68 0.72 1.0)))
        (band (max (- (abs (- (sqrt (+ (* x x) (* y y))) 0.36)) 0.06) y)))
    (sdf/layer
      (sdf/fill (sdf/circle 0.72) (material :color badge-col))
      (sdf/fill band (material :color glyph-col))
      (sdf/fill (sdf/translate -0.36 0.12 (sdf/rounded-rect 0.14 0.34 0.11))
        (material :color glyph-col))
      (sdf/fill (sdf/translate 0.36 0.12 (sdf/rounded-rect 0.14 0.34 0.11))
        (material :color glyph-col)))))

(def instrument-key-piano (inst)
  (subtree :key (str "instrument-key-piano-" (get inst :track) "-" (get inst :rack-slot))
    (box :debug-name "instrument-keys-frame"
      :padding 0.3 :background-color :mixer-strip-bg
      :border-color :mixer-strip-border :corner-radius 8
      (piano-keyboard
        :key "instrument-key-piano"
        :debug-name "instrument-key-piano"
        :start-note (instrument-key-start-note)
        :key-count (instrument-key-key-count)
        :width (instrument-key-piano-width)
        :height instrument-key-piano-height
        :notes-by-track (list (instrument-key-active-notes inst))
        :track-colors (list (instrument-key-activity-color inst))
        :tracks (list 0)
        :overlap-mode :loudest
        :press-depth 0.6
        :selected-notes key-lock-view.notes
        :note-marks (instrument-key-note-marks (key-locks-of inst))
        :label-octaves true
        :on-click (lambda (info)
          (instrument-key-select-note (get info :note)
            (get info :additive-selection) (get info :shift)))))))

(def instrument-key-lock-control-panel (inst)
  (let ((selected-count (len key-lock-view.notes)))
    (box :width (+ (instrument-key-panel-width) 2) :background-color :black :corner-radius 16 :padding 1
      (v-stack :debug-name "instrument-key-lock-control-panel"
        :width (instrument-key-panel-width) :height st/fx-panel-body-content-height
        :gap 0.4 :padding instrument-key-panel-padding
        (h-stack :debug-name "instrument-key-header" :gap 0.35 :height 1.2 :align :center :width :fill
          (button "<" :width 1.6 :height 1.1 :padding 0 :font-size 10
            :on-click |x y r| (instrument-key-shift-octave -1))
          (label (instrument-key-range-label) :font-size 10 :width 4.4 :h-align :center
            :color :dim :bg :transparent)
          (button ">" :width 1.6 :height 1.1 :padding 0 :font-size 10
            :on-click |x y r| (instrument-key-shift-octave 1))
          (box :width 0.4)
          (label "oct" :font-size 9 :width 1.8 :color :dim :bg :transparent)
          (number-picker :debug-name "instrument-key-octave-count"
            :width 2.4 :height 1.0 :noui true :font-size 9.5 :decimals 0 :step 1
            :value key-lock-view.octave-count :min 1 :max instrument-key-max-octaves
            :on-change (lambda (v) (instrument-key-set-octave-count v)))
          (box :flex 1 :height 0.1)
          (if (> selected-count 0)
            (button "unselect all" :debug-name "instrument-key-unselect-all"
              :width 6 :height 1.0 :padding 0 :font-size 9
              :on-click |x y r| (instrument-key-unselect-all))
            (box :width 0 :height 0))
          (box :key "instrument-key-audition" :debug-name "instrument-key-audition"
            :width 1.8 :height 1.2 :align :center
            :on-click |x y r| (toggle! key-lock-view.audition)
            (instrument-key-audition-icon :active (if key-lock-view.audition 1 0))))
        (instrument-key-piano inst)
        (wrap :key "instrument-key-lock-variant-strip"
          :width :fill :gap 0.18 :row-gap 0.04 :align :start
          (let ((keys (key-locks-of inst)))
            (each (key-lock-chips keys) |chip idx|
              (instrument-key-lock-chip inst keys chip))))))))

;; The custom-*-ui dispatchers and custom-ui-current-kind are a host->script
;; protocol: src/ui/custom_ui.rs GENERATES headerless lisp that (re)defines
;; the dispatchers by bare spelling on every custom-UI rebuild, and
;; eseq.effects.state pins custom-ui-current-kind to eseq.vanilla for the
;; same reason (hazard i). A bare reference here would intern this module's
;; own slot and strand it on the first-healed cell when the codegen re-defs
;; (hazard m), so both directions use the §3 escape-hatch spelling, which
;; shares the exact eseq.vanilla slot the codegen writes.
(def instrument-synth-panel-body (inst)
  (do
    (set! eseq.vanilla/custom-ui-current-kind "instrument")
    (let ((custom (eseq.vanilla/custom-instrument-synth-ui inst)))
      (let ((body
              (if custom
                (box custom
                  :debug-name "custom-synth-wrapper" :padding 0
                  :h-align :start :v-align :stretch)
                (box (pg/fx-param-grid (get inst :synth) false)
                  :debug-name "fallback-synth-wrapper"))))
        (if (= instrument-view.tab 1)
          (h-stack :debug-name "instrument-keys-inline-body" :height :fill :gap 0.45 :align :stretch
            (instrument-key-lock-control-panel inst)
            body)
          (if instrument-view.mods-open
          (h-stack :debug-name "instrument-mods-inline-body" :height :fill :gap 0.45 :align :stretch
            (im/mod-control-panel inst)
            body)
          body))))))

(def midi-fx-panel-body (fx)
  (do
    (set! eseq.vanilla/custom-ui-current-kind "midi-fx")
    (let ((custom (eseq.vanilla/custom-midi-fx-ui fx)))
      (if custom
        (box
          (v-stack :gap 0.25 custom)
          :debug-name "custom-midi-fx-wrapper" :padding 0 :h-align :start :v-align :start)
        (box (pg/fx-param-grid (get fx :params) fx)
          :debug-name "fallback-midi-fx-wrapper")))))

(def audio-fx-panel-body (fx params)
  (let ((builtin-ui (afx/builtin-audio-fx-ui fx)))
    (let ((body
            (if builtin-ui
              builtin-ui
              (do
                (set! eseq.vanilla/custom-ui-current-kind "audio-fx")
                (let ((custom (eseq.vanilla/custom-audio-fx-ui fx)))
                  (if custom
                    (box
                      (v-stack :gap 0.25 custom)
                      :debug-name "custom-audio-fx-wrapper" :padding 0 :h-align :start :v-align :start)
                    (pg/fx-param-grid params fx)))))))
      (if (pc/effect-mods-active? fx)
        (h-stack :debug-name "effect-mods-inline-body" :height st/fx-panel-body-content-height :gap 0.45 :align :stretch
          (em/mod-control-panel fx)
          body)
        body))))

;; The effect is the delete target (Backspace deletes it).
(def fx-panel-selected? (fx)
  (let ((d (dv/fx-device fx)))
    (if d d.delete-target false)))

(def fx-panel-header-bg (selected)
  (if selected :fx-panel-header-selected-bg :fx-panel-header-bg))
