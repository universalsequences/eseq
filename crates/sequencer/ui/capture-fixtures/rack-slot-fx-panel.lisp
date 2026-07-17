;; Per-slot FX panel regression fixture. Uses a checked-in WAV so the rack and
;; native OTT instance are built through the production headless project path.
(capture-project
  (track :layer-rack
    :name "Layer Rack + Slot FX"
    :samples ("../../assets/ir/lexicon-300-rich-plate.wav")
    :rack-slot-audio-fx ("OTT")))
