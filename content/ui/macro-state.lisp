;; Shared macro-mapping arm state. The FX manifest loads this before both the
;; device wrappers and the reusable macro controls.
(module eseq.macro-state)

(export macro-arm
        rack-armed?
        arm-macro!
        macro-name
        macro-mapping-sidebar-open-hook
        macro-mapping-sidebar-close-hook
        macro-mapping-sidebar-refresh-hook
        clear-mapping-arm
        rack-clear-mapping-arm
        macro-mapping-arm-enter-hook)

;; The macro a click on a green parameter maps: a project macro while `open`
;; (by its id, `mid`: the map command's address) or a drum rack macro of the
;; current track's rack (by its index, `rack-index`); -1 for none.
(def-kind macro-arm
  :key ()
  :state ((open false)
          (mid -1)
          (rack-index -1)))

;; Whether a drum rack macro is armed (the map commands address it).
(def rack-armed? () (>= macro-arm.rack-index 0))

;; Arm project macro `mid` for mapping (no hooks: the controls run those).
(def arm-macro! (mid)
  (set! macro-arm.open true)
  (set! macro-arm.mid mid))

;; COMPAT (eseq-0l17.61): the rack panel's macro dicts name a legacy field
;; for live typing (`:name-field`); the effects port reads the rack macro's
;; own `rm.name` and deletes this. Empty text is an edit in progress, so fall
;; back only when the field is absent.
(def macro-name (macro)
  (let ((name (if (get macro :name-field) (reactive-get "SEQ" (get macro :name-field)) nil)))
    (if (= name nil) (get macro :name) name)))

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
  (set! macro-arm.open false)
  (set! macro-arm.mid -1)
  (run-hook "macro-mapping-sidebar-close-hook")
  true)

(def rack-clear-mapping-arm ()
  (if (rack-armed?)
    (do
      ;; Clear the arm before rebuilding the layout so the sidebar switches
      ;; back to its previous content instead of rendering one stale frame.
      (set! macro-arm.rack-index -1)
      (run-hook "macro-mapping-sidebar-close-hook")
      true)
    false))

;; param-controls.lisp adds the three-way arm-mode handoff as a listener.
(defhook "macro-mapping-arm-enter-hook")
