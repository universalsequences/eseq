;; Shared macro-mapping arm state. The FX manifest loads this before both the
;; device wrappers and the reusable macro controls.

(defstate macro-mapping-open false)
(defstate macro-mapping-selected -1)

;; The full sequencer overrides these to temporarily mount the mapping table in
;; its sidebar. Standalone macro-control tests and captures keep no-op hooks.
(def macro-mapping-sidebar-open-hook () true)
(def macro-mapping-sidebar-close-hook () true)
(def macro-mapping-sidebar-refresh-hook () true)

(def macro-clear-mapping-arm ()
  (do
    (set! macro-mapping-open false)
    (set! macro-mapping-selected -1)
    (macro-mapping-sidebar-close-hook)))

;; param-controls.lisp replaces this hook with the three-way arm-mode handoff.
(def macro-mapping-arm-enter-hook () true)
