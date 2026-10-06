;; A frozen view of the rolling 30-second live MIDI history. All time values
;; here are seconds. The crop is a start, a bar count and a whole-number BPM;
;; its end follows from those, so every crop is a loop Send can reproduce.
;; The capture itself is the host's `retro` singleton (lanes, notes, duration,
;; the audition's state); the crop and the roll's view are this view's own.
(module eseq.retrospective)
(import eseq.kinds :refer (retro transport))
(import eseq.view-kit :refer (nothing))
(export panel open close action apply-guess retro-crop retro-view)

;; The crop Send imports. `requested-bars` keeps the user's choice apart from
;; `bars`, so a marquee crop can undo an automatic double-time reading instead
;; of keeping an inflated bar count.
(def-kind retro-crop
  :key ()
  :state ((open false)
          (start 0)
          (end 1)
          (bars 1)
          (bpm 120)
          (requested-bars 1)))

;; The roll's scroll and zoom.
(def-kind retro-view
  :key ()
  :state ((start 0)
          (duration 30)
          (lane-scroll 0)
          (lane-height 1.7)))

(def loop-seconds () (/ (* 240 retro-crop.bars) retro-crop.bpm))
(def sync-end () (set! retro-crop.end (+ retro-crop.start (loop-seconds))))
;; Clamp a crop start into the capture.
(def capture-time (t) (clamp t 0 (max 0 (- retro.duration 0.001))))
;; The crop, as the audition and import commands take it.
(def crop () (dict :start retro-crop.start :end retro-crop.end :bars retro-crop.bars))

(def stop-loop ()
  (when retro.playing (host-command "retrospective-stop" (dict))))
(def audition () (host-command "retrospective-audition" (crop)))
;; Tuning while the preview plays restarts it on the new loop, so the tempo
;; can be adjusted by ear.
(def crop-changed ()
  (sync-end)
  (when retro.playing (audition)))

;; A free crop (marquee) picks the whole BPM nearest to its length, doubling
;; slow phrases, then snaps its end onto that tempo. A drag streams these, so
;; it stops the preview rather than restarting it on every event.
(def fit-crop (start end)
  (let ((duration (max 0.001 (- end start))))
    (stop-loop)
    (set! retro-crop.start start)
    (set! retro-crop.bars (seq-capture-bar-count duration retro-crop.requested-bars))
    (set! retro-crop.bpm (clamp (round (/ (* 240 retro-crop.bars) duration)) 70 999))
    (sync-end)))

;; Open with the crop fitted to start..end on a capture of `duration`
;; seconds. The host calls this right after presenting the capture, before
;; its next tick pushes `retro.duration`: the roll's zoom takes the passed
;; duration.
(def open (start end duration)
  (set! retro-crop.requested-bars 1)
  (fit-crop start end)
  (set! retro-view.start 0)
  (set! retro-view.duration (max 0.1 duration))
  (set! retro-view.lane-scroll 0)
  (set! retro-crop.open true))

;; Host-detected groove: its first hit, tempo and loop length in bars.
(def apply-guess (start tempo loop-bars)
  (set! retro-crop.start start)
  (set! retro-crop.bpm tempo)
  (set! retro-crop.bars loop-bars)
  (set! retro-crop.requested-bars loop-bars)
  (crop-changed))

(def close () (set! retro-crop.open false))
(def cancel () (host-command "retrospective-close" (dict)))
(def toggle-loop ()
  (if retro.playing (stop-loop) (audition)))

(def set-start (value)
  (set! retro-crop.start (capture-time value))
  (crop-changed))
(def set-bars (value)
  (set! retro-crop.requested-bars (clamp (round value) 1 16))
  (set! retro-crop.bars retro-crop.requested-bars)
  (crop-changed))
(def set-bpm (value)
  (set! retro-crop.bpm (clamp (round value) 70 999))
  (crop-changed))

(def marquee (event)
  (when (> (- event.time-b event.time-a) 0.001)
    (fit-crop (capture-time event.time-a) event.time-b)))

(def action (event)
  (match event.type
    :marquee-select (marquee event)
    :finish-marquee-select (marquee event)
    :scroll-view
    (do
      (unless (= event.view-start nil)
        (set! retro-view.start
          (clamp event.view-start 0 (max 0 (- retro.duration retro-view.duration)))))
      (unless (= event.lane-scroll nil)
        (set! retro-view.lane-scroll (max 0 event.lane-scroll))))
    :zoom-view
    (let ((ratio (/ (- event.anchor-time retro-view.start) retro-view.duration))
          (duration (clamp (/ retro-view.duration event.factor) 0.1 (max 0.1 retro.duration))))
      (set! retro-view.duration duration)
      (set! retro-view.start
        (clamp (- event.anchor-time (* ratio duration)) 0 (max 0 (- retro.duration duration)))))
    :zoom-lanes
    (set! retro-view.lane-height (clamp (* retro-view.lane-height event.factor) 0.5 4))))

(def send () (host-command "retrospective-import" (crop)))

