;; A frozen view of the rolling 30-second live MIDI history. All time values
;; here are seconds. The crop is a start, a bar count and a whole-number BPM;
;; its end follows from those, so every crop is a loop Send can reproduce.
(module eseq.retrospective)
(export panel open close action apply-guess crop-start crop-end bars bpm open?)

(defstate open? false)
(defstate crop-start 0)
(defstate crop-end 1)
(defstate bars 1)
(defstate bpm 120)
;; Keep the user's choice separate so a marquee crop can undo automatic
;; double-time interpretation instead of retaining an inflated bar count.
(defstate requested-bars 1)
(defstate view-start 0)
(defstate view-duration 30)
(defstate lane-scroll 0)
(defstate lane-height 1.7)

(def loop-seconds () (/ (* 240 bars) bpm))
(def sync-end () (set! crop-end (+ crop-start (loop-seconds))))

(def stop-loop ()
  (if RETRO.playing
    (host-command "retrospective-stop" (dict)) nil))
(def audition ()
  (host-command "retrospective-audition" (dict :start crop-start :end crop-end :bars bars)))
;; Tuning while the preview plays restarts it on the new loop, so the tempo
;; can be adjusted by ear.
(def crop-changed ()
  (sync-end)
  (if RETRO.playing (audition) nil))

;; A free crop (marquee) picks the whole BPM nearest to its length, doubling
;; slow phrases, then snaps its end onto that tempo. A drag streams these, so
;; it stops the preview rather than restarting it on every event.
(def fit-crop (start end)
  (let ((duration (max 0.001 (- end start))))
    (stop-loop)
    (set! crop-start start)
    (set! bars (seq-capture-bar-count duration requested-bars))
    (set! bpm (max 70 (min 999 (round (/ (* 240 bars) duration)))))
    (sync-end)))

(def open (start end)
  (set! requested-bars 1)
  (fit-crop start end)
  (set! view-start 0)
  (set! view-duration (max 0.1 RETRO.duration))
  (set! lane-scroll 0)
  (set! open? true))

;; Host-detected groove: its first hit, tempo and loop length in bars.
(def apply-guess (start tempo loop-bars)
  (set! crop-start start)
  (set! bpm tempo)
  (set! bars loop-bars)
  (set! requested-bars loop-bars)
  (crop-changed))

(def close () (set! open? false))
(def cancel () (host-command "retrospective-close" (dict)))
(def toggle-loop ()
  (if RETRO.playing (stop-loop) (audition)))

(def set-start (value)
  (set! crop-start (max 0 (min (- RETRO.duration 0.001) value)))
  (crop-changed))
(def set-bars (value)
  (set! requested-bars (max 1 (min 16 (round value))))
  (set! bars requested-bars)
  (crop-changed))
(def set-bpm (value)
  (set! bpm (max 70 (min 999 (round value))))
  (crop-changed))

(def action (event)
  (match event.type
    :marquee-select
    (if (> (- event.time-b event.time-a) 0.001)
      (fit-crop (max 0 (min (- RETRO.duration 0.001) event.time-a)) event.time-b) nil)
    :finish-marquee-select
    (if (> (- event.time-b event.time-a) 0.001)
      (fit-crop (max 0 (min (- RETRO.duration 0.001) event.time-a)) event.time-b) nil)
    :scroll-view
    (do
      (if (= event.view-start nil) nil
        (set! view-start (max 0 (min (max 0 (- RETRO.duration view-duration)) event.view-start))))
      (if (= event.lane-scroll nil) nil
        (set! lane-scroll (max 0 event.lane-scroll))))
    :zoom-view
    (let ((ratio (/ (- event.anchor-time view-start) view-duration))
          (duration (max 0.1 (min RETRO.duration (/ view-duration event.factor)))))
      (set! view-duration duration)
      (set! view-start (max 0 (min (- RETRO.duration duration) (- event.anchor-time (* ratio duration))))))
    :zoom-lanes
    (set! lane-height (max 0.5 (min 4 (* lane-height event.factor))))))

(def send ()
  (host-command "retrospective-import"
    (dict :start crop-start :end crop-end :bars bars)))

