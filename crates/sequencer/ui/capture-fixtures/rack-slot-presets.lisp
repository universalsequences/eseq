;; Select a real rack slot and inspect its instrument preset bank in *samples*.
(capture-project
  (track :layer-rack :name "Instrument Rack"
    :instruments ("factory:Synths/Digi Drift" "factory:Drums/808 Kick")))

(def capture-after-sync ()
  (set! eseq.vanilla/sbrowser-tab "presets")
  (seq-set-delete-target :rack-slot (dict :track 0 :slot 0)))
