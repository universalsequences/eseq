;; Resample: SP-404 style "print the last 30 seconds". The master output is
;; always recorded into a ring; opening this modal freezes that ring into a
;; waveform (src/ui/host_commands/resample.rs). Crop it with the start/end
;; handles, loop it through the preview voice, name and tag it, and Add puts
;; it in the sample library and on a new sampler track. All times are seconds.
;;
;; Mounted by both step-panel buffers, like Capture MIDI; Rust activates the
;; mount's tile before calling `open`.
(module eseq.resample)
(import eseq.sample-import)
(export panel open close open? crop-start crop-end name tags add-tag commit)

(defstate open? false)
(defstate crop-start 0)
(defstate crop-end 1)
(defstate view-start 0)
(defstate view-duration 30)
;; "none" | "start" | "end": the handle being dragged, if any.
(defstate active-marker "none")
(defstate name "")
(defstate tags (list))
(defstate tag-draft "")

(def duration () (max 0.001 RESAMPLE.duration))
(def playing? () SEQ.browser-preview-playing)

(def stop-loop ()
  (if (playing?) (host-command "stop-sample-preview" (dict)) nil))
(def audition ()
  (host-command "resample-audition" (dict :start crop-start :end crop-end)))
(def toggle-loop ()
  (if (playing?) (stop-loop) (audition)))
;; A playing loop follows the crop so it can be trimmed by ear, but only once
;; a handle drag lands: restarting on every drag event would stutter.
(def crop-changed ()
  (if (and (playing?) (= active-marker "none")) (audition) nil))

(def set-crop (start end)
  (let ((a (max 0 (min (duration) (min start end))))
        (b (max 0 (min (duration) (max start end)))))
    (if (> (- b a) 0.001)
      (do (set! crop-start a) (set! crop-end b) (crop-changed))
      nil)))

(def open (start end default-name default-tags)
  (set! crop-start start)
  (set! crop-end (max end (+ start 0.001)))
  (set! view-start 0)
  (set! view-duration (duration))
  (set! active-marker "none")
  (set! name default-name)
  (set! tags default-tags)
  (set! tag-draft "")
  (set! open? true))

(def close ()
  (set! open? false)
  (set! tag-draft ""))
(def cancel () (host-command "resample-close" (dict)))

(def has-tag? (tag)
  (> (len (filter |existing| (= (string-downcase existing) (string-downcase tag)) tags)) 0))
(def add-tag (tag)
  (if (or (= tag "") (has-tag? tag)) (set! tag-draft "")
    (do (set! tags (append tags (list tag))) (set! tag-draft ""))))
(def remove-tag (tag)
  (set! tags (filter |existing| (not (= existing tag)) tags)))

;; A tag typed but not yet entered still counts.
(def commit ()
  (add-tag tag-draft)
  (host-command "resample-commit"
    (dict :start crop-start :end crop-end :name name :tags tags)))

(def clamp-view-start (start)
  (max 0 (min (max 0 (- (duration) view-duration)) start)))

(def action (event)
  (match event.type
    :set-selection
    (set-crop event.start event.end)
    :begin-marker-drag
    (set! active-marker (if (= event.marker :start) "start" "end"))
    :end-marker-drag
    (do (set! active-marker "none") (crop-changed))
    :clear-selection
    (set-crop 0 (duration))
    :scroll-view
    (set! view-start (clamp-view-start (+ view-start event.delta-time)))
    :zoom-view
    (let ((ratio (/ (- event.anchor-time view-start) view-duration))
          (next (max 0.05 (min (duration) (/ view-duration event.factor)))))
      (set! view-duration next)
      (set! view-start (clamp-view-start (- event.anchor-time (* ratio next)))))
    _
    nil))

(def seconds-text (value) (str (/ (round (* 1000 value)) 1000)))

