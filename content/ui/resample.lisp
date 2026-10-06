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
(import eseq.kinds :refer (browser))
(export resample-view panel open close show-error add-tag commit action)

;; The modal: the frozen print the host hands `open` (its waveform-buffer
;; map and duration), the crop, the waveform's scroll and zoom, the handle
;; being dragged ("none", "start" or "end"), the name and tags, and the last
;; error a resample command reported.
(def-kind resample-view
  :key ()
  :state ((open false)
          (buffer :any :default false)
          (duration 0)
          (crop-start 0)
          (crop-end 1)
          (view-start 0)
          (view-duration 30)
          (marker "none")
          (name "")
          (tags '())
          (tag-draft "")
          (error "")))

(def duration () (max 0.001 resample-view.duration))
(def playing? () browser.preview-playing)

(def stop-loop ()
  (if (playing?) (host-command "stop-sample-preview" (dict)) nil))
(def audition ()
  (host-command "resample-audition"
    (dict :start resample-view.crop-start :end resample-view.crop-end)))
(def toggle-loop ()
  (if (playing?) (stop-loop) (audition)))
;; A playing loop follows the crop so it can be trimmed by ear, but only once
;; a handle drag lands: restarting on every drag event would stutter.
(def crop-changed ()
  (if (and (playing?) (= resample-view.marker "none")) (audition) nil))

(def set-crop (start end)
  (let ((a (max 0 (min (duration) (min start end))))
        (b (max 0 (min (duration) (max start end)))))
    (if (> (- b a) 0.001)
      (do (set! resample-view.crop-start a) (set! resample-view.crop-end b) (crop-changed))
      nil)))

;; The host opens the modal on a fresh print (its waveform-buffer map) of
;; `seconds`, with the crop it seeds (the audible span) and the default name
;; and tags.
(def open (start end default-name default-tags print seconds)
  (set! resample-view.buffer print)
  (set! resample-view.duration seconds)
  (set! resample-view.error "")
  (set! resample-view.crop-start start)
  (set! resample-view.crop-end (max end (+ start 0.001)))
  (set! resample-view.view-start 0)
  (set! resample-view.view-duration (duration))
  (set! resample-view.marker "none")
  (set! resample-view.name default-name)
  (set! resample-view.tags default-tags)
  (set! resample-view.tag-draft "")
  (set! resample-view.open true))

(def close ()
  (set! resample-view.open false)
  (set! resample-view.buffer false)
  (set! resample-view.tag-draft ""))

;; A resample command failed (the host reports why).
(def show-error (message)
  (set! resample-view.error message))
(def cancel () (host-command "resample-close" (dict)))

(def has-tag? (tag)
  (> (len (filter |existing| (= (string-downcase existing) (string-downcase tag))
             resample-view.tags))
     0))
(def add-tag (tag)
  (if (or (= tag "") (has-tag? tag)) (set! resample-view.tag-draft "")
    (do (set! resample-view.tags (append resample-view.tags (list tag)))
        (set! resample-view.tag-draft ""))))
(def remove-tag (tag)
  (set! resample-view.tags (filter |existing| (not (= existing tag)) resample-view.tags)))

;; A tag typed but not yet entered still counts.
(def commit ()
  (add-tag resample-view.tag-draft)
  (host-command "resample-commit"
    (dict :start resample-view.crop-start :end resample-view.crop-end
          :name resample-view.name :tags resample-view.tags)))

(def clamp-view-start (start)
  (max 0 (min (max 0 (- (duration) resample-view.view-duration)) start)))

(def action (event)
  (match event.type
    :set-selection
    (set-crop event.start event.end)
    :begin-marker-drag
    (set! resample-view.marker (if (= event.marker :start) "start" "end"))
    :end-marker-drag
    (do (set! resample-view.marker "none") (crop-changed))
    :clear-selection
    (set-crop 0 (duration))
    :scroll-view
    (set! resample-view.view-start (clamp-view-start (+ resample-view.view-start event.delta-time)))
    :zoom-view
    (let ((ratio (/ (- event.anchor-time resample-view.view-start) resample-view.view-duration))
          (next (max 0.05 (min (duration) (/ resample-view.view-duration event.factor)))))
      (set! resample-view.view-duration next)
      (set! resample-view.view-start (clamp-view-start (- event.anchor-time (* ratio next)))))
    _
    nil))

(def seconds-text (value) (str (/ (round (* 1000 value)) 1000)))

(def panel ()
  (modal :key "resample-modal" :is-open resample-view.open :on-close (lambda () (cancel))
    :title "Resample" :width-px 1120 :height-px 760
    (if resample-view.open
    (v-stack :width :fill :height :fill :gap 0.6
      (h-stack :width :fill :gap 1 :align :center
        (label "Your last 30 seconds" :bg :transparent :font-size 18)
        (box :height 0 :flex 1)
        (button "Zoom to crop" :key "resample-zoom-crop" :variant :ghost :on-click |event|
          (do (set! resample-view.view-duration
                (max 0.05 (- resample-view.crop-end resample-view.crop-start)))
              (set! resample-view.view-start (clamp-view-start resample-view.crop-start))))
        (button "Show all" :key "resample-show-all" :variant :ghost :on-click |event|
          (do (set! resample-view.view-start 0) (set! resample-view.view-duration (duration))))
        (button "Print again" :key "resample-reprint" :variant :ghost
          :on-click |event| (host-command "resample-open" (dict))))
      (label "Drag the start and end handles to crop. Scroll or pinch to zoom."
        :bg :transparent :font-size 11 :color :dim)
      (box :key "resample-wave-container" :width :fill :height 6
        :background-color :instrument-control-bg :corner-radius 10 :padding 0.3
        (if resample-view.buffer
          (subtree :key (str "resample-wave-" (get resample-view.buffer :registry-key))
            (waveform :key "resample-wave" :width :fill :height 5.4
              :header-height 1.2 :ruler-font-size 8 :ruler-color :dim :ruler-bg :black
              :grid-major-color :black :grid-minor-color :black
              :bg :instrument-control-bg :focusable true
              :marker-selection true :active-marker resample-view.marker
              :marker-color :dim :active-marker-color :widget-knob-filled
              :waveform-color :yellow :inactive-waveform-color '(rgba 0.25 0.25 0.25 1)
              :buffer resample-view.buffer
              :view-start resample-view.view-start :view-duration resample-view.view-duration
              :zoom-min-duration 0.05 :zoom-max-duration (duration)
              :selection-start resample-view.crop-start :selection-end resample-view.crop-end
              :playhead-time (if (playing?) #'browser.preview-position -1)
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
              (play-icon :active #'browser.preview-playing))))
        (label "Start (s)" :bg :transparent :font-size 11)
        (number-picker :key "resample-start" :value resample-view.crop-start
          :min 0 :max (duration) :step 0.01 :decimals 3 :width 9
          :on-change |value| (set-crop value resample-view.crop-end))
        (label "End (s)" :bg :transparent :font-size 11)
        (number-picker :key "resample-end" :value resample-view.crop-end
          :min 0 :max (duration) :step 0.01 :decimals 3 :width 9
          :on-change |value| (set-crop resample-view.crop-start value))
        (label (str "Loop " (seconds-text (- resample-view.crop-end resample-view.crop-start)) " s")
          :key "resample-length" :bg :transparent :font-size 11 :color :dim :flex 1))
      (h-stack :width :fill :gap 1 :align :start
        (v-stack :width 0 :flex 1 :gap 0.3
          (label "Name" :bg :transparent :font-size 11)
          (text-input :key "resample-name" :width :fill :height 1.35 :font-size 11
            :value resample-view.name :placeholder "sample name"
            :on-change (lambda (v) (set! resample-view.name v))))
        (v-stack :width 0 :flex 1 :gap 0.3
          (label "Tags" :bg :transparent :font-size 11)
          (eseq.sample-import/chip-row "resample-tags" resample-view.tags
            (lambda (tag) (remove-tag tag)))
          (eseq.sample-import/tag-entry "resample-tag" resample-view.tag-draft
            (lambda (v) (set! resample-view.tag-draft v))
            resample-view.tags
            (lambda (tag) (add-tag tag))
            "add a tag, Enter to apply")))
      (label "Add saves the crop to the sample library and loads it on a new sampler track."
        :bg :transparent :font-size 10 :color :dim)
      (h-stack :width :fill :gap 0.8 :align :center
        (box :height 0 :flex 1)
        (button "Cancel" :key "resample-cancel" :variant :ghost :on-click |event| (cancel))
        (button "Add as sampler track" :key "resample-commit" :on-click |event| (commit)))
      (label resample-view.error :key "resample-error" :bg :transparent :font-size 11 :color :red))
    (box :height 0 :width 0))))
