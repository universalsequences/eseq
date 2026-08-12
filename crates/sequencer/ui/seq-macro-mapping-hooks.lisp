;; seq-shell listeners on the macro-mapping-sidebar-* hooks.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3.
(module eseq.seq-macro-mapping-hooks)

;; No compat aliases: nothing outside this file references its state, and hook
;; names are strings in the flat hook keyspace (spec §6), which does not
;; auto-qualify. The bare references below (`samples-sidebar-visible`,
;; `seq-refresh-current-layout`) resolve out to eseq.vanilla, which works only
;; because ui/seq-core-state.lisp and ui/seq-layout.lisp are loaded before this
;; file — a converted module's outbound references must already exist when it
;; compiles.
(defstate sidebar-was-visible true)

(add-hook "macro-mapping-sidebar-open-hook" "seq-shell"
  (lambda ()
    (do
      (set! sidebar-was-visible samples-sidebar-visible)
      (set! samples-sidebar-visible true))))

(add-hook "macro-mapping-sidebar-refresh-hook" "seq-shell"
  (lambda () (seq-refresh-current-layout)))

(add-hook "macro-mapping-sidebar-close-hook" "seq-shell"
  (lambda ()
    (do
      (set! samples-sidebar-visible sidebar-was-visible)
      (seq-refresh-current-layout))))
