;; ui/piano-roll.lisp -- step-quantized piano roll for current track.
;; Renders to *piano-roll* buffer. Loaded by ui/main.lisp.
;;
;; Converted to a module in S3b. This file is a small hub: its names are
;; spelled FLAT from four directions — production Rust
;; (`src/ui/piano_roll.rs` reads `piano-roll-fit-pending` and invokes
;; `piano-roll-apply-pending-fit` through `rt.global_value(...)`), two Rust
;; harnesses that eval the WHOLE file into a bare VM and then poke it with
;; flat `eval_str` (`metal_seq_piano_roll_lisp_loads`,
;; `sync_piano_roll_state_applies_pending_track_fit_after_items_update`), the
;; unconverted `ui/seq-panels.lisp` / `ui/seq-step-tabs.lisp`, and the already
;; converted `ui/arrangement.lisp`. So the externally reachable names convert
;; with NO renames and *identity* compat aliases (the seq-core-state
;; precedent): an unconverted caller matches the alias key flat, a converted
;; module's bare reference qualifies against itself, misses, and lands on the
;; same alias by base name. Everything with no caller outside this file is
;; `%`-private.
;;
;; `piano-roll-fit-pending` is the one *mutable plain def* here (hazard m), so
;; it is PINNED to eseq.vanilla rather than aliased — an alias cannot protect a
;; slot that a later flat write could unlink, and production Rust reads it by
;; its flat spelling every tick. Every in-file reference therefore spells the
;; pinned name in full.
(module eseq.piano-roll)

;; Compile-time edge (spec §4): `piano-roll-default-pane-height` moved home
;; to eseq.seq-step-tabs (the layout hub) so that hub stops reading a render
;; root at load time; this import supplies the alias/def before our readers
;; compile. Resolution in the whole-file harness VMs works per (n2): module
;; files resolve against the source-manager cwd, which is crates/sequencer.
(import eseq.seq-step-tabs)

(export piano-roll-view-start
        piano-roll-view-duration
        piano-roll-lane-scroll
        piano-roll-lane-height
        piano-roll-arrangement-mode?
        piano-roll-max-lane-scroll
        piano-roll-request-fit-for-track
        piano-roll-request-fit
        piano-roll-apply-pending-fit
        piano-roll-action)

;; `cool-off-follow` belongs to the converted eseq.seq-core-state. It is
;; referenced BARE on purpose: it is an event-time call, so the base-name
;; rung of the late-binding heal resolves it through the identity alias —
;; no compile-time surface needed, hence no import.


(defstate tool :pointer)
(defstate piano-roll-view-start 0)
(defstate piano-roll-view-duration 10.6667)
(defstate piano-roll-lane-scroll 36)
(defstate piano-roll-lane-height 0.5)
(defstate cursor-time 0)
(defstate selection-rect nil)
(defstate piano-roll-status "piano roll")
(defstate create-duration 1)
;; PINNED (hazard m): mutable plain def, read flat by production Rust.
(def eseq.vanilla/piano-roll-fit-pending false)
(def fit-track -1)

;; True only when the lower piano roll was entered from arrangement clip
;; gestures. Kept in a reactive channel rather than a source defstate because
;; activating the effect buffer evaluates its source; the entry mode must
;; survive that activation. Missing/unseeded reads are ordinary session mode.
(def piano-roll-arrangement-mode? ()
  (= (reactive-get "SEQV" "piano-roll-arrangement-mode") 1))

(def timeline-height 35)
(def header-height 2)
(def view-padding 1)
(def min-view-duration 4)
(def max-view-duration 256)

(def native-action? (event)
  (or (= event.type :select)
      (= event.type :clear-selection)
      (= event.type :finish-marquee-select)
      (= event.type :delete-items)
      (= event.type :copy-items)
      (= event.type :paste-items)
      (= event.type :nudge-selection)
      (= event.type :move-items-absolute)
      (= event.type :resize-item-absolute)
      (= event.type :finish-move-items)
      (= event.type :finish-resize-items)
      (= event.type :finish-create-item)))

(def event-num (event key fallback)
  (let ((value (get event key)))
    (if (= value nil) fallback value)))

(def lane-height-value ()
  (if (= piano-roll-lane-height nil) 1 piano-roll-lane-height))

