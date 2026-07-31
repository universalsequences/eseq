;; Sound-glyph visual iteration fixture (sound-glyph spec §6, P2):
;; core/wavetable plants at defaults plus two heavily-tweaked variants.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/sound-glyph-wavetable.lisp \
;;     --buffer sound-glyph-gallery --width 1400 --height 800 --out /tmp/sound-glyph-wavetable.png

(capture-project
  (track :sampler :name "Gallery"))

(def capture-sound-glyphs
  (list
    (dict :key "glyph-demo:wavetable:default"
      :instrument "core/wavetable"
      :params (dict))
    (dict :key "glyph-demo:wavetable:folded"
      :instrument "core/wavetable"
      :params (dict :osc1_warp 0.95 :osc1_fold 0.9 :osc1_gain_db 0
                    :osc2_on 1 :osc2_warp 0.8 :osc2_fold 0.85
                    :osc2_detune 0.4 :cutoff 19000 :resonance 0.85
                    :filter_env_amt 0.9 :filt_attack_ms 5))
    (dict :key "glyph-demo:wavetable:hollow"
      :instrument "core/wavetable"
      :params (dict :osc1_warp 0.02 :osc1_fold 0 :osc1_gain_db -30
                    :osc2_on 0 :cutoff 220 :resonance 0.1
                    :amp_attack_ms 4000 :amp_release_ms 8000
                    :filt_sustain 0.05 :volume_db -24))))

(def glyph-cell (title key)
  (v-stack :gap 0.2 :align :center
    (sound-glyph :key (str "gallery-" key) :source key
      :width 26 :height 22
      :background-color '(rgba 0.03 0.035 0.04 1))
    (label title :font-size 9 :color :dim)))

(effect-buffer "*sound-glyph-gallery*"
  (h-stack :gap 2 :padding 1
    (glyph-cell "wavetable / defaults" "glyph-demo:wavetable:default")
    (glyph-cell "wavetable / folded" "glyph-demo:wavetable:folded")
    (glyph-cell "wavetable / hollow" "glyph-demo:wavetable:hollow")))
