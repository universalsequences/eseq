;; Instrument, MIDI FX, and audio FX panel body selection.
(module eseq.effects.panel-bodies)

(import eseq.effects.state :as st)
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
;; the selected keys. Live note activity rides SEQ.track-active-notes INSIDE
;; the piano's own subtree, so playback only rebuilds the keyboard — the old
;; per-key buttons read SEQ.instrument-active-notes from the panel body and
;; rebuilt the whole instrument panel on every note.
(def instrument-key-white-width 1.3)
(def instrument-key-piano-height 4.0)
(def instrument-key-panel-padding 0.35)
(def instrument-key-min-piano-width 24)
(def instrument-key-max-octaves 7)
(def instrument-key-unlocked-mark-color (list 0.62 0.62 0.66))

(def instrument-key-start-note ()
  (* (+ st/instrument-key-lock-octave 1) 12))

;; Whole octaves plus the closing C, so the view reads C3..C6.
(def instrument-key-key-count ()
  (+ (* st/instrument-key-lock-octave-count 12) 1))

(def instrument-key-piano-width ()
  (max instrument-key-min-piano-width
    (* (+ (* st/instrument-key-lock-octave-count 7) 1) instrument-key-white-width)))

(def instrument-key-panel-width ()
  (+ (instrument-key-piano-width) (* 2 instrument-key-panel-padding) 1.1))

(def instrument-key-max-start-octave ()
  (- 9 st/instrument-key-lock-octave-count))

(def instrument-key-shift-octave (delta)
  (set! st/instrument-key-lock-octave
    (max -1 (min (instrument-key-max-start-octave) (+ st/instrument-key-lock-octave delta)))))

(def instrument-key-set-octave-count (v)
  (do
    (set! st/instrument-key-lock-octave-count
      (max 1 (min instrument-key-max-octaves (round v))))
    (instrument-key-shift-octave 0)))

(def instrument-key-range-label ()
  (str "C" st/instrument-key-lock-octave
       "–C" (+ st/instrument-key-lock-octave st/instrument-key-lock-octave-count)))

(def instrument-key-note-selected? (note)
  (pg/fx-list-contains? st/instrument-key-lock-selected-notes note))

