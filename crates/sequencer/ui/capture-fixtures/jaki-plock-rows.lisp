;; jaki rows with (plock NAME V) items (docs/jaki-plock-spec.md §5.3, §6):
;; row 2 (Hat, no filter) draws fx1:Filter:cutoff in the error color; the
;; Off row is never validated. Capture with --buffer "*jaki · jaki 1*".
(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Snare" :audio-fx ("filter"))
  (track :sampler :name "Hat"))
(import alez.jaki.kind)
(host-command "instance-create" (dict :kind "alez/jaki:jaki"))
(def capture-after-sync ()
  (let ((j (instance-ref 1)))
    (do
      (set! j.figures (list (list :dot :dot :dash) (list 4 :dot :dash)))
      (set! j.rows
        (list (dict :route 0 :mods (list "left" (list "plock" "instrument:speed" (list "seq" ":hit" 1 2 0.5))))
              (dict :route 1 :mods (list "right" (list "plock" "fx1:Filter:cutoff" 0.4)))
              (dict :route 2 :mods (list (list "plock" "fx1:Filter:cutoff" 0.4) (list "on" "left" (list "plock" "instrument:speed" 2))))
              (dict :route -1 :mods (list (list "plock" "anything:goes" 1))))))))
