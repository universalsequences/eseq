;; ui/patcher.lisp — patch editor key bindings.
;;
;; The patcher widget handles typing into an edited node and into an agentic
;; bubble prompt natively. Every other key it refuses, and the editor hands the
;; refused key to the widget's :on-focus-key, which is `handle-focus-key`
;; below. That looks the key up in the binding table the natives keep and runs
;; the named command against the patcher that refused it, so a binding never
;; has to know patcher internals: a command that does not apply (nothing
;; selected, no bubble to retry) returns false and the key falls through.
;;
;; Key names follow the editor's spelling (`M-x describe-key`): RET, UP, BS,
;; ESC, Tab, Delete, lowercase letters, with modifiers C- M- S- s-. `P-` stands
;; for the platform's primary shortcut modifier (Cmd on macOS, Ctrl elsewhere)
;; and `P-S-` for primary plus Shift, so one table serves both platforms.
;;
;; Rebind from user lisp, for example in init.lisp:
;;   (eseq.patcher/bind-key "M-RET" "accept-suggestions")
;;   (eseq.patcher/unbind-key "Tab")
;; Commands: create-below connect-last-two undo redo copy paste encapsulate
;; toggle-cable-style retry-bubble dismiss-bubble open-bubble connect-bubble
;; open-macro delete-selection accept-suggestions
(module eseq.patcher)

(export handle-focus-key
        bind-key
        unbind-key
        command
        open-context-menu
        context-menu-panel
        menu-entries)

(def bind-key (key cmd) (patcher-bind-key key cmd))
(def unbind-key (key) (patcher-unbind-key key))
(def command (name) (patcher-command name))

(def handle-focus-key (key text)
  (patcher-key key))

;; ── Context menu ──
;;
;; Right-click (or ctrl+click) on the canvas. The widget's payload carries the
;; node or cable under the pointer (`:node` / `:cable`), how many nodes are
;; selected after the click, and whether Jev ghost cables are showing. Every
;; item runs a named patcher command against the patcher that was clicked, so
;; the menu is the same surface as the keys, with the bound key as its hint.
(def menu-open (state false))
(def menu-col (state 0))
(def menu-row (state 0))
(def menu-event (state nil))

(def open-context-menu (event)
  (do
    (set! menu-event event)
    (set! menu-col (get event :col))
    (set! menu-row (get event :row))
    (set! menu-open true)))

(def run-menu-command (name)
  (do
    (set! menu-open false)
    (patcher-command name)))

(def shortcut-hint (name)
  (let ((key (patcher-key-for-command name)))
    (if key key "")))

(def menu-entry (label name)
  (menu-item label
    :key (str "patcher-menu-" name)
    :shortcut (shortcut-hint name)
    :on-select (lambda (event) (run-menu-command name))))

(def menu-entries ()
  (let ((event menu-event)
        (node (if (= menu-event nil) nil (get menu-event :node)))
        (cable (if (= menu-event nil) nil (get menu-event :cable))))
    (append
      (if (and (not (= node nil)) (get node :macro?))
        (list (menu-entry "Open Macro" "open-macro"))
        (list))
      (if (not (= node nil))
        (list (menu-entry "Connect with Agent…" "connect-bubble")
              (menu-entry "Ask Agent…" "open-bubble"))
        (list (menu-entry "Ask Agent…" "open-bubble")))
      (if (and (not (= event nil)) (get event :ghosts))
        (list (menu-entry "Accept Suggested Cables" "accept-suggestions"))
        (list))
      (if (not (= node nil))
        (list (menu-entry "Encapsulate" "encapsulate")
              (menu-entry "Copy" "copy"))
        (list))
      (list (menu-entry "Paste" "paste"))
      (if (not (= cable nil))
        (list (menu-entry "Toggle Cable Style" "toggle-cable-style"))
        (list))
      (if (or (not (= node nil)) (not (= cable nil)))
        (list (menu-entry "Delete" "delete-selection"))
        (list)))))

(def context-menu-panel ()
  (context-menu :is-open menu-open
    :anchor-col menu-col
    :anchor-row menu-row
    :on-close (lambda () (set! menu-open false))
    (each (menu-entries) |entry| entry)))

;; Defaults. Tests compare this list against the Rust copy
;; (`DEFAULT_PATCHER_BINDINGS`), so keep each on one line as (bind-key "key" "command").
(bind-key "P-RET" "create-below")
(bind-key "P-UP" "connect-last-two")
(bind-key "P-z" "undo")
(bind-key "P-S-z" "redo")
(bind-key "P-c" "copy")
(bind-key "P-v" "paste")
(bind-key "P-e" "encapsulate")
(bind-key "P-y" "toggle-cable-style")
(bind-key "P-r" "retry-bubble")
(bind-key "P-k" "open-bubble")
(bind-key "P-S-k" "connect-bubble")
(bind-key "RET" "open-macro")
(bind-key "ESC" "dismiss-bubble")
(bind-key "BS" "delete-selection")
(bind-key "Delete" "delete-selection")
(bind-key "Tab" "accept-suggestions")
;; The Ctrl spellings kept working on macOS alongside Cmd for these five, so
;; they stay bound; on other platforms they duplicate the P- rows above.
(bind-key "C-z" "undo")
(bind-key "C-S-z" "redo")
(bind-key "C-c" "copy")
(bind-key "C-v" "paste")
(bind-key "C-e" "encapsulate")
