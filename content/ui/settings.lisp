(module eseq.settings)
(export open-settings close-settings panel)

(defstate settings-open? false)
(def open-settings () (set! settings-open? true))
(def close-settings () (set! settings-open? false))

(def set-device-enabled (device enabled)
  (host-command "midi-set-enabled" (dict :id (get device :id) :enabled enabled)))

(def device-row (device)
  (h-stack :key (str "midi-device-" (get device :id)) :width :fill :height 2.2 :gap 0.6 :align :center
    (v-stack :flex 1 :gap 0.2
      (label (get device :name) :key (str "midi-name-" (get device :id))
        :font-size 13 :color :white :bg :transparent)
      (label (get device :status) :key (str "midi-status-" (get device :id))
        :font-size 10 :color :dim :bg :transparent))
    (button (if (get device :enabled) "Disable" "Enable")
      :key (str "midi-toggle-" (get device :id))
      :on-click (lambda (event) (set-device-enabled device (not (get device :enabled)))))))

;; The engine reads the worker count once at start; AUDIO.workers-note says
;; what is running now and what the saved choice becomes after a restart.
(def audio-section ()
  (v-stack :width :fill :gap 0.2
    (h-stack :width :fill :height 1.6 :gap 0.6 :align :center
      (label "Audio workers" :font-size 14 :color :white :bg :transparent)
      (box :flex 1 :bg :transparent)
      (dropdown :key "audio-workers" :width 10 :height 1.2 :font-size 12
        :value (reactive-get "AUDIO" "workers-choice")
        :options (reactive-get "AUDIO" "workers-options")
        :on-change (lambda (v) (host-command "audio-set-workers" (dict :choice v)))))
    (label (reactive-get "AUDIO" "workers-note") :key "audio-workers-note"
      :font-size 11 :color :dim :bg :transparent)
    (label "Helper threads for the audio graph. Changes apply on next launch."
      :font-size 11 :color :dim :bg :transparent)))

(def settings-body ()
  (let ((devices (reactive-get "MIDI" "devices"))
        (error (reactive-get "MIDI" "error")))
    (v-stack :width :fill :height :fill :gap 0.3
      (label "Settings" :font-size 18 :color :white :bg :transparent)
      (audio-section)
      (h-stack :width :fill :gap 0.6 :align :center
        (label "MIDI inputs" :font-size 14 :color :white :bg :transparent)
        (box :flex 1 :bg :transparent)
        (button "Refresh" :key "midi-refresh"
          :on-click (lambda (event) (host-command "midi-refresh" (dict)))))
      (label (if (reactive-get "MIDI" "persistent")
        "New inputs connect automatically. Device choices are saved."
        "New inputs connect automatically. Choices apply to this session.")
        :font-size 11 :color :dim :bg :transparent)
      (label "Arm a track or rack to play it from an enabled input." :font-size 11 :color :dim :bg :transparent)
      (if (and error (not (= error "")))
        (label error :key "midi-error" :width :fill :wrap true :font-size 11 :bg :transparent)
        (box :width 0 :height 0 :bg :transparent))
      (scroll :key "midi-device-list" :width :fill :flex 1
        (v-stack :width :fill :gap 0.3
          (if (and devices (> (len devices) 0))
            (map device-row devices)
            (label "No MIDI inputs found. Connect a device to begin." :key "midi-empty"
              :font-size 12 :color :dim :bg :transparent))))
      (h-stack :width :fill
        (box :flex 1 :bg :transparent)
        (button "Done" :key "settings-done" :variant :primary
          :on-click (lambda (event) (close-settings)))))))

(def panel ()
  (modal :is-open settings-open? :on-close close-settings :width-px 680 :height-px 640
    (box :debug-name "settings-panel" :width :fill :height :fill :padding 0.8 :bg :transparent
      (if settings-open? (settings-body) (box :width 0 :height 0 :bg :transparent)))))
