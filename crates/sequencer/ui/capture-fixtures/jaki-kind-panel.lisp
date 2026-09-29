;; A `jaki` instance's panel (alez/jaki, docs/jaki-kind-spec.md §5).
;; Capture with --buffer "*jaki · jaki 1*".
(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Snare")
  (track :sampler :name "Hat"))
(import alez.jaki.kind)
(host-command "instance-create" (dict :kind "alez/jaki:jaki"))
(def capture-after-sync ()
  (let ((j (instance-ref 1)))
    (do
      (set! j.figures (list (list :dot :dot :dash) (list :dot :dash)))
      (set! j.rows
        (list (dict :route 0 :mods (list (dict :op "left" :args (list))))
              (dict :route 1 :mods (list (dict :op "right" :args (list))
                                         (dict :op "accent" :args (list))))
              (dict :route 2 :mods (list (dict :op "trunc" :args (list 3))
                                         (dict :op "right" :args (list))
                                         (dict :op "every" :args (list 4 "rev")))))))))