(def panel ()
  (modal :key "resample-modal" :is-open open? :on-close (lambda () (cancel))
    :title "Resample" :width-px 1120 :height-px 760
    (if open?
    (v-stack :width :fill :height :fill :gap 0.6
      (h-stack :width :fill :gap 1 :align :center
        (label "Your last 30 seconds" :bg :transparent :font-size 18)
        (box :height 0 :flex 1)
        (button "Zoom to crop" :key "resample-zoom-crop" :variant :ghost :on-click |event|
          (do (set! view-duration (max 0.05 (- crop-end crop-start)))
              (set! view-start (clamp-view-start crop-start))))
        (button "Show all" :key "resample-show-all" :variant :ghost :on-click |event|
          (do (set! view-start 0) (set! view-duration (duration))))
        (button "Print again" :key "resample-reprint" :variant :ghost
          :on-click |event| (host-command "resample-open" (dict))))
      (label "Drag the start and end handles to crop. Scroll or pinch to zoom."
        :bg :transparent :font-size 11 :color :dim)
      (box :key "resample-wave-container" :width :fill :height 6
        :background-color :instrument-control-bg :corner-radius 10 :padding 0.3
        (if RESAMPLE.buffer
          (subtree :key (str "resample-wave-" (get RESAMPLE.buffer :registry-key))
            (waveform :key "resample-wave" :width :fill :height 5.4
              :header-height 1.2 :ruler-font-size 8 :ruler-color :dim :ruler-bg :black
              :grid-major-color :black :grid-minor-color :black
              :bg :instrument-control-bg :focusable true
              :marker-selection true :active-marker active-marker
              :marker-color :dim :active-marker-color :widget-knob-filled
              :waveform-color :yellow :inactive-waveform-color '(rgba 0.25 0.25 0.25 1)
              :buffer RESAMPLE.buffer
              :view-start view-start :view-duration view-duration
              :zoom-min-duration 0.05 :zoom-max-duration (duration)
              :selection-start crop-start :selection-end crop-end
              :playhead-time (if (playing?) (bind-seq "browser-preview-playhead") -1)
              :time-ruler (dict :mode :seconds)
              :on-action |event| (action event)))
          (label "Nothing captured" :bg :transparent :font-size 12 :color :dim)))
      (h-stack :width :fill :gap 0.8 :align :center
        (box :key "resample-playback" :background-color :mixer-strip-bg :corner-radius 72
          :padding 0.015 :height 1.4
          (h-stack :gap 0.2 :align :center
            (box :key "resample-loop-stop" :width 2.5 :on-click |x y r| (stop-loop)
              (stop-icon))
            (box :key "resample-audition" :width 2.5 :on-click |x y r| (toggle-loop)
              (play-icon :active (if (playing?) 1 0)))))
        (label "Start (s)" :bg :transparent :font-size 11)
        (number-picker :key "resample-start" :value crop-start
          :min 0 :max (duration) :step 0.01 :decimals 3 :width 9
          :on-change |value| (set-crop value crop-end))
        (label "End (s)" :bg :transparent :font-size 11)
        (number-picker :key "resample-end" :value crop-end
          :min 0 :max (duration) :step 0.01 :decimals 3 :width 9
          :on-change |value| (set-crop crop-start value))
        (label (str "Loop " (seconds-text (- crop-end crop-start)) " s")
          :key "resample-length" :bg :transparent :font-size 11 :color :dim :flex 1))
      (h-stack :width :fill :gap 1 :align :start
        (v-stack :width 0 :flex 1 :gap 0.3
          (label "Name" :bg :transparent :font-size 11)
          (text-input :key "resample-name" :width :fill :height 1.35 :font-size 11
            :value name :placeholder "sample name"
            :on-change (lambda (v) (set! name v))))
        (v-stack :width 0 :flex 1 :gap 0.3
          (label "Tags" :bg :transparent :font-size 11)
          (eseq.sample-import/chip-row "resample-tags" tags (lambda (tag) (remove-tag tag)))
          (eseq.sample-import/tag-entry "resample-tag" tag-draft
            (lambda (v) (set! tag-draft v))
            tags
            (lambda (tag) (add-tag tag))
            "add a tag, Enter to apply")))
      (label "Add saves the crop to the sample library and loads it on a new sampler track."
        :bg :transparent :font-size 10 :color :dim)
      (h-stack :width :fill :gap 0.8 :align :center
        (box :height 0 :flex 1)
        (button "Cancel" :key "resample-cancel" :variant :ghost :on-click |event| (cancel))
        (button "Add as sampler track" :key "resample-commit" :on-click |event| (commit)))
      (label RESAMPLE.error :key "resample-error" :bg :transparent :font-size 11 :color :red))
    (box :height 0 :width 0))))
