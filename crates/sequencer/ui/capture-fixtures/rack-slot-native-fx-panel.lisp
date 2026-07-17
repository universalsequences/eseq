;; Rack-slot analyzer/option regression fixture. EQ Eight is selected first so
;; the capture exercises its production analyzer source through a rack host.
(capture-project
  (track :layer-rack
    :name "Layer Rack + Native Slot FX"
    :samples ("../../assets/ir/lexicon-300-rich-plate.wav")
    :rack-slot-audio-fx ("EQ8" "Phaser-Flanger")))
