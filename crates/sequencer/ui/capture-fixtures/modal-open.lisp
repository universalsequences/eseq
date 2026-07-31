;; Modal widget visual fixture: a hardcoded-open modal over busy underlay
;; content. Exercises v-stack + scroll + each children, the full-frame scrim,
;; and the centered panel chrome.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/modal-open.lisp \
;;     --buffer modal-preview --width 900 --height 600 --out /tmp/modal-open.png

(capture-project
  (track :sampler :name "Sampler"))

(effect-buffer "*modal-preview*"
  (v-stack :gap 1 :padding 1
    (label "underlay heading — the scrim must dim this")
    (h-stack :gap 1
      (box :background-color '(rgba 0.25 0.2 0.3 1) :width 30 :height 14
        (label "left underlay panel"))
      (box :background-color '(rgba 0.2 0.3 0.25 1) :width 30 :height 14
        (label "right underlay panel")))
    (label "bottom underlay row")
    (modal :is-open true :title "Select a sound"
      (v-stack :gap 0.4
        (label "Pick an entry below")
        (scroll :height 10
          (v-stack :gap 0.2
            (each (list "Kick 909" "Snare wire" "Hat shimmer" "Perc clave"
                        "Bass sub" "Lead saw" "Pad glass" "FX riser"
                        "Tom low" "Ride bell" "Clap room" "Vox chop")
              |name|
              (h-stack :gap 1
                (label name :width 16)
                (button "apply")
                (button "+mix")))))
        (label "footer hint — Esc closes")))))
