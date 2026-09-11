;; Use --track 0..6 to inspect the custom readouts in Mods mode.
(capture-project
  (track :instrument "factory:Synths/Heat")
  (track :instrument "factory:Synths/Digi FM")
  (track :instrument "factory:Synths/Digi Drift")
  (track :instrument "factory:Synths/Poseidon")
  (track :instrument "factory:Synths/Melt")
  (track :instrument "factory:Drums/808 Clap")
  (track :instrument "factory:Drums/808 Kick"))
(def capture-after-sync ()
  (set! eseq.effects.state/instrument-mods-open true))
