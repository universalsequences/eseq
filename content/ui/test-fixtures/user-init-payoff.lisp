;; Test fixture for module-system slice 4. Loaded through the production
;; post-factory user-init seam; never through the process user's ~/.eseq.d.
(module test.user-init)

(export init-payoff-command)

(add-hook "test-init-payoff-hook" "user-init"
  (lambda ()
    (host-command "user-init-hook-ran" (dict))))

;; Every Lisp function is discoverable through M-x completion.
(def init-payoff-command () "user command ran")

;; User theme deltas apply after the factory theme.
(apply-theme (dict :accent '(0.95 0.25 0.65)))

;; Advice wraps a visible factory component without capturing it.
(override eseq.mixer/patch-mixer-strip :around (original track)
  (v-stack :gap 0.3
    (badge "USER INIT" :color :accent)
    (original track)))

(run-hook "test-init-payoff-hook")
