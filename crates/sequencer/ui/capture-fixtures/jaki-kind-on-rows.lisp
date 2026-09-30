;; A jaki panel with scoped (on SEL word) rows, a (seq :hit …) melody, and one
;; long row that stays on one line (crates/sequencer/docs/jaki-sequencer-spec.md
;; §7.1-§7.2). Capture with --buffer "*jaki · jaki 1*".
(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Snare")
  (track :sampler :name "Hat"))
(import alez.jaki.kind)
(host-command "instance-create" (dict :kind "alez/jaki:jaki"))
(def capture-after-sync ()
  (let ((j (instance-ref 1)))
    (do
      (set! j.figures (list (list :dot :dot :dash) (list 4 :dot :dash)))
      (set! j.rows
        (list (dict :route 0 :mods (list (list "on" "left" (list "vel*" 0.05)) (list "dashdecay" 0.26) (list "on" (list "fig" 4) (list "note" 5)) (list "minvel" 0) (list "dotdecay" 0.29) (list "on" "left" (list "note+" (list 22 14 12))) (list "on" (list "fig" 2) (list "slow" 2)) (list "note" (list "seq" ":hit" 0 3 7 10))))
              (dict :route 1 :mods (list (list "on" (list "fig" 2) (list "every" 2 (list "fast" 2)))))
              (dict :route 2 :mods (list (list "on" (list "and" (list "fig" 3) "tail") (list "vel*" 0.6))
                                         (list "on" (list "nth" 2 (list "fig" 2)) "rest")))
              (dict :route 0 :mods (list (list "note" 5) (list "on" (list "and" (list "fig" 2) "dot") (list "note+" 7)) "stac")))))))
