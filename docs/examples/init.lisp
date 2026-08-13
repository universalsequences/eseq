;; Example ~/.eseq.d/init.lisp
;;
;; Eseq loads this file transactionally after all factory modules and authored
;; content. Missing files are fine; a failing file is rolled back and reported
;; in *lisp-reload* without aborting the app. Save edits while metal_seq is
;; running to hot-reload them.

(module user.init)

;; 1. Extend a factory hook. The entry key makes reload replace this listener
;; instead of adding a duplicate.
(defstate macro-sidebar-open-count 0)
(add-hook "macro-mapping-sidebar-open-hook" "user.init/count-opens"
  (lambda ()
    (set! macro-sidebar-open-count (+ macro-sidebar-open-count 1))))

;; 2. Add an M-x command and bind it. Declared-module commands appear in M-x
;; under their qualified name: user.init/focus-mixer.
(def focus-mixer ()
  (switch-to-buffer "*mixer*"))
(bind-key "C-c m" "user.init/focus-mixer")

;; 3. Apply a small theme delta after the factory theme. Unmentioned colors
;; retain their current values.
(apply-theme (dict
  :accent '(0.95 0.25 0.65)
  :blue   '(0.30 0.65 1.00)))

;; 4. Wrap a visible factory component. `original` is resolved when this code
;; runs, so editing/reloading mixer.lisp updates the component underneath this
;; badge without removing the advice.
(override eseq.mixer/patch-mixer-strip :around (original track)
  (v-stack :gap 0.3
    (badge "CUSTOM" :color :accent)
    (original track)))

;; Evaluate this to return to stock immediately:
;; (remove-override eseq.mixer/patch-mixer-strip)
