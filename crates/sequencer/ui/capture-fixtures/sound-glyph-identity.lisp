;; Delta-glyph identity-tier fixture (delta-glyph spec §5.1a): an
;; all-identical, all-default cohort previously rendered as a literal void.
;; Every tile here must instead show the instrument's AST silhouette — the
;; same mass on every tile (correct: nothing differs), different across
;; instruments. The last two cells add one live param so the identity
;; padding composes with real accent pieces.
;;
;; Also exercises the shader tuning props: rim glow, outer halo, and
;; SDF-depth interior shading (all off by default in the widget).
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/sound-glyph-identity.lisp \
;;     --buffer sound-glyph-gallery --width 1400 --height 800 --out /tmp/sound-glyph-identity.png

(capture-project
  (track :sampler :name "Gallery"))

(def capture-sound-glyphs
  (list
    (dict :key "glyph-demo:snare:a" :instrument "drums/ultrasnare" :params (dict))
    (dict :key "glyph-demo:snare:b" :instrument "drums/ultrasnare" :params (dict))
    (dict :key "glyph-demo:snare:c" :instrument "drums/ultrasnare" :params (dict))
    (dict :key "glyph-demo:operator:same" :instrument "core/operator" :params (dict))
    (dict :key "glyph-demo:operator:same2" :instrument "core/operator" :params (dict))))

(def glyph-cell (title key)
  (v-stack :gap 0.2 :align :center
    (sound-glyph :key (str "gallery-" key) :source key
      :width 26 :height 22
      :rim-width 0.06 :rim-gain 0.5
      :glow-width 0.12 :glow-gain 0.25
      :interior-shade 0.18 :interior-width 0.42
      :background-color '(rgba 0.03 0.035 0.04 1))
    (label title :font-size 9 :color :dim)))

(effect-buffer "*sound-glyph-gallery*"
  (h-stack :gap 2 :padding 1
    (glyph-cell "ultrasnare / same A" "glyph-demo:snare:a")
    (glyph-cell "ultrasnare / same B" "glyph-demo:snare:b")
    (glyph-cell "ultrasnare / same C" "glyph-demo:snare:c")
    (glyph-cell "operator / same A" "glyph-demo:operator:same")
    (glyph-cell "operator / same B" "glyph-demo:operator:same2")))
