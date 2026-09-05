;; Production palettes for effect displays, instrument envelopes/wavetables,
;; rack chrome, and a connected modulation cable. All routes are real graph
;; routes, not injected SEQ values. Capture each track's fx buffer, or mixer.
(capture-project
  (track :sampler :name "Phaser" :audio-fx ("Phaser-Flanger"))
  (track :sampler :name "Dynamics" :audio-fx ("OTT"))
  (track :sampler :name "Reverb" :audio-fx ("Reverb"))
  (track :sampler :name "Equalizer" :audio-fx ("EQ8"))
  (track :sampler :name "Filterbank" :audio-fx ("Filterbank"))
  (track :sampler :name "Filter Table" :audio-fx ("Filter Table"))
  (track :instrument "Synths/Poseidon" :name "Poseidon")
  (track :instrument "Synths/Digi Wave" :name "Digi Wave")
  (track :layer-rack :name "Rack"
    :samples ("../../../../content/impulses/prepared/king-tubby.wav"))
  (track :modulator :name "Modulation")
  (track :sampler :name "Compressor" :audio-fx ("Compressor"))
  (mod-route 9 6 0))

(def capture-after-sync ()
  (seq-theme-phosphor))