(def panel ()
  (modal :key "retrospective-modal" :is-open open? :on-close (lambda () (cancel))
    :title "Capture MIDI" :width-px 1120 :height-px 720
    (if open?
    (v-stack :width :fill :height :fill :gap 0.6
      (h-stack :width :fill :gap 1 :align :center
        (label "Your last 30 seconds" :bg :transparent :font-size 18 :flex 1)
        (button "Zoom to crop" :variant :ghost :on-click |event|
          (do (set! view-start crop-start) (set! view-duration (max 0.1 (- crop-end crop-start)))))
        (button "Show all" :variant :ghost :on-click |event|
          (do (set! view-start 0) (set! view-duration (max 0.1 RETRO.duration))))
        (button "Refresh capture" :variant :ghost
          :on-click |event| (host-command "retrospective-open" (dict))))
      (label "Detect finds the repeating groove. Drag in the roll to crop by hand; the end snaps to a whole BPM."
        :bg :transparent :font-size 11 :color :dim)
      (if (= (len RETRO.items) 0)
        (label "Play an armed track or drum rack, then refresh the capture." :bg :transparent :font-size 12)
        (box :height 0 :width 0))
      (box :key "retrospective-roll-container" :width :fill :flex 1 :height 0
        (timeline :key "retrospective-roll" :width :fill :height :fill
          :focusable true :tool :marquee :sidebar-width 14 :header-height 1.5
          :time-ruler (dict :mode :seconds) :grid-density 2
          :lanes RETRO.lanes :items RETRO.items :lane-height lane-height :lane-scroll lane-scroll
          :view-start view-start :view-duration view-duration
          :zoom-min-duration 0.1 :zoom-max-duration 30 :snap 0 :loop-visible false
          :playhead-time (if RETRO.playing (+ crop-start (* RETRO.position (- crop-end crop-start))) -1)
          :selection-rect (dict :time-a crop-start :time-b crop-end
                               :lane-a 0 :lane-b (max 0 (- (len RETRO.lanes) 1)))
          :selection-rect-style :marquee :scroll-mode :smooth
          :on-action |event| (action event)))
      (h-stack :width :fill :gap 0.8 :align :center
        (label "Start (s)" :bg :transparent :font-size 11)
        (number-picker :key "retrospective-start" :value crop-start
          :min 0 :max RETRO.duration :step 0.01 :decimals 3 :width 9
          :on-change |value| (set-start value))
        (label "Bars" :bg :transparent :font-size 11)
        (number-picker :key "retrospective-bars" :value bars :min 1 :max 16 :step 1 :decimals 0 :width 6
          :on-change |value| (set-bars value))
        (label "BPM" :bg :transparent :font-size 11)
        (number-picker :key "retrospective-bpm" :value bpm :min 70 :max 240 :step 1 :decimals 0 :width 7
          :on-change |value| (set-bpm value))
        (button "Detect" :key "retrospective-detect" :variant :ghost
          :disabled (= (len RETRO.items) 0)
          :on-click |event| (host-command "retrospective-detect" (dict)))
        (box :height 0 :flex 1))
      (h-stack :width :fill :gap 0.8 :align :center
        ;; Same pill and icons as the transport's playback controls.
        (box :key "retrospective-playback" :background-color :mixer-strip-bg :corner-radius 72
          :padding 0.015 :height 1.4
          (h-stack :gap 0.2 :align :center
            (box :key "retrospective-loop-stop" :width 2.5
              :on-click |x y r| (stop-loop)
              (stop-icon))
            (box :key "retrospective-audition" :width 2.5
              :on-click |x y r|
              (if (or SEQ.playing (= (len RETRO.items) 0)) nil (toggle-loop))
              (play-icon :active (if RETRO.playing 1 0)))))
        (label (if SEQ.playing "Stop the song to preview the loop."
                 (str "Loop " (/ (round (* 1000 (loop-seconds))) 1000) " s, ends at "
                      (/ (round (* 1000 crop-end)) 1000) " s"
                      (if (> crop-end RETRO.duration) " (past the capture: rest)" "")))
          :key "retrospective-loop-info" :bg :transparent :font-size 11 :color :dim :flex 1)
        (button "Cancel" :variant :ghost :on-click |event| (cancel))
        (if SEQ.playing
          (button "Stop playback" :key "retrospective-stop" :on-click |event| (seq-toggle-play))
          (button "Send to tracks" :key "retrospective-send" :disabled (= (len RETRO.items) 0)
            :on-click |event| (send))))
      (if RETRO.truncated
        (label "Capture was very dense; only the most recent 8192 trigs are available." :bg :transparent :font-size 11)
        (box :height 0 :width 0))
      (label "Send creates new patterns and sets the project BPM. Undo restores the patterns and tempo together."
        :bg :transparent :font-size 10 :color :dim)
      (label RETRO.error :key "retrospective-error" :bg :transparent :font-size 11 :color :red))
    (box :height 0 :width 0))))
