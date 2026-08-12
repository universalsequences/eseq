;; seq-shell listeners on the macro-mapping-sidebar-* hooks.
;; Extracted from ui/main.lisp (module-system spec slice S2). Headerless on
;; purpose: implicit eseq.vanilla until per-file (module …) headers land in S3.

(defstate macro-mapping-sidebar-was-visible true)

(add-hook "macro-mapping-sidebar-open-hook" "seq-shell"
  (lambda ()
    (do
      (set! macro-mapping-sidebar-was-visible samples-sidebar-visible)
      (set! samples-sidebar-visible true))))

(add-hook "macro-mapping-sidebar-refresh-hook" "seq-shell"
  (lambda () (seq-refresh-current-layout)))

(add-hook "macro-mapping-sidebar-close-hook" "seq-shell"
  (lambda ()
    (do
      (set! samples-sidebar-visible macro-mapping-sidebar-was-visible)
      (seq-refresh-current-layout))))
