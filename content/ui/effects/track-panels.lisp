;; Track-level parameter, accumulator, and parameter-lock panels.
(module eseq.effects.track-panels)

(import eseq.kinds :refer (selection transport project mute-group-options lock-param!
                           unlock-param! lock-rack-macro! unlock-rack-macro! stamp-variant!))
(import eseq.view-kit :refer (index-of color-rgba))
(import eseq.effects.state :as st)
(import eseq.effects.devices :as dv)
(import eseq.effects.param-controls :as pc)
(import eseq.drum-rack-v2)
;; The header pill + chip *step* shares with the *processes* dock.
(import eseq.panel-header :as header)
(import eseq.effects.scale-editor :as se)

(export plock-table
        poly?
        plock-row-selected?
        clear-plock-row!
        delete-selected-plock-row
        plock-chip-click
        track-plocks-panel
        step-parameters-panel
        track-parameters-panel
        toggle-polyphony
        open-polyphony-menu
        apply-polyphony-to-all-scenes
        polyphony-context-menu)

;; src/ui/input.rs reads plock-row-selected? by its qualified name.

;; The drum rack slot the track settings and the instrument header edit: the
;; current rack's selected slot (a rack's voices are its slots'), else nil.
(def settings-rack-slot ()
  (let ((t selection.track) (i selection.rack-slot))
    (if (and t t.rack (>= i 0)) (dv/rack-slot-device t.index i) nil)))

;; Whether the edited owner plays polyphonically (a rack slot: more than one
;; voice), and its voices.
(def poly? ()
  (let ((sd (settings-rack-slot)) (t selection.track))
    (if sd (> sd.voices 1) (if t t.poly false))))

(def voices ()
  (let ((sd (settings-rack-slot)) (t selection.track))
    (if sd sd.voices (if t t.max-polyphony 1))))

;; Both track settings and the instrument header edit the same selected owner.
;; Rack playback uses the slot's voice count, not the parent track's poly flag.
(def toggle-polyphony ()
  (do
    (eseq.seq-core-state/cool-off-follow)
    (if (settings-rack-slot)
      (host-command "set-rack-slot-max-polyphony"
        (dict :track selection.track.index :slot selection.rack-slot
              :value (if (poly?) 1 6)))
      (seq-set-track-param :poly (if (poly?) 0 1)))))

;; Right-click on the mono/poly button: copy just this choice to every scene.
;; The menu is an overlay, so the *fx* buffer renders it once; the button only
;; sets this state. The target (the track's position, the rack slot or -1) is
;; captured at open time so a selection change while the menu is up cannot
;; redirect it.
(def-kind polyphony-menu
  :key ()
  :state ((open false)
          (col 0)
          (row 0)
          (track -1)
          (rack-slot -1)))

(def open-polyphony-menu (event)
  (let ((t selection.track))
    (set! polyphony-menu.track (if t t.index -1))
    (set! polyphony-menu.rack-slot (if (settings-rack-slot) selection.rack-slot -1))
    (set! polyphony-menu.col (get event :col))
    (set! polyphony-menu.row (get event :row))
    (set! polyphony-menu.open true)))

(def apply-polyphony-to-all-scenes ()
  (do
    (set! polyphony-menu.open false)
    (host-command "apply-polyphony-to-all-scenes"
      (if (>= polyphony-menu.rack-slot 0)
        (dict :track polyphony-menu.track :rack-slot polyphony-menu.rack-slot)
        (dict :track polyphony-menu.track)))))

(def polyphony-context-menu ()
  (context-menu :is-open polyphony-menu.open
    :anchor-col polyphony-menu.col :anchor-row polyphony-menu.row
    :on-close (lambda () (set! polyphony-menu.open false))
    (menu-item (str "Apply " (if (poly?) "poly" "mono") " to all scenes")
      :key "polyphony-apply-all-scenes"
      :on-select (lambda (event) (apply-polyphony-to-all-scenes)))))

(def mute-group-value (label)
  (if (= label "1") 1
    (if (= label "2") 2
      (if (= label "3") 3
        (if (= label "4") 4
          (if (= label "5") 5
            (if (= label "6") 6
              (if (= label "7") 7
                (if (= label "8") 8
                  0)))))))))

(def set-timebase (label)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (if (seq-has-selection?)
      (seq-plock-timebase label)
      (seq-set-timebase label))))

