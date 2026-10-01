;; Jaki rows with rules, row 0's rules box open
;; (docs/jaki-row-processes-spec.md §9, §13). Capture with --buffer "*jaki · jaki 1*".
(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Keys"))
(import alez.jaki.kind)
(host-command "instance-create" (dict :kind "alez/jaki:jaki"))
(def capture-after-sync ()
  (let ((j (instance-ref 1)))
    (do
      (set! j.figures (list (list :dot :dot :dash) (list 2 :dot :dash)))
      (set! j.rows
        (list (dict :route 0 :mods (list "left" (list "scale" ":minor" ":root" "C"))
                    :procs (list (list "any" (list "acc" ":by" (list "coin" ":p" 0.5) ":min" 0 ":max" 16) (list "note+" "x"))
                                 (list "left" (list "coin" ":p" 0.5) (list "note+" -12))
                                 (list "any" (list "harmony" ":track" "1" ":amount" 1))))
              (dict :route 1 :mods (list "accent"))))
      (set! j.rules-open "0:0"))))
