;; Process Channels UI demo: a regular ESeqLisp buffer that drives process inlets.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "crates/sequencer/scripts/processes/process-ui-control-demo.lisp")
;;
;; Start the transport and put a few notes on any melodic track. The panel writes
;; process inlets from normal UI callbacks; the scheduler picks those values up
;; through the published process authoring snapshot.
;;
;; Current v1 limitation: the UI can write process inlets, but process outlets and
;; channels are not yet reactive UI bindings. The event-view below visualizes the
;; marker notes the process emits through the normal track-event telemetry.

(def process-ui-value
  (defchan process-ui-value 0))

(def-process process-ui-bounce
  :in ((step :float 0 12 :default 1)
       (range :float 0 24 :default 7)
       (period :float 0.25 8 :default 1)
       (track :int 0 15 :default 0)
       (marker-every :int 1 16 :default 4)
       (enabled :bool :default true)
       (markers :bool :default true))
  :out ((value :float))
  :state ((x 0)
          (dir 1)
          (tick 0)
          (marker 0))
  :every (in :period)
  :run (if (in :enabled)
         (do
           (set! tick (+ tick 1))
           (set! marker (+ marker 1))
           (set! x (+ x (* dir (in :step))))

           (if (> x (in :range))
             (do
               (set! x (in :range))
               (set! dir -1))
             nil)

           (if (< x (- 0 (in :range)))
             (do
               (set! x (- 0 (in :range)))
               (set! dir 1))
             nil)

           (transpose! x)
           (out :value x)
           (send :process-ui-value x)

           (if (and (in :markers) (>= marker (in :marker-every)))
             (do
               (set! marker 0)
               (emit :track (in :track) :note 0 :vel 0.55 :duration 0.25))
             nil))
         (do
           (transpose! 0)
           (out :value x)
           (send :process-ui-value x))))

(defstate process-ui-step 1)
(defstate process-ui-range 7)
(defstate process-ui-period 1)
(defstate process-ui-marker-every 4)
(defstate process-ui-enabled true)
(defstate process-ui-markers true)
(defstate process-ui-track-label "Track 1")

(def process-ui-track-options
  (list "Track 1" "Track 2" "Track 3" "Track 4"
        "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12"
        "Track 13" "Track 14" "Track 15" "Track 16"))

(def process-ui-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

(def process-ui-track-index (label)
  (process-ui-index-of process-ui-track-options label))

(def process-ui-set-step (v)
  (do
    (set! process-ui-step v)
    (process-ui-wander :step v)))

(def process-ui-set-range (v)
  (do
    (set! process-ui-range v)
    (process-ui-wander :range v)))

(def process-ui-set-period (v)
  (do
    (set! process-ui-period v)
    (process-ui-wander :period v)))

(def process-ui-set-marker-every (v)
  (do
    (set! process-ui-marker-every v)
    (process-ui-wander :marker-every v)))

(def process-ui-set-track (label)
  (do
    (set! process-ui-track-label label)
    (process-ui-wander :track (process-ui-track-index label))))

(def process-ui-set-enabled (v)
  (do
    (set! process-ui-enabled v)
    (process-ui-wander :enabled v)))

(def process-ui-set-markers (v)
  (do
    (set! process-ui-markers v)
    (process-ui-wander :markers v)))

(def process-ui-wander
  (process-ui-bounce
    :step process-ui-step
    :range process-ui-range
    :period process-ui-period
    :track (process-ui-track-index process-ui-track-label)
    :marker-every process-ui-marker-every
    :enabled process-ui-enabled
    :markers process-ui-markers))

(start process-ui-wander)

(def script-buffer-name "*process-ui*")
(def script-tab-label "Process UI")
(def script-sequencer-name "")
(def script-init-fn ()
  (do
    (start process-ui-wander)
    true))

(def process-ui-row-height 1.0)
(def process-ui-label-width 6.0)
(def process-ui-control-width 7.0)

(def process-ui-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value
    :min lo
    :max hi
    :step stp
    :decimals dec
    :width process-ui-control-width
    :height process-ui-row-height
    :font-size 9
    :on-change on-change))

(def process-ui-toggle (key value on-change)
  (box
    :width process-ui-control-width
    :height process-ui-row-height
    :padding 0
    :h-align :center
    :v-align :center
    (toggle
      :key key
      :value value
      :color "#4f7dff"
      :off-color "#5f687a"
      :knob-color "#e8ecf4"
      :off-knob-color "#d8dde8"
      :on-change on-change)))

(def process-ui-row (text control)
  (h-stack :gap 0.5 :align :center
    (label text :width process-ui-label-width :height process-ui-row-height :font-size 9 :h-align :right :color :dim :bg :transparent)
    control))

(def process-ui-panel (track-events track-event-current-beat track-colors)
  (box
    :padding 0.85
    :gap 0.6
    :width 56
    :height 18
    (h-stack :gap 0.8 :align :top
      (box :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 0.75 :corner-radius 16
        (v-stack :gap 0.45
          (label "process wander" :width 16 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
          (process-ui-row "step"
            (process-ui-num "process-ui-step" process-ui-step 0 12 0.25 2
              (lambda (v) (process-ui-set-step v))))
          (process-ui-row "range"
            (process-ui-num "process-ui-range" process-ui-range 0 24 1 0
              (lambda (v) (process-ui-set-range v))))
          (process-ui-row "period"
            (process-ui-num "process-ui-period" process-ui-period 0.25 8 0.25 2
              (lambda (v) (process-ui-set-period v))))
          (process-ui-row "marker"
            (process-ui-num "process-ui-marker-every" process-ui-marker-every 1 16 1 0
              (lambda (v) (process-ui-set-marker-every v))))
          (process-ui-row "track"
            (dropdown
              :key "process-ui-track"
              :value process-ui-track-label
              :options process-ui-track-options
              :width process-ui-control-width
              :height process-ui-row-height
              :font-size 8
              :on-change (lambda (v) (process-ui-set-track v))))
          (process-ui-row "run"
            (process-ui-toggle "process-ui-enabled" process-ui-enabled
              (lambda (v) (process-ui-set-enabled v))))
          (process-ui-row "emit"
            (process-ui-toggle "process-ui-markers" process-ui-markers
              (lambda (v) (process-ui-set-markers v))))
          (h-stack :gap 0.5 :align :center
            (button "start" :key "process-ui-start" :width 5.0 :height 1.2 :font-size 9
              :on-click (lambda (event) (start process-ui-wander)))
            (button "stop" :key "process-ui-stop" :width 5.0 :height 1.2 :font-size 9
              :on-click (lambda (event) (stop process-ui-wander))))))

      (box :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 0.75 :corner-radius 16
        (v-stack :gap 0.45
          (label "track events" :width 30 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
          (event-view
            :key "process-ui-track-event-view"
            :events track-events
            :current-beat track-event-current-beat
            :renderer :heatmap
            :x :beat-phase
            :x-min 0
            :x-max 16
            :y :transpose
            :y-min -24
            :y-max 24
            :phase-beats 16
            :window-beats 16
            :brightness :velocity
            :color-by :track
            :color-mode :categorical
            :color-palette track-colors
            :color-min 0
            :color-max 15
            :color-count 16
            :x-bins 64
            :y-bins 48
            :background (rgba 0.1 0.1 0.1 0.5)
            :width 30
            :height 12))))))

(effect-buffer "*process-ui*"
  (process-ui-panel SEQ.track-events SEQ.track-event-current-beat SEQ.track-colors))

(seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")

(ps)
