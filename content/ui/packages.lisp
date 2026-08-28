;; A dired-like view over the user's module root. Filesystem and project-buffer
;; mutations stay in the host, while this module owns the editor command/mode
;; surface so user key customizations use the ordinary Lisp keymap machinery.
(module eseq.packages)

(export packages
        new-package
        handle-key)

(def packages ()
  (host-command "open-packages-view" (dict)))

(def new-package ()
  (packages))

(def handle-key (key text)
  (do
    (host-command "packages-view-key" (dict :key key :text text))
    true))

(def attach-project () (handle-key "C-a" ""))
(def attach-user-init () (handle-key "C-i" ""))
(def jump-to-user-init () (handle-key "C-j" ""))
(def refresh () (handle-key "C-g" ""))

(define-mode "eseq.packages/packages-mode"
  :read-only true
  :live-keys true
  :on-key "handle-key")
(mode-bind-key "eseq.packages/packages-mode" "C-a" "attach-project")
(mode-bind-key "eseq.packages/packages-mode" "C-i" "attach-user-init")
(mode-bind-key "eseq.packages/packages-mode" "C-j" "jump-to-user-init")
(mode-bind-key "eseq.packages/packages-mode" "C-g" "refresh")

;; The shortcut is intentionally a prefix chord: ordinary printable keys in
;; the view belong to its shared filter/new-name field.
(bind-key "C-x p" "eseq.packages/packages")
