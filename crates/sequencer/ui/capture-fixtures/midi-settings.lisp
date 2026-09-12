;; Real application modal, with deterministic device discovery presentation.
(capture-project (track :sampler :name "Sampler"))

(def capture-after-sync ()
  (reactive-set "MIDI" "devices" (list
    (dict :id "keyboard" :name "USB MIDI Keyboard" :enabled true :connected true :status "Connected")
    (dict :id "pads" :name "Pad Controller" :enabled false :connected false :status "Disabled")))
  (reactive-set "MIDI" "error" "")
  (eseq.settings/open-settings))
