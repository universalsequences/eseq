;; Shared macro-mapping arm state. The FX manifest loads this before both the
;; device wrappers and the reusable macro controls.

(defstate macro-mapping-open false)
(defstate macro-mapping-selected -1)
(defstate rack-macro-mapping-selected -1)

;; Extension hooks: the full sequencer adds listeners that temporarily mount
;; the mapping table in its sidebar. Standalone macro-control tests and
;; captures leave them empty — running a listener-less hook is a no-op.
(defhook "macro-mapping-sidebar-open-hook")
(defhook "macro-mapping-sidebar-close-hook")
(defhook "macro-mapping-sidebar-refresh-hook")

(def macro-clear-mapping-arm ()
  (do
    (set! macro-mapping-open false)
    (set! macro-mapping-selected -1)
    (macro-mapping-sidebar-close-hook)
    true))

(def rack-macro-clear-mapping-arm ()
  (if (>= rack-macro-mapping-selected 0)
    (do
      ;; Clear the arm before rebuilding the layout so the sidebar switches
      ;; back to its previous content instead of rendering one stale frame.
      (set! rack-macro-mapping-selected -1)
      (macro-mapping-sidebar-close-hook)
      true)
    false))

;; param-controls.lisp adds the three-way arm-mode handoff as a listener.
(defhook "macro-mapping-arm-enter-hook")
