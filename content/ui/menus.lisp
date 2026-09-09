;; Public application-menu configuration. The native adapter owns no menu policy.
(module eseq.menus)
(export set-menus register-menu remove-menu definition current-menus activate shortcut-label)

(defstate menu-definitions (list))

(def definition () menu-definitions)
(def current-menus () (native-menu-definition))
(def activate (id) (native-menu-activate id))
(def shortcut-label (shortcut)
  (if shortcut (native-menu-shortcut-label shortcut) ""))

;; :enabled-when is a zero-argument Lisp predicate. Reads inside it participate
;; in this observer's dependency graph, including native-menu-context reads.
(def resolve-item (item)
  (if item
    (let ((predicate (get item :enabled-when))
          (enabled (if predicate (predicate) (not (= (get item :enabled) false))))
          (items-fn (get item :items-when))
          (children (if items-fn (items-fn) (get item :items)))
          (check (get item :checked-when))
          (resolved (if check (merge item :checked (check)) item)))
      (if (or items-fn children)
        (merge resolved :enabled (and enabled (> (len children) 0)) :items (map resolve-item children))
        (merge resolved :enabled enabled)))
    nil))

(def set-menus (menus)
  ;; Validate first; invalid configuration never replaces the current registry.
  (if (native-menu-validate (map resolve-item menus))
    (do (set! menu-definitions menus) true)
    false))

(def register-menu (menu)
  (let ((id (get menu :id))
        (existing (filter (lambda (entry) (= (get entry :id) id)) menu-definitions)))
    (set-menus
      (if (> (len existing) 0)
        (map (lambda (entry) (if (= (get entry :id) id) menu entry)) menu-definitions)
        (append menu-definitions (list menu))))))

(def remove-menu (id)
  (set-menus (filter (lambda (menu) (not (= (get menu :id) id))) menu-definitions)))

(observe (native-menu-set! (map resolve-item menu-definitions)))