(def lane-count ()
  (max 1 (len SEQ.piano-roll-lanes)))

(def content-height ()
  (max 1
    (- eseq.seq-step-tabs/piano-roll-default-pane-height
      header-height)))

(def visible-lane-count ()
  (/ (content-height) (lane-height-value)))

(def piano-roll-max-lane-scroll ()
  (max 0 (- (lane-count) (visible-lane-count))))

(def clamp-lane-scroll (scroll)
  (max 0 (min (piano-roll-max-lane-scroll) scroll)))

(def set-lane-scroll (scroll)
  (set! piano-roll-lane-scroll (clamp-lane-scroll scroll)))

(def action-duration (event)
  (max 0.03125
    (event-num event :duration
      (- (event-num event :end 1)
         (event-num event :start 0)))))

(def set-cursor-from-event (event)
  (let ((time (get event :time)))
    (if (= time nil)
      nil
      (set! cursor-time time))))

(def current-track-color ()
  (if (and (< SEQ.current-track (len SEQ.track-colors)) (>= SEQ.current-track 0))
    (nth SEQ.track-colors SEQ.current-track)
    (list 0.34 0.48 0.98)))

;; The piano roll's axis is the FOCUS length (clip-edit-target spec 3.5):
;; a pinned pattern's num-steps, a pinned take's playable length, or the
;; live pattern's num-steps in follow mode. SEQ.tp-num-steps stays the live
;; value for the step grid.
(def num-steps ()
  (let ((focus (or SEQ.focus-num-steps 0)))
    (if (> focus 0) focus SEQ.tp-num-steps)))

(def max-view-start (duration)
  (max 0 (- (+ (num-steps) 4) duration)))

(def set-view-start (start duration)
  (set! piano-roll-view-start
    (max 0 (min (max-view-start duration) start))))

(def zoom-view (event)
  (let ((cur-duration piano-roll-view-duration)
        (factor (event-num event :factor 1)))
    (let ((next-duration (max min-view-duration
                           (min max-view-duration
                             (/ piano-roll-view-duration factor)))))
      (let ((anchor-ratio
              (if (<= cur-duration 0)
                0.5
                (max 0 (min 1 (/ (- (event-num event :anchor-time piano-roll-view-start) piano-roll-view-start) cur-duration))))))
        (do
          (set! piano-roll-view-duration next-duration)
          (set-view-start
            (- (event-num event :anchor-time piano-roll-view-start) (* anchor-ratio next-duration))
            next-duration))))))

(def zoom-lanes (event)
  (let ((cur-height (lane-height-value))
        (factor (event-num event :factor 1)))
    (let ((next-height (max 0.5 (min 6 (* (lane-height-value) factor)))))
      (let ((anchor-lane (event-num event :anchor-lane piano-roll-lane-scroll)))
        (let ((anchor-offset (- anchor-lane piano-roll-lane-scroll)))
          (do
            (set! piano-roll-lane-height next-height)
            (set-lane-scroll
              (- anchor-lane
                (* anchor-offset (/ cur-height next-height))))))))))

(def has-items? ()
  (> (len SEQ.piano-roll-items) 0))

(def item-start (item)
  (event-num item :start 0))

(def item-end (item)
  (max (item-start item)
    (event-num item :end (item-start item))))

(def item-lane (item)
  (event-num item :lane 0))

(def fit-horizontal (min-start max-end)
  (let ((start (max 0 (- min-start view-padding)))
        (end (max min-start (+ max-end view-padding))))
    (let ((duration (max min-view-duration
                      (min max-view-duration
                        (- end start)))))
      (do
        (set! piano-roll-view-duration duration)
        (set-view-start start duration)))))

(def fit-vertical (min-lane max-lane)
  (let ((center (/ (+ min-lane max-lane 1) 2)))
    (set-lane-scroll
      (- center (/ (visible-lane-count) 2)))))

(def fit-empty-view ()
  (do
    (set! piano-roll-view-duration
      (max min-view-duration
        (min max-view-duration (num-steps))))
    (set-view-start 0 piano-roll-view-duration)
    (fit-vertical
      (floor (/ (- (lane-count) 1) 2))
      (floor (/ (- (lane-count) 1) 2)))))

