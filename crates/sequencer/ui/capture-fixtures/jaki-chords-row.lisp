;; A jaki instance with a Chords row and three followers
;; (docs/harmony-declaration-spec.md). Capture with --buffer "*jaki · jaki 1*".
(capture-project
  (track :sampler :name "Bass")
  (track :sampler :name "Keys")
  (track :sampler :name "Pad"))
(import alez.jaki.kind)
(host-command "instance-create" (dict :kind "alez/jaki:jaki"))
(def capture-after-sync ()
  (let ((j (instance-ref 1)))
    (do
      (set! j.figures (list (list :dot :dot :dash) (list :dot :dash)))
      (set! j.rows
        (list (dict :route -2 :mods (list (list "key" "A" ":minor")
                                          (list "chord" (list "i" "iv" (list "seq" ":fig" "V7" "bVI")))
                                          (list "on" "accent" (list "chord" "E7"))))
              (dict :route 0 :mods (list (list "note" (list "deg" 1)) (list "note+" -24)))
              (dict :route 1 :mods (list "voice" "right"))
              (dict :route 2 :mods (list (list "chord" ":declared") "accent")))))))