;; A track setting's (timebase, swing, swing-resolution) p-lock at the
;; displayed step: its `(dict :name :value)` row of t.setting-locks, else
;; nil. Each lockable control reads it in its own subtree, so a lock coming
;; or going re-renders that control alone.
(def setting-lock (t name)
  (first (filter (lambda (row) (= (get row :name) name)) t.setting-locks)))

;; The p-lock table (selection.plock-rows, eseq.kinds' plock-row). A device
;; param's or rack macro's lock edits through its kind setters; any other
;; row through the table's host commands, addressed by r.address.

(def plock-set-value (r v)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (cond
      (r.param (lock-param! r.param (list r.step) v))
      (r.rack-macro (lock-rack-macro! r.rack-macro (list r.step) v))
      (else (host-command "set-track-plock-entry" (merge r.address :value v))))))

(def plock-set-option (r label)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (if r.param
      (let ((i (index-of r.options label)))
        (if (< i 0) nil (lock-param! r.param (list r.step) i)))
      (host-command "set-track-plock-entry-option" (merge r.address :label label)))))

(def plock-clear (r)
  (cond
    (r.param (unlock-param! r.param (list r.step)))
    (r.rack-macro (unlock-rack-macro! r.rack-macro (list r.step)))
    (else (host-command "clear-track-plock-entry" r.address))))

;; The p-lock table's selected row (its index in selection.plock-rows), -1
;; none.
(def-kind plock-table
  :key ()
  :state ((row -1)))

(def plock-param-col-width 6.45)
(def plock-lock-col-width 6.35)
(def plock-def-col-width 4.25)
(def plock-col-gap 0.22)

(def plock-row-selected? ()
  (and (>= plock-table.row 0)
       (< plock-table.row (len selection.plock-rows))))

;; Deselect the table's row (an effect selection takes the delete key).
(def clear-plock-row! ()
  (if (= plock-table.row -1) false (set! plock-table.row -1)))

(def delete-selected-plock-row ()
  (if (plock-row-selected?)
    (let ((rows selection.plock-rows)
          (idx plock-table.row)
          (r (nth rows idx)))
      (if (= r.source "preview")
        (set! plock-table.row -1)
        (let ((next-count (- (len rows) 1)))
          (do
            (plock-clear r)
            (set! plock-table.row
              (if (<= next-count 0)
                -1
                (min idx (- next-count 1))))))))
    nil))

;; The def chip's color: the theme's p-lock base, as an :rgb.
(def plock-base-color ()
  (let ((b THEME.plock_base))
    (rgb (nth b 0) (nth b 1) (nth b 2))))

;; Stamp chip v's variant (nil: the def chip) onto the selected steps, or
;; preview its locks with none selected.
(def plock-chip-click (v)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (set! plock-table.row -1)
    (if (> (len selection.steps) 0)
      (stamp-variant! selection.track selection.steps v)
      (host-command "preview-plock-variant" (dict :label (if v v.label "def"))))))