(def fit-notes-to-view ()
  (if (has-items?)
    (let ((first (nth SEQ.piano-roll-items 0)))
      (let ((min-start (reduce |acc item| (min acc (item-start item))
                         (item-start first)
                         SEQ.piano-roll-items))
            (max-end (reduce |acc item| (max acc (item-end item))
                       (item-end first)
                       SEQ.piano-roll-items))
            (min-lane (reduce |acc item| (min acc (item-lane item))
                        (item-lane first)
                        SEQ.piano-roll-items))
            (max-lane (reduce |acc item| (max acc (item-lane item))
                        (item-lane first)
                        SEQ.piano-roll-items)))
        (do
          (fit-horizontal min-start max-end)
          (fit-vertical min-lane max-lane))))
    (fit-empty-view)))

(def piano-roll-request-fit-for-track (track)
  (do
    (set! eseq.vanilla/piano-roll-fit-pending true)
    (set! fit-track track)
    (piano-roll-apply-pending-fit)))

(def piano-roll-request-fit ()
  (piano-roll-request-fit-for-track SEQ.current-track))

(def piano-roll-apply-pending-fit ()
  (if (and eseq.vanilla/piano-roll-fit-pending (= fit-track SEQ.current-track))
    (do
      (fit-notes-to-view)
      (set! eseq.vanilla/piano-roll-fit-pending false))
    nil))

(def piano-roll-action (event)
  (do
    (match event.type
      :scroll-view
      (let ((view-start (get event :view-start))
            (lane-scroll (get event :lane-scroll)))
        (do
          (set-view-start
            (if (= view-start nil)
              (+ piano-roll-view-start (event-num event :delta-time 0))
              view-start)
            piano-roll-view-duration)
          (set-lane-scroll
            (if (= lane-scroll nil)
              (+ piano-roll-lane-scroll (event-num event :delta-lanes 0))
              lane-scroll))))
      :zoom-view
      (zoom-view event)
      :zoom-lanes
      (zoom-lanes event)
      :set-cursor
      (set! cursor-time event.time)
      ;; The loop bar edits the FOCUSED source's length (clip-edit-target
      ;; spec 5, locked decision 3): the live pattern through today's track
      ;; path, a pinned pattern through the pattern-addressed write (the
      ;; SHARED pattern — every clip referencing it). A take's length is
      ;; owned by recording/splice, so its band is read-only.
      :resize-content-length
      (match SEQ.focus-kind
        :live
        (do
          ;; eseq.seq-core-state, reached bare through its identity alias.
          (eseq.seq-core-state/cool-off-follow)
          (seq-set-track-param :num-steps event.length))
        :pattern
        (host-command "focus-set-num-steps"
          (dict :track SEQ.current-track :length event.length))
        :take nil)
      :finish-resize-content-length
      (if (= SEQ.focus-kind :pattern)
        (host-command "focus-finish-num-steps" (dict :track SEQ.current-track))
        nil)
      ;; Band-body slide (spec 5): live frames are preview-only; the release
      ;; carries the TOTAL delta and lowers to one undoable phase edit.
      :slide-band nil
      :finish-slide-band
      (let ((delta (round (event-num event :delta-time 0))))
        (if (or (not (= SEQ.focus-clip-kind :pattern)) (= delta 0))
          nil
          (host-command "focus-slide-band"
            (dict :track SEQ.current-track :delta-steps delta))))
      :marquee-select
      (set! selection-rect event)
      :finish-marquee-select
      (set! selection-rect nil)
      :select
      (do
        (set! selection-rect nil)
        (set-cursor-from-event event))
      :clear-selection
      (do
        (set! selection-rect nil)
        (set-cursor-from-event event))
      :finish-create-item
      (set! create-duration (action-duration event))
      :resize-item-absolute
      (set! create-duration (action-duration event)))
    (if (native-action? event)
      (set! piano-roll-status (seq-piano-roll-action event))
      (set! piano-roll-status "piano roll"))))

