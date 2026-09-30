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
      (set! j.figures (list (list :dot :dot :dash) (list 4 :dot :dash)))
      ;; Slot items (docs/sexp-slot-spec.md): words, forms, a number list
      ;; and a word cycle, one per cycle; row 3 is a record saved before the
      ;; slot (it still reads).
      (set! j.rows
        (list (dict :route 0 :mods (list "left" (list "fast" (list 1 2))))
              (dict :route 1 :mods (list "right" "accent"
                                         (list "left" "left" "right")))
              (dict :route 2 :mods (list (list "trunc" (list 3 1)) "right"
                                         (list "every" 2 (list "rev" "swap"))
                                         (list "every-fig" 2 "rev")))
              (dict :route 0 :mods (list (dict :op "every" :args (list 4 "rev")))))))))