;; A variant chip (v, one of the track's variants), or the def chip (nil).
(def plock-chip (v)
  (let ((chip-label (if v v.label "def"))
        (color (if v v.color (plock-base-color)))
        (current (= selection.plock-variant chip-label))
        (c (color-rgba color 1.0)))
    (box :key (str "track-plock-chip-" (if v "variant" "def") "-" chip-label)
      :height 1.0
      :width 4.00
      :align :baseline
      :padding 0.014
      :background-color (if current
        (color-rgba color 0.11)
        :mixer-strip-bg
        )
      :border-width (if current 0.75 0.35)
      :border-color (if current c :mixer-strip-selected-bg)
      :corner-radius 4
      :on-click |x y r| (plock-chip-click v)
      (h-stack :gap 0.16 :align :baseline
        (box :width 0.18 :height 0.28
          :corner-radius 2
          :background-color (if v c :transparent)
          :border-width (if v 0 1)
          :border-color c)
        (box :width 0.2)
        (label (substring (if v v.name "base") 0 6)
          :align :center :flex 1
          :font-size 10.0 :color (if current :black :dim) :bg :transparent)
        (box :width 0.2 )
        ))))

(def plock-domain-title (domain)
  (if (= domain "inst")
    "INST"
    (if (= domain "seq")
      "SEQ"
      (if (= domain "fx")
        "FX"
        "NEURAL"))))

(def plock-domain-count (domain)
  (len (filter |r| (= r.domain domain) selection.plock-rows)))

;; A rack macro row is named by the macro's own name (rm.name, live while
;; it is renamed); any other row by its name.
(def plock-row-title (r)
  (if r.rack-macro r.rack-macro.name r.name))

(def plock-row-key (idx suffix)
  (str "track-plock-row-" idx "-" suffix))

