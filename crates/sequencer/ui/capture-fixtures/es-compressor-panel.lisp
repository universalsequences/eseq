;; ES Compressor builtin panel fixture.
;; cargo run -q -p sequencer --bin metal_seq -- capture --script crates/sequencer/ui/capture-fixtures/es-compressor-panel.lisp --buffer fx --track 0 --width 2400 --height 420 --out /tmp/es-compressor.png
(capture-project
  (track :sampler :name "ES Compressor" :audio-fx ("ES Compressor")))
