;; Shared macro-mapping arm state. The FX manifest loads this before both the
;; device wrappers and the reusable macro controls.
(module eseq.macro-state)

;; Compat aliases (module-system spec §10 slice 3): every renamed def that has
;; callers outside this file. Unconverted callers keep using the flat names;
;; the table is deleted when the migration finishes. Aliases must be evaluated
;; before a caller *compiles*, which is why the FX manifest loads this file
;; first (ui/effects.lisp:6).
(module-compat-alias macro-mapping-open mapping-open)
(module-compat-alias macro-mapping-selected mapping-selected)
(module-compat-alias rack-macro-mapping-selected rack-mapping-selected)
(module-compat-alias macro-clear-mapping-arm clear-mapping-arm)
(module-compat-alias rack-macro-clear-mapping-arm rack-clear-mapping-arm)

(defstate mapping-open false)
(defstate mapping-selected -1)
(defstate rack-mapping-selected -1)

;; Extension hooks: the full sequencer adds listeners that temporarily mount
;; the mapping table in its sidebar. Standalone macro-control tests and
;; captures leave them empty — running a listener-less hook is a no-op.
;; Hook names are a flat keyspace (spec §6) and do NOT auto-qualify — leave
;; these strings alone or every add-hook site in the app breaks.
(defhook "macro-mapping-sidebar-open-hook")
(defhook "macro-mapping-sidebar-close-hook")
(defhook "macro-mapping-sidebar-refresh-hook")

;; `defhook` registers the caller-facing `(macro-mapping-sidebar-close-hook)`
;; native at RUNTIME, under the flat hook name — but this file's own call sites
;; are resolved at COMPILE time, when that global does not exist yet, so a bare
;; call would intern a dead `eseq.macro-state/…` slot. Inside a module, reach
;; hooks through `run-hook` (the flat keyspace, addressed as data).
(def clear-mapping-arm ()
  (do
    (set! mapping-open false)
    (set! mapping-selected -1)
    (run-hook "macro-mapping-sidebar-close-hook")
    true))

(def rack-clear-mapping-arm ()
  (if (>= rack-mapping-selected 0)
    (do
      ;; Clear the arm before rebuilding the layout so the sidebar switches
      ;; back to its previous content instead of rendering one stale frame.
      (set! rack-mapping-selected -1)
      (run-hook "macro-mapping-sidebar-close-hook")
      true)
    false))

;; param-controls.lisp adds the three-way arm-mode handoff as a listener.
(defhook "macro-mapping-arm-enter-hook")
