;; ui/settings.lisp — the Settings modal: audio workers and MIDI inputs
;; (the host's `settings` kind, kind-bindings spec §14.2i).
(module eseq.settings)
(import eseq.kinds :refer (settings project))
(import eseq.view-kit :refer (nothing))
(export settings-view open-settings close-settings panel)

;; Whether the modal is open (File > Settings… opens it through the host).
(def-kind settings-view
  :key ()
  :state ((open false)))

(def open-settings () (set! settings-view.open true))
(def close-settings () (set! settings-view.open false))

(def device-row (d)
  (h-stack :key (str "midi-device-" d.device-id) :width :fill :height 2.2 :gap 0.6 :align :center
    (v-stack :flex 1 :gap 0.2
      (label d.name :key (str "midi-name-" d.device-id)
        :font-size 13 :color :white :bg :transparent)
      (label d.status :key (str "midi-status-" d.device-id)
        :font-size 10 :color :dim :bg :transparent))
    (button (if d.enabled "Disable" "Enable")
      :key (str "midi-toggle-" d.device-id)
      :on-click (lambda (event) (toggle! d.enabled)))))

;; The engine reads the worker count once at start; the note says what is
;; running now and what the saved choice becomes after a restart.
(def audio-section ()
  (v-stack :width :fill :gap 0.2
    (h-stack :width :fill :height 1.6 :gap 0.6 :align :center
      (label "Audio workers" :font-size 14 :color :white :bg :transparent)
      (box :flex 1 :bg :transparent)
      (dropdown :key "audio-workers" :width 10 :height 1.2 :font-size 12
        :value settings.audio-workers-choice
        :options project.audio-workers-options
        :on-change (lambda (v) (set! settings.audio-workers-choice v))))
    (label settings.audio-workers-note :key "audio-workers-note"
      :font-size 11 :color :dim :bg :transparent)
    (label "Helper threads for the audio graph. Changes apply on next launch."
      :font-size 11 :color :dim :bg :transparent)))

(def settings-body ()
  (v-stack :width :fill :height :fill :gap 0.3
    (label "Settings" :font-size 18 :color :white :bg :transparent)
    (audio-section)
    (h-stack :width :fill :gap 0.6 :align :center
      (label "MIDI inputs" :font-size 14 :color :white :bg :transparent)
      (box :flex 1 :bg :transparent)
      (button "Refresh" :key "midi-refresh"
        :on-click (lambda (event) (host-command "midi-refresh" (dict)))))
    (label (if settings.midi-persistent
      "New inputs connect automatically. Device choices are saved."
      "New inputs connect automatically. Choices apply to this session.")
      :font-size 11 :color :dim :bg :transparent)
    (label "Arm a track or rack to play it from an enabled input." :font-size 11 :color :dim :bg :transparent)
    (if (= settings.midi-error "")
      (nothing)
      (label settings.midi-error :key "midi-error" :width :fill :wrap true :font-size 11 :bg :transparent))
    (scroll :key "midi-device-list" :width :fill :flex 1
      (v-stack :width :fill :gap 0.3
        (if (empty? settings.midi-devices)
          (label "No MIDI inputs found. Connect a device to begin." :key "midi-empty"
            :font-size 12 :color :dim :bg :transparent)
          (each settings.midi-devices |d| (device-row d)))))
    (h-stack :width :fill
      (box :flex 1 :bg :transparent)
      (button "Done" :key "settings-done" :variant :primary
        :on-click (lambda (event) (close-settings))))))

(def panel ()
  (modal :is-open settings-view.open :on-close close-settings :width-px 680 :height-px 640
    (box :debug-name "settings-panel" :width :fill :height :fill :padding 0.8 :bg :transparent
      (if settings-view.open (settings-body) (nothing)))))
