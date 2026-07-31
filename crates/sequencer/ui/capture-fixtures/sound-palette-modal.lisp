;; Migrated sound selector (modal spec §4): the palette now renders as a
;; centered modal over the arrangement view instead of a band prepended to
;; it. Opens through the real funnel: seq-sound-palette-open → host command →
;; App::sound_palette_open → SEQ.sound-palette sync.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/sound-palette-modal.lisp \
;;     --buffer arrangement --width 1100 --height 700 --out /tmp/sound-palette-modal.png

(capture-project
  (track :sampler :name "Sampler")
  (track :sampler :name "Bass"))

(seq-sound-palette-open 0)
