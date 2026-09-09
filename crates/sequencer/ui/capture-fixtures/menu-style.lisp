;; Shared popup chrome: checked, focused, unchecked and disabled menu rows.
;; Capture with --buffer menu-style --width 900 --height 600.
(capture-project
  (track :sampler :name "Sampler"))

(def capture-click-widgets (list "menu-item"))

(effect-buffer "*menu-style*"
  (context-menu :is-open true :anchor-col 3 :anchor-row 3
    (menu-item "Main" :checked true
      (menu-item "Rename" :on-select (lambda (event) nil)))
    (menu-item "Sends only" :checked false :on-select (lambda (event) nil))
    (menu-item "Bus A" :checked false :on-select (lambda (event) nil))
    (menu-item "Bus B" :checked false :disabled true :on-select (lambda (event) nil))))