(def piano-roll-timeline ()
  (box :height :fill :flex 1 :width 0
    ;; Flex child of the panel row (the arrangement-lane idiom): the timeline
    ;; absorbs exactly the width left of the clip panel — without :width 0
    ;; :flex 1 an h-stack child lays out against an INFINITE max width.
    (timeline
      :width :fill
      :height :fill
      :focusable true
      :sidebar-width 5
      :sidebar-style :piano
      :header-height header-height
      :time-ruler (dict :mode :bars-beats :beats-per-bar 4)
      :item-color (current-track-color)
      :loop-color (current-track-color)
      :tool tool
      :playhead-time (bind-seq "piano-roll-playhead")
      :cursor-time cursor-time
      :lanes SEQ.piano-roll-lanes
      :items SEQ.piano-roll-items
      :selection SEQ.piano-roll-selection
      :selection-rect selection-rect
      :view-start piano-roll-view-start
      :view-duration piano-roll-view-duration
      :zoom-min-duration min-view-duration
      :zoom-max-duration max-view-duration
      :content-length (num-steps)
      :content-length-min 1
      :content-length-max 256
      ;; Loop-window gestures + overlay (clip-edit-target spec 5): only a
      ;; pinned clip has a window to slide or mark.
      :band-slide (= SEQ.focus-clip-kind :pattern)
      :window-marker SEQ.focus-window-marker
      :window-span SEQ.focus-window-span
      :window-repeat SEQ.focus-window-repeat
      :lane-scroll piano-roll-lane-scroll
      :lane-height (lane-height-value)
      :scroll-viewport-height eseq.seq-step-tabs/piano-roll-default-pane-height
      :snap 1
      :min-duration 0.03125
      :create-duration create-duration
      :move-snap-mode :alignment-helper
      :resize-snap :grid
      :snap-mode :floor
      :resize-snap-mode :alignment-helper
      :scroll-mode :smooth
      :on-action |event| (piano-roll-action event)))
  )

;; ── Clip panel (clip-edit-target spec 6) ───────────────────────────────────
;; Ableton-style numeric column left of the piano roll: source identity,
;; Start/End (beats), signed start offset, length, loop duality. Start/End/
;; Offset only exist for a pinned clip; in follow mode the column shows the
;; session identity, the live length and the loop row.
;;
;; Widget `:key`s auto-qualify inside a module (hazard a), so the hand-rolled
;; "piano-roll-" prefix is dropped: "panel-start" renders as
;; "eseq.piano-roll/panel-start".

(def clip-panel-width 24)

(def panel-row (name body)
  (h-stack :gap 0.3 :align :center
    (box :width 4.6 :height 1.0
      (label name :font-size 10 :color :dim :bg :transparent))
    body))

(def panel-static (name text key)
  (panel-row name
    (box :width 5 :height 1.0 :h-align :left
      (label text :key key :font-size 10 :color :white :bg :transparent))))

;; Signed display (spec 6): an offset in the top half of the pattern reads as
;; a negative pickup — offset L−1 shows as −1, exactly Ableton's start = −1
;; with the loop at 0.
(def signed-offset (offset)
  (let ((steps (num-steps)))
    (if (> offset (/ steps 2)) (- offset steps) offset)))

(def clip-panel-rows ()
  (if (= SEQ.focus-clip-start nil)
    (box :width 0 :height 0 :bg :transparent)
    (v-stack :gap 0.2
      (h-stack
        (panel-row "Start"
          (number-picker :key "panel-start"
            :value SEQ.focus-clip-start
            :min 0 :max 512 :decimals 0
            :background-color :buffer-bg
            :on-change (lambda (v)
              (if (= v SEQ.focus-clip-start)
                nil
                (host-command "focus-clip-resize"
                  (dict :track SEQ.current-track
                    :start-beat v :end-beat SEQ.focus-clip-end))))
            :width 5 :height 1.0 :font-size 8))
        (panel-row "End"
          (number-picker :key "panel-end"
            :value SEQ.focus-clip-end
            :min 0 :max 512 :decimals 0
            :background-color :buffer-bg
            :on-change (lambda (v)
              (if (= v SEQ.focus-clip-end)
                nil
                (host-command "focus-clip-resize"
                  (dict :track SEQ.current-track
                    :start-beat SEQ.focus-clip-start :end-beat v))))
            :width 5 :height 1.0 :font-size 8))
        )
        (panel-row "Offset"
          (number-picker :key "panel-offset"
            ;; Signed pickup display is a PATTERN-wrap idea (spec 6); a take's
            ;; offset clamps at 0 and never wraps, so it reads raw.
            :background-color :buffer-bg
            :value (if (= SEQ.focus-clip-kind :pattern)
              (signed-offset SEQ.focus-clip-offset)
              SEQ.focus-clip-offset)
            :min (if (= SEQ.focus-clip-kind :pattern) -256 0) :max 256 :decimals 0
            :on-change (lambda (v)
              (if (= v (if (= SEQ.focus-clip-kind :pattern)
                    (signed-offset SEQ.focus-clip-offset)
                    SEQ.focus-clip-offset))
                nil
                (host-command "focus-set-offset"
                  (dict :track SEQ.current-track :offset-steps v))))
            :width 5 :height 1.0 :font-size 8)))))

