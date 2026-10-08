;; Real application modal, with deterministic device discovery presentation.
(capture-project (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (present-fixture "settings"
    (dict :midi-devices
          (list
            (dict :device-id "keyboard" :name "USB MIDI Keyboard" :enabled true :connected true :status "Connected")
            (dict :device-id "pads" :name "Pad Controller" :enabled false :connected false :status "Disabled"))
          :midi-error ""
          :audio-workers-choice "Auto (6)"
          :audio-workers-options (list "Auto (6)" "1" "2" "3" "4" "5" "6" "7" "8" "9" "10")
          :audio-workers-note "Running 6 workers."))
  (eseq.settings/open-settings))