;; The lock a row shows: its param's or rack macro's value, bound (a drag
;; repaints the row alone), else the row's own.
(def plock-row-value (r)
  (cond
    (r.param #'r.param.value)
    (r.rack-macro #'r.rack-macro.value)
    (else #'r.value)))

(def plock-group-header (domain)
  (box 
    (h-stack :gap 0.35 :align :center
      (label (plock-domain-title domain)
        :font-size 8.5 :color :dim :bg :transparent :width 4.5)
      (box :height 0.05 :width :fill :background-color (rgba 1 1 1 0.10)))))

(def plock-row (r idx)
  (subtree :key (str "track-plock-" idx "-" r.id)
    (box :width :fill
      :height 1.14
      :align :baseline
      :padding 0.07
      :background-color (if (= plock-table.row idx)
        (rgba 0.27 0.78 0.86 0.18)
        (if (= (mod idx 2) 0) (rgba 1 1 1 0.025) :transparent))
      :border-width (if (= plock-table.row idx) 1 0)
      :border-color (rgba 0.27 0.78 0.86 0.55)
      :corner-radius 2
      :on-click |x y r| (set! plock-table.row idx)
      (h-stack :width :fill :gap plock-col-gap :align :center
        (label (substring (plock-row-title r) 0 12)
          :key (plock-row-key idx "param")
          :font-size 9.2 :width plock-param-col-width
          :v-align :center
          :color (if (= plock-table.row idx) :white :dim)
          :bg :transparent)
        (if (= r.source "step")
          (if (> (len r.options) 0)
            (dropdown :value r.text
              :options r.options
              :key (plock-row-key idx "lock")
              :on-change (lambda (v) (plock-set-option r v))
              :width plock-lock-col-width :height 0.98 :font-size 8.4)
            (number-picker :value (plock-row-value r)
              :min r.min :max r.max :decimals 2
              :key (plock-row-key idx "lock")
              :noui true :font-size 9.2 :text-color :yellow :text-align :right
              :on-change (lambda (v) (plock-set-value r v))
              :width plock-lock-col-width :height 1.0))
          (label r.text
            :key (plock-row-key idx "lock")
            :font-size 9.2 :width plock-lock-col-width
            :h-align :right :color :yellow :bg :transparent))
        (label r.default-text
          :key (plock-row-key idx "def")
          :v-align :center
          :font-size 9.2 :width plock-def-col-width
          :h-align :right :color :dark-gray :bg :transparent)))))

(def plock-group (domain)
  (if (> (plock-domain-count domain) 0)
    (v-stack :gap 0.012
      (plock-group-header domain)
      (each selection.plock-rows |r idx|
        (if (= r.domain domain)
          (plock-row r idx)
          (box :height 0))))
    (box :height 0)))

(def track-plocks-panel ()
  (let ((t selection.track))
    (box :debug-name "track-plocks-panel" :padding 0.72
      (v-stack :gap 0.30
        (if t
          (label "p-locks" :height 1 :bg :transparent :color :dim :font-size 8)
          )
        
        (wrap :key "track-plock-variant-strip"
          :width :fill :gap 0.18 :row-gap 0.04 :align :start
          ;; The def chip (nil), then the track's variants.
          (each (if t (cons nil t.variants) (list)) |v idx| (plock-chip v)))
        (if (> (len selection.plock-rows) 0)
          (v-stack :key "track-plock-table" :width :fill :gap 0.1
            (h-stack :key "track-plock-table-header" :width :fill :gap plock-col-gap
              (label "PARAM" :key "track-plock-header-param"
                :font-size 8.2 :width plock-param-col-width :color :dark-gray :bg :transparent)
              (label "LOCK" :key "track-plock-header-lock"
                :font-size 8.2 :width plock-lock-col-width :h-align :right
                :color :dark-gray :bg :transparent)
              (label "DEF" :key "track-plock-header-def"
                :font-size 8.2 :width plock-def-col-width :h-align :right
                :color :dark-gray :bg :transparent))
            (plock-group "inst")
            (plock-group "seq")
            (plock-group "fx")
            (plock-group "neural"))
          )))))

(def step-set-param-direct (mode value)
  ;; The stopped-transport edit path: cursor step, or the p-lock path for a
  ;; selection.
  (do
    (eseq.seq-core-state/cool-off-follow)
    (if (seq-has-selection?)
      (seq-set-step-param-plock
        (eseq.seqv-track-params/seqv-param-keyword mode)
        (eseq.seqv-track-params/seqv-step-param-value mode value))
      (seq-set-step-param
        (eseq.seq-core-state/current-step)
        (eseq.seqv-track-params/seqv-param-keyword mode)
        (eseq.seqv-track-params/seqv-step-param-value mode value)))))

(def step-set-param (mode value)
  ;; Playing with record on: the drag arms live PRINT mode — the value lands
  ;; on the trigger steps the playhead passes, not the cursor step (bead
  ;; eseq-jc9), and only while the mouse is held (step-param-release ends
  ;; it). No cool-off-follow in that branch: the performer is watching the
  ;; playhead, so auto-follow must stay alive. The cursor step rides along
  ;; as the fallback target if the gate races off before dispatch.
  (if (and transport.playing transport.recording)
    (seq-print-step-param
      (eseq.seq-core-state/current-step)
      (eseq.seqv-track-params/seqv-param-keyword mode)
      (eseq.seqv-track-params/seqv-step-param-value mode value))
    (step-set-param-direct mode value)))

(def step-param-release (mode)
  ;; Hold-to-print: mouse-up on a picker ends that param's print
  ;; immediately. A no-op while nothing is latched (plain clicks, stopped
  ;; transport).
  (seq-print-step-param-release
    (eseq.seqv-track-params/seqv-param-keyword mode)))

(def step-duration-print-context? (mode)
  (and (= mode 1) transport.playing transport.recording))

(def step-param-min (mode)
  (if (step-duration-print-context? mode) 0.125
    (if (= mode 3) -48
      (if (= mode 1) 0
        (eseq.seqv-track-params/seqv-param-min mode)))))

(def step-param-max (mode)
  (if (step-duration-print-context? mode) 2
    (if (= mode 3) 48
      (if (= mode 1) 128
        (eseq.seqv-track-params/seqv-param-max mode)))))

;; Retrig rate (mode 8) drags on a log taper: equal drag distance is equal
;; interval, so the top of the range sweeps pitch evenly instead of crawling
;; through the rhythmic decade (docs/step-retrig-spec.md).
(def step-param-taper (mode)
  (if (= mode 8) "log"
    ;; Retrig count (mode 7) is 0..127 with most musical action at low counts.
    ;; Square keeps that range precise without making the first repeat require
    ;; the excessive travel of the old cube curve; 127/inf remains reachable.
    (if (= mode 7) "square" "linear")))

;; The retrig pickers span 0..127 / 1..1024 from a one-row strip; pin their
;; full-travel drag distance so a flick is not the whole range.
(def step-param-drag-rows (mode)
  (if (or (= mode 7) (= mode 8)) 24 0))

;; The value the picker for step param `key` shows: es's (the edited step,
;; selection.edit-step), 0 without one.
(def step-param-binding (es key)
  (if (= es nil)
    0
    (match key
      "transpose" #'es.transpose
      "velocity" #'es.velocity
      "duration" #'es.duration
      "pan" #'es.pan
      "retrig" #'es.retrig
      "retrig-rate" #'es.retrig-rate
      _ 0)))

(def step-param-picker (es mode key width)
  (box 
    :corner-radius 16 :width 12 :padding 0.2 :background-color :mixer-strip-bg 
    (h-stack :align :center :gap 0.24
      (box :width 0.5)
      (label (eseq.seqv-track-params/seqv-param-name mode) :font-size 10 :color :dim :bg :transparent :v-align :center :flex 1)
      (number-picker
        :key (str "step-param-" key)
        :value (step-param-binding es key)
        :min (step-param-min mode)
        :max (step-param-max mode)
        :taper (step-param-taper mode)
        :drag-rows (step-param-drag-rows mode)
        :decimals (eseq.seqv-track-params/seqv-param-decimals mode)
        :noui true
        :font-size 10
        :text-color :white
        :on-change (lambda (v) (step-set-param mode v))
        :on-release (lambda () (step-param-release mode))
        :width width
        :height 1.15))))

;; The track-colour helpers resolve through eseq.mixer's compat aliases, NOT
;; an import: importing eseq.mixer would evaluate mixer.lisp, whose top-level
;; (effect-buffer "*mixer*") / define-mode registrations must not ride along
;; into every VM that loads the effects family.
(def track-rgba (i dim)
  (rgba (eseq.mixer/track-color-r i dim) (eseq.mixer/track-color-g i dim)
        (eseq.mixer/track-color-b i dim) 1.0))

;; The current track's chip (eseq.panel-header's chip shape). A binding
;; cannot be negated, so the box binds t.audible as :muted with the silenced
;; look on its plain props and the heard look as the "muted" one.
(def step-track-badge ()
  (let ((t selection.track))
    (if t
      (box
        :key "step-track-badge"
        :width 4.55 :height 1.0
        :padding 0
        :corner-radius 8
        :v-align :center
        :muted #'t.audible
        :background-color (track-rgba t.index true)
        :muted-background-color (track-rgba t.index false)
        (label (eseq.mixer/track-collapsed-label t.index)
          :width 4.55
          :font-size 10
          :v-align :center
          :h-align :center
          :active #'t.audible
          :color :dim
          :active-color :black
          :bg :transparent))
      (box :width 4.55 :height 1.0))))

;; "step N · M selected": the cursor step and the selection's size, in
;; their own subtree (the step panel reads the edit step alone).
(def step-selection-summary ()
  (subtree :key "step-selection-summary-subtree"
    (let ((cursor selection.cursor-step))
      (h-stack :key "step-selection-summary" :gap 0.15 :align :center
        (number-label :key "step-cursor-label"
          :value (if cursor (+ cursor.index 1) 1)
          :prefix "step " :decimals 0 :width 3.3
          :font-size 8 :color :dim :bg :transparent)
        (label "·" :font-size 8 :color :dim :bg :transparent)
        (number-label :key "step-selection-count-label"
          :value (len selection.steps)
          :suffix " selected" :decimals 0 :width 5.0
          :font-size 8 :color :dim :bg :transparent)))))

(def step-parameters-panel ()
  (let ((es selection.edit-step))
  (box :debug-name "step-parameters-panel" :padding 0.5
    (box :padding 0.0
      :background-color :transparent ;:mixer-strip-bg
      :corner-radius 16
      :border-color :transparent ;:mixer-strip-border    
      (v-stack :gap 0.55
        (header/pill
            (step-track-badge)
            (step-selection-summary))
          (v-stack :gap 0.25 
            (h-stack :gap 0.55 :align :center
              (step-param-picker es 3 "transpose" 4.2)
              (step-param-picker es 0 "velocity" 4.2)
              )
            (h-stack :gap 0.55 :align :center
              (step-param-picker es 1 "duration" 4.2)
              (step-param-picker es 4 "pan" 4.2)
              )
            (h-stack :gap 0.55 :align :center
              (step-param-picker es 7 "retrig" 4.2)
              (step-param-picker es 8 "retrig-rate" 4.2)
              ))
        
        
        )
      ))))

;; A member of a drum rack playing a groove: the scheduler replaces track
;; swing with the rack's groove (docs/rack-groove-spec.md, "UI"), so the
;; swing control shows disabled with a hint naming the groove instead of a
;; value that would do nothing.
(def groove-swing-hint (gr)
  (let ((pg gr.pool-groove))
    (v-stack :gap 0.15 :align :center
      (label "swing" :font-size 8 :color :dim :bg :transparent :v-align :center)
      (box :key "track-swing-groove-hint"
        :debug-name "track-swing-groove-hint"
        :width 5.2 :height 1.0 :padding 0
        :h-align :center :v-align :center
        :background-color '(rgba 0.12 0.13 0.14 1.0)
        :corner-radius 3
        (label (str "groove")
          :font-size 8 :color :blue :bg :transparent :v-align :center))
      (label (substring (eseq.drum-rack-v2/pool-groove-label pg) 0 12)
        :font-size 6.5 :color :dim :bg :transparent :v-align :center))))

;; The lockable settings: each shows its lock at the displayed step (the
;; p-lock accent) over the track's own value.
(def swing-resolution-control (t)
  (subtree :key "track-swing-resolution-control"
    (let ((lock (setting-lock t "swing-resolution")))
      (v-stack :align :center :gap 0.15
        (label "swg res" :font-size 8 :color :dim :bg :transparent :v-align :center)
        (dropdown :value (if lock (get lock :value) t.swing-resolution)
          :key "track-swing-resolution"
          :options '("1/16" "1/8" "1/4" "1/2")
          :on-change (lambda (v) (eseq.seq-core-state/cool-off-follow) (seq-set-swing-resolution v))
          :plock-active (if lock 1 0)
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :width 5.0 :height 1.0 :font-size 9)))))

(def swing-control (t)
  (subtree :key "track-swing-control"
    (let ((lock (setting-lock t "swing")))
      (v-stack :gap 0.15 :align :center
        (label "swing" :font-size 8 :color :dim :bg :transparent :v-align :center)
        (number-picker :value (if lock (get lock :value) #'t.swing) :min 50 :max 75 :decimals 1
          :key "track-swing"
          :border-color :none
          :noui false :font-size 8 :text-color :dim
          :plock-active (if lock 1 0)
          :plock-default t.swing
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :on-change (lambda (v) (eseq.seq-core-state/cool-off-follow) (seq-set-track-param :swing v))
          :width 5.2 :height 1.0)))))

(def timebase-control (t)
  (subtree :key "track-timebase-control"
    (let ((lock (setting-lock t "timebase")))
      (v-stack :align :center :gap 0.15
        (label "timebase" :font-size 8 :color :dim :bg :transparent :v-align :center)
        (dropdown :value (if lock (get lock :value) t.timebase)
          :key "track-timebase"
          :options st/seq-timebase-options
          :on-change (lambda (v) (set-timebase v))
          :plock-active (if lock 1 0)
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :width 6.0 :height 1.0 :font-size 9)))))

(def track-parameters-panel ()
  (if (se/editor-open?)
    (se/scale-editor-panel)
    (track-settings-strip)))

(def track-settings-strip ()
  (let ((t selection.track))
    (if t
      (settings-strip t)
      (box :width 0 :height 0))))

(def settings-strip (t)
  (let ((poly (poly?)))
  (box :debug-name "track-parameters-strip" :padding 0.0
    (v-stack :gap 0.25
      (h-stack :gap 1.05 :align :center
        (v-stack :gap 0.15 :align :center
          (label "steps" :font-size 8 :color :dim :bg :transparent :v-align :center)
          (number-picker :value #'t.num-steps :min 1 :max 256 :decimals 0
            :border-color :none
            :noui false :font-size 8 :text-color :white
            :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-track-param :num-steps v)))
            :width 4.2 :height 1.0))
        
        (v-stack :align :center :gap 0.15
          (label "poly" :font-size 8 :color :dim :bg :transparent :v-align :center)
          (button  (if poly "ON" "OFF") :width 3.0 :height 1.0
            :background-color (if poly :control-on-bg :poly-off-bg)
            :border-color :none
            :font-size 10
            :color (if poly :control-on-fg :poly-off-fg)
            :on-click |x y r| (toggle-polyphony)
            )
          )
        (v-stack :gap 0.15 :align :center
          (label "voices" :font-size 8 :color :dim :bg :transparent :v-align :center)
          (number-picker :value (voices) :min 1 :max 12 :decimals 0
            :border-color :none
            :noui false :font-size 8 :text-color :white
            :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow)
                (if (settings-rack-slot)
                  (host-command "set-rack-slot-max-polyphony"
                    (dict :track t.index :slot selection.rack-slot :value v))
                  (seq-set-track-param :voices v))))
            :width 3.4 :height 1.0)
          )
        (if t.supports-mono-trigger
          (v-stack :align :center :gap 0.15
            (label "priority"  :font-size 8 :color :dim :bg :transparent :v-align :center)
            (dropdown :value t.voice-priority :options '("Last" "High" "Low")
              :on-change (lambda (v)
                (seq-set-track-param :voice-priority
                  (if (= v "High") 1 (if (= v "Low") 2 0))))
              :width 6.0 :height 1.0 :font-size 9)))
        (if t.supports-mono-trigger
          (v-stack :align :center :gap 0.15
            (label "trigger"  :font-size 8 :color :dim :bg :transparent :v-align :center)
            (dropdown :value t.mono-trigger :options '("retrig" "legato")
              :on-change (lambda (v)
                (seq-set-track-param :mono-trigger (if (= v "legato") 1 0)))
              :width 6.0 :height 1.0 :font-size 9)))
      
        
        )
      (h-stack :gap 1.05 :align :center
        (swing-resolution-control t)
        (v-stack :align :center :gap 0.22
          (let ((gr (eseq.drum-rack-v2/groove-of-track t)))
            (if gr (groove-swing-hint gr) (swing-control t))))
        (timebase-control t)
        
        (v-stack :align :center :gap 0.15
          (label "mute grp" :font-size 8 :color :dim :bg :transparent :v-align :center)
          (dropdown :value (nth mute-group-options t.mute-group)
            :options mute-group-options
            :on-change (lambda (v)
              (do
                (eseq.seq-core-state/cool-off-follow)
                (seq-set-track-param :mute-group (mute-group-value v))))
            :width 5.4 :height 1.0 :font-size 9))
        )
      (v-stack :align :center :gap 0.5
  	(v-stack :align :left :gap 0.15
          (label "   scale" :font-size 8 :color :dim :bg :transparent :v-align :center)
          (h-stack :gap 0.3 :align :center
            (dropdown :value t.fts
              :options project.fts-options
              :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-fts v)))
              :width 8.6 :height 1.0 :font-size 9)
            (se/scale-settings-button)))        )
      )
    )
  ))