;; The timeline takes its lanes and notes as rows. Scroll and zoom re-render
;; the panel, so the rows are kept with the capture lists they were built
;; from and rebuilt only when `retro.lanes` / `retro.items` change (a dense
;; capture holds 8192 notes).
(def-kind roll-rows
  :key ()
  :state ((lanes-of :any :default nil)
          (lanes :any :default nil)
          (items-of :any :default nil)
          (items :any :default nil)))

(def roll-lanes ()
  (let ((source retro.lanes))
    (unless (= roll-rows.lanes-of source)
      (set! roll-rows.lanes (map (lambda (l) (dict :id l.index :label l.label)) source))
      (set! roll-rows.lanes-of source))
    roll-rows.lanes))
(def roll-items ()
  (let ((source retro.items))
    (unless (= roll-rows.items-of source)
      (set! roll-rows.items
        (map (lambda (n) (dict :id n.index :lane n.lane.index :start n.start :end n.end))
          source))
      (set! roll-rows.items-of source))
    roll-rows.items))

(def note (text &rest props)
  (apply label text :bg :transparent :font-size 11 props))
(def seconds (t) (/ (round (* 1000 t)) 1000))

(def loop-info ()
  (if transport.playing
    "Stop the song to preview the loop."
    (str "Loop " (seconds (loop-seconds)) " s, ends at " (seconds retro-crop.end) " s"
         (if (> retro-crop.end retro.duration) " (past the capture: rest)" ""))))

(def panel ()
  (modal :key "retrospective-modal" :is-open retro-crop.open :on-close (lambda () (cancel))
    :title "Capture MIDI" :width-px 1120 :height-px 720
    (if retro-crop.open
    (let ((empty (= (len retro.items) 0)))
    (v-stack :width :fill :height :fill :gap 0.6
      ;; Loop preview sits above the roll in a raised pill, like the
      ;; transport's playback controls.
      (h-stack :width :fill :gap 0.8 :align :center
        (box :key "retrospective-playback" :background-color :mixer-control-bg :corner-radius 72
          :padding 0.015 :height 1.4
          (h-stack :gap 0.2 :align :center
            (box :key "retrospective-loop-stop" :width 2.5
              :on-click |x y r| (stop-loop)
              (stop-icon))
            (box :key "retrospective-audition" :width 2.5
              :on-click |x y r|
              (unless (or transport.playing empty) (toggle-loop))
              (play-icon :active #'retro.playing))))
        (note (loop-info) :key "retrospective-loop-info" :color :dim :flex 1))
      (if empty
        (label "Play an armed track or drum rack, then reopen Capture MIDI." :bg :transparent :font-size 12)
        (nothing))
      (box :key "retrospective-roll-container" :width :fill :flex 1 :height 0
        (timeline :key "retrospective-roll" :width :fill :height :fill
          :focusable true :tool :marquee :sidebar-width 14 :header-height 1.5
          :time-ruler (dict :mode :seconds) :grid-density 2
          :lanes (roll-lanes) :items (roll-items)
          :lane-height retro-view.lane-height :lane-scroll retro-view.lane-scroll
          :view-start retro-view.start :view-duration retro-view.duration
          :zoom-min-duration 0.1 :zoom-max-duration 30 :snap 0 :loop-visible false
          :playhead-time #'retro.playhead
          :selection-rect (dict :time-a retro-crop.start :time-b retro-crop.end
                               :lane-a 0 :lane-b (max 0 (- (len retro.lanes) 1)))
          :selection-rect-style :marquee :scroll-mode :smooth
          :on-action |event| (action event)))
      (h-stack :width :fill :gap 0.8 :align :center
        (note "Start (s)")
        (number-picker :key "retrospective-start" :value retro-crop.start
          :min 0 :max retro.duration :step 0.01 :decimals 3 :width 9
          :on-change |value| (set-start value))
        (note "Bars")
        (number-picker :key "retrospective-bars" :value retro-crop.bars
          :min 1 :max 16 :step 1 :decimals 0 :width 6
          :on-change |value| (set-bars value))
        (note "BPM")
        (number-picker :key "retrospective-bpm" :value retro-crop.bpm
          :min 70 :max 240 :step 1 :decimals 0 :width 7
          :on-change |value| (set-bpm value))
        (button "Detect" :key "retrospective-detect" :variant :ghost
          :disabled empty
          :on-click |event| (host-command "retrospective-detect" (dict))))
      (h-stack :width :fill :gap 0.8 :align :center
        (box :height 0 :flex 1)
        (button "Cancel" :variant :ghost :on-click |event| (cancel))
        (if transport.playing
          (button "Stop playback" :key "retrospective-stop"
            :on-click |event| (set! transport.playing false))
          (button "Send to tracks" :key "retrospective-send" :disabled empty
            :on-click |event| (send))))
      (if retro.truncated
        (note "Capture was very dense; only the most recent 8192 trigs are available.")
        (nothing))
      (if (= retro.error "")
        (nothing)
        (note retro.error :key "retrospective-error" :color :red))))
    (nothing))))
