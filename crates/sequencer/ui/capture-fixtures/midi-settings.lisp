;; Real application modal, with deterministic device discovery presentation.
(capture-project (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (reactive-set "MIDI" "devices" (list
    (dict :id "keyboard" :name "USB MIDI Keyboard" :enabled true :connected true :status "Connected")
    (dict :id "pads" :name "Pad Controller" :enabled false :connected false :status "Disabled")))
  (reactive-set "MIDI" "error" "")
  (reactive-set "AUDIO" "workers-choice" "Auto (6)")
  (reactive-set "AUDIO" "workers-options" (list "Auto (6)" "1" "2" "3" "4" "5" "6" "7" "8" "9" "10"))
  (reactive-set "AUDIO" "workers-note" "Running 6 workers.")
  (eseq.settings/open-settings))