(def instrument-key-note-variant-row (inst note)
  (nth
    (filter |row| (= (get row :note) note)
      (if (get inst :key-lock-note-variants) (get inst :key-lock-note-variants) '()))
    0))

;; One `{:note :color}` per key-locked note: its variant color, or neutral
;; gray for locks that belong to no variant.
(def instrument-key-note-marks (inst)
  (let ((variant-rows (if (get inst :key-lock-note-variants) (get inst :key-lock-note-variants) '()))
        (locked (if (get inst :key-locked-notes) (get inst :key-locked-notes) '())))
    (append
      (map (lambda (row)
             (dict :note (get row :note)
                   :color (list (get row :color-r) (get row :color-g) (get row :color-b))))
        variant-rows)
      (map (lambda (note) (dict :note note :color instrument-key-unlocked-mark-color))
        (filter |note| (not (instrument-key-note-variant-row inst note)) locked)))))

;; Notes currently sounding on the instrument's track, as the piano's
;; single-source `:notes-by-track`. Capture fixtures override this to light
;; keys without a running note source.
(def instrument-key-active-notes (inst)
  (let ((notes (nth SEQ.track-active-notes (get inst :track))))
    (if notes notes '())))

(def instrument-key-activity-color (inst)
  (let ((c (nth SEQ.track-colors (get inst :track))))
    (if c c (list 1.0 0.72 0.10))))

(def instrument-key-lock-variant-items (inst)
  (if (get inst :key-lock-variants) (get inst :key-lock-variants) '()))

(def instrument-key-lock-chip-color (chip alpha)
  (let ((c (if (= (get chip :kind) "def")
        THEME.plock_base
        (list (get chip :color-r) (get chip :color-g) (get chip :color-b)))))
    (rgba (nth c 0) (nth c 1) (nth c 2) alpha)))

(def instrument-key-lock-chip-label (chip)
  (if (get chip :display)
    (substring (get chip :display) 0 6)
    (get chip :label)))

(def instrument-key-lock-chip-note-matches? (inst chip note)
  (let ((row (instrument-key-note-variant-row inst note))
        (def-chip (= (get chip :kind) "def")))
    (if def-chip
      (if row false true)
      (if row (= (get row :label) (get chip :label)) false))))

(def instrument-key-lock-chip-current? (inst chip)
  (if (pc/instrument-key-lock-has-selection?)
    (= (len (filter |note| (instrument-key-lock-chip-note-matches? inst chip note)
              st/instrument-key-lock-selected-notes))
       (len st/instrument-key-lock-selected-notes))
    (= (get chip :kind) "def")))

(def instrument-key-lock-chip-click (chip)
  (do
    ;; cool-off-follow is owned by the unconverted ui/seq-core-state.lisp —
    ;; bare, the stage-3 heal covers the read.
    (eseq.seq-core-state/cool-off-follow)
    (host-command "stamp-key-lock-variant"
      (dict :label (get chip :label)
            :notes st/instrument-key-lock-selected-notes))))

;; Same boxy chip as the *step* buffer's p-lock variants (track-panels
;; plock-chip): fixed width, color tick, dark label on the current chip.
(def instrument-key-lock-chip (inst chip)
  (let ((current (instrument-key-lock-chip-current? inst chip))
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
      :on-click |x y r| (instrument-key-lock-chip-click chip)
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
  (if (and st/instrument-key-lock-audition (instrument-key-note-selected? note))
    (host-command "audition-instrument-key" (dict :note note))
    false))

;; Plain click selects just this key (clicking the sole selected key clears
;; it); cmd toggles it into the selection; shift selects the range from the
;; last plain/cmd-clicked key.
(def instrument-key-select-note (note additive extend)
  (let ((selected st/instrument-key-lock-selected-notes)
        (anchor st/instrument-key-lock-anchor)
        (already (instrument-key-note-selected? note)))
    (do
      (if (and extend (>= anchor 0))
        (set! st/instrument-key-lock-selected-notes
          (range (min anchor note) (+ (max anchor note) 1)))
        (do
          (set! st/instrument-key-lock-anchor note)
          (set! st/instrument-key-lock-selected-notes
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
    (set! st/instrument-key-lock-selected-notes '())
    (set! st/instrument-key-lock-anchor -1)))

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
        :selected-notes st/instrument-key-lock-selected-notes
        :note-marks (instrument-key-note-marks inst)
        :label-octaves true
        :on-click (lambda (info)
          (instrument-key-select-note (get info :note)
            (get info :additive-selection) (get info :shift)))))))

(def instrument-key-lock-control-panel (inst)
  (let ((selected-count (len st/instrument-key-lock-selected-notes)))
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
            :value st/instrument-key-lock-octave-count :min 1 :max instrument-key-max-octaves
            :on-change (lambda (v) (instrument-key-set-octave-count v)))
          (box :flex 1 :height 0.1)
          (if (> selected-count 0)
            (button "unselect all" :debug-name "instrument-key-unselect-all"
              :width 6 :height 1.0 :padding 0 :font-size 9
              :on-click |x y r| (instrument-key-unselect-all))
            (box :width 0 :height 0))
          (box :key "instrument-key-audition" :debug-name "instrument-key-audition"
            :width 1.8 :height 1.2 :align :center
            :on-click |x y r| (set! st/instrument-key-lock-audition (not st/instrument-key-lock-audition))
            (instrument-key-audition-icon :active (if st/instrument-key-lock-audition 1 0))))
        (instrument-key-piano inst)
        (wrap :key "instrument-key-lock-variant-strip"
          :width :fill :gap 0.18 :row-gap 0.04 :align :start
          (each (instrument-key-lock-variant-items inst) |chip idx|
            (instrument-key-lock-chip inst chip)))))))

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
        (if (= st/instrument-panel-tab 1)
          (h-stack :debug-name "instrument-keys-inline-body" :height :fill :gap 0.45 :align :stretch
            (instrument-key-lock-control-panel inst)
            body)
          (if st/instrument-mods-open
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

(def fx-panel-selected? (fx)
  (do
    SEQ.delete-target-version
    (if (get fx :rack-fx)
      (seq-delete-target? :fx-effect
        (dict :chain "rack"
              :track (get fx :track-idx)
              :rack-slot (get fx :rack-slot)
              :effect-slot (get fx :slot-idx)))
      (if (get fx :midi-fx)
      (seq-delete-target? :fx-effect (dict :chain "midi" :slot (get fx :slot-idx)))
      (if (get fx :bus-fx)
        (seq-delete-target? :fx-effect
          (dict :chain "bus" :bus (get fx :bus-idx) :slot (get fx :slot-idx)))
        (seq-delete-target? :fx-effect (dict :chain "audio" :slot (get fx :slot-idx))))))))

(def fx-panel-header-bg (selected)
  (if selected :fx-panel-header-selected-bg :fx-panel-header-bg))
