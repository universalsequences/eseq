;; A frozen view of the rolling 30-second live MIDI history. All time values
;; here are seconds. Only Send converts them to the chosen number of bars.
(module eseq.retrospective)
(export panel open close action crop-start crop-end bars open?)

(defstate open? false)
(defstate crop-start 0)
(defstate crop-end 1)
(defstate bars 1)
;; Keep the user's choice separate so shrinking a crop can undo automatic
;; double-time interpretation instead of retaining an inflated bar count.
(defstate requested-bars 1)
(defstate view-start 0)
(defstate view-duration 30)
(defstate lane-scroll 0)
(defstate lane-height 1.7)

(def update-bars ()
  (set! bars (seq-capture-bar-count (max 0.001 (- crop-end crop-start)) requested-bars)))

(def open (start end)
  (set! crop-start start)
  (set! crop-end end)
  (set! requested-bars 1)
  (update-bars)
  (set! view-start 0)
  (set! view-duration (max 0.1 RETRO.duration))
  (set! lane-scroll 0)
  (set! open? true))

(def close () (set! open? false))
(def cancel () (host-command "retrospective-close" (dict)))
(def stop-loop ()
  (if RETRO.playing
    (host-command "retrospective-stop" (dict)) nil))

(def set-start (value)
  (stop-loop)
  (set! crop-start (max 0 (min (- crop-end 0.001) value)))
  (update-bars))
(def set-end (value)
  (stop-loop)
  (set! crop-end (min RETRO.duration (max (+ crop-start 0.001) value)))
  (update-bars))

(def action (event)
  (match event.type
    :marquee-select
    (if (> (- event.time-b event.time-a) 0.001)
      (do
        (set! crop-start (max 0 (min (- RETRO.duration 0.001) event.time-a)))
        (set-end event.time-b)) nil)
    :finish-marquee-select
    (if (> (- event.time-b event.time-a) 0.001)
      (do
        (set! crop-start (max 0 (min (- RETRO.duration 0.001) event.time-a)))
        (set-end event.time-b)) nil)
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
        (button "Refresh capture" :variant :ghost
          :on-click |event| (host-command "retrospective-open" (dict))))
      (label "Crop a phrase, choose its bar count, and loop it. Editing the crop stops the preview."
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
          :min 0 :max crop-end :step 0.01 :decimals 3 :width 9
          :on-change |value| (set-start value))
        (label "End (s)" :bg :transparent :font-size 11)
        (number-picker :key "retrospective-end" :value crop-end
          :min crop-start :max RETRO.duration :step 0.01 :decimals 3 :width 9
          :on-change |value| (set-end value))
        (button "Zoom to crop" :variant :ghost :on-click |event|
          (do (set! view-start crop-start) (set! view-duration (max 0.1 (- crop-end crop-start)))))
        (button "Show all" :variant :ghost :on-click |event|
          (do (set! view-start 0) (set! view-duration (max 0.1 RETRO.duration)))))
      (h-stack :width :fill :gap 0.8 :align :center
        (label "Bars" :bg :transparent :font-size 11)
        (number-picker :key "retrospective-bars" :value bars :min 1 :max 16 :step 1 :decimals 0 :width 6
          :on-change |value| (do (stop-loop) (set! requested-bars (round value)) (update-bars)))
        (label (str (round (/ (* 240 bars) (max 0.001 (- crop-end crop-start)))) " BPM on send (rounded)")
          :bg :transparent :font-size 11 :color :dim :flex 1)
        (button "Cancel" :variant :ghost :on-click |event| (cancel))
        (button (if RETRO.playing "Stop loop" "Loop crop") :key "retrospective-audition"
          :disabled (or SEQ.playing (= (len RETRO.items) 0))
          :on-click |event|
          (if RETRO.playing (stop-loop)
            (host-command "retrospective-audition" (dict :start crop-start :end crop-end :bars bars))))
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