(def panel-length-row ()
  (panel-row "Length"
    (number-picker :key "panel-length"
      :noui true
      :value (num-steps)
      ;; A take's linear axis can outgrow one pattern (chunks); patterns and
      ;; the live path stay capped at MAX_STEPS.
      :min 1 :max (if (= SEQ.focus-kind :take) 4096 256) :decimals 0
      :on-change (lambda (v)
        (if (= v (num-steps))
          nil
          (if (= SEQ.focus-kind :take)
            ;; Coalesced take resize: grows mint silent chunks, shrinks keep
            ;; noted ones; one undo entry per drag.
            (host-command "focus-take-set-length"
              (dict :track SEQ.current-track :length v))
            (if (= SEQ.focus-kind :pattern)
              ;; Stage only: frames coalesce into one undo entry, sealed like a
              ;; device-knob gesture (the seal drains the deferred song-row
              ;; refresh). The loop bar's release event sends the seal itself.
              (host-command "focus-set-num-steps"
                (dict :track SEQ.current-track :length v))
              (do
                ;; eseq.seq-core-state, reached bare through its identity alias.
                (eseq.seq-core-state/cool-off-follow)
                (seq-set-track-param :num-steps v))))))
      :width 5 :height 1.0 :font-size 8)))

(def clip-panel ()
  (let ((color (current-track-color)))
    (box :height :fill
      (v-stack
        :height :fill
        :padding 0 :width clip-panel-width
        (box
          :height 1
          :width :fill
          :background-color (rgba (nth color 0) (nth color 1) (nth color 2) 1.0)
          (h-stack 
            :gap 0
            (box :width 1.0)
            (badge (substring (nth SEQ.track-names SEQ.current-track) 0 15)
              :key (str "pianoroll-label-content-" SEQ.current-track)
              :icon (eseq.track-collapse/type-icon SEQ.current-track)
              :width 10.8
              :height 1.0
              :padding 0
              :font-size 10
              :h-align :left
              :background-color :transparent
              :border-color :transparent
              :highlight-color :transparent
              :shadow-color :transparent
              :color :black
              :bg :transparent)
            )
          ;(label
          ;  (nth SEQ.track-names SEQ.current-track)
          ;  :font-size 10 :bg :transparent :color :black
          ;))
          )
        
        (box :width :fill :height 1.0
          :background-color :mixer-strip-selected-bg
          (h-stack (box :width 0.5)
            (label SEQ.focus-label
              :key "panel-source"
              :font-size 10 :color :white :bg :transparent
              )
            ))
        
        (box :padding 1  :height 5
          :background-color :mixer-strip-bg
          :width clip-panel-width
          (v-stack :gap 0.25
            
            (clip-panel-rows)
            (panel-length-row)
            ;; Informational in v1 (spec 6): states the pattern/take duality; layout
            ;; leaves room for Position/Length when sub-pattern windows land (5.1).
            (panel-static "Loop"
              (if (= SEQ.focus-kind :take) "off" "on")
              "panel-loop")))))))

(def buffer-content ()
  (if (and (piano-roll-arrangement-mode?) (= SEQ.focus-clip-start nil))
    (box
      :key "no-clip-selected"
      :width :fill :height :fill
      :h-align :center :v-align :center
      (label "No clip selected"
        :key "no-clip-selected-label"
        :font-size 11 :color :dim :bg :transparent))
    (h-stack :width :fill :gap 0.0 :height :fill
      (clip-panel)
      (piano-roll-timeline))))

(effect-buffer "*piano-roll*"
  (box :width :fill :height :fill
    (buffer-content)))
