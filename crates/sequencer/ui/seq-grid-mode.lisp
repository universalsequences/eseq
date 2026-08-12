;; seq-grid-mode key handling, page/pattern commands, param-mode setters and current-param accessors.
;; Extracted from ui/main.lisp (module-system spec slice S2). Headerless on
;; purpose: implicit eseq.vanilla until per-file (module …) headers land in S3.

(def seq-grid-handle-key (key text)
  (if (= (current-buffer-name) "*sequencer*")
    (seqv-handle-key key text)
    (if (= key "LEFT")
      (do (cursor-left) true)
      (if (= key "RIGHT")
        (do (cursor-right) true)
        (if (= key "C-a")
          (do (select-all-steps) true)
          (if (or (= key "BS") (= key "Delete"))
            (do (delete-selected-steps) true)
            (if (= key "RET")
              (do (cursor-toggle) true)
              false)))))))

(def goto-page (page)
  (do
    (cool-off-follow)
    (set-track-cursor-step (min (* page page-size) (- (max 1 SEQ.tp-num-steps) 1)))))

(def double-track-pattern ()
  (do
    (cool-off-follow)
    (seq-double-track-pattern)
    (set-track-cursor-step (min (current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

(def halve-track-pattern ()
  (do
    (cool-off-follow)
    (seq-halve-track-pattern)
    (set-track-cursor-step (min (current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

;; Cursor keys scoped to *metal* buffer via mode
(define-mode "seq-grid-mode" :read-only true :on-key "seq-grid-handle-key")
(mode-bind-key "seq-grid-mode" "LEFT" "cursor-left")
(mode-bind-key "seq-grid-mode" "RIGHT" "cursor-right")
(mode-bind-key "seq-grid-mode" "C-a" "select-all-steps")
(mode-bind-key "seq-grid-mode" "BS" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "Delete" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "RET" "cursor-toggle")
(mode-bind-key "seq-grid-mode" "C-h" "seqv-collapse-all-tracks")

(def set-vel-mode () (set! param-mode 0))
(mode-bind-key "seq-grid-mode" "v" "set-vel-mode")
(def set-dur-mode () (set! param-mode 1))
(mode-bind-key "seq-grid-mode" "d" "set-dur-mode")
(def set-aux-mode () (set! param-mode 2))
(mode-bind-key "seq-grid-mode" "a" "set-aux-mode")
(def set-transpose-mode () (set! param-mode 3))
(mode-bind-key "seq-grid-mode" "t" "set-transpose-mode")
(def set-pan-mode () (set! param-mode 4))
(mode-bind-key "seq-grid-mode" "p" "set-pan-mode")
(def set-sync-mode () (set! param-mode 5))
(mode-bind-key "seq-grid-mode" "s" "set-sync-mode")
(def set-delay-mode () (set! param-mode 6))
(mode-bind-key "seq-grid-mode" "l" "set-delay-mode")
(def set-process-lane-mode ()
  (if (> (len SEQ.process-lanes) 0)
    (set! param-mode seqv-process-lane-mode-offset)
    nil))
(mode-bind-key "seq-grid-mode" "x" "set-process-lane-mode")


(def param-values ()
  (seqv-current-param-values param-mode))

(def param-min ()
  (seqv-param-min param-mode))

(def param-max ()
  (seqv-param-max param-mode))

(def param-slider-min ()
  (if (= param-mode 1) 0 (param-min)))

(def param-slider-max ()
  (if (= param-mode 1) 1 (param-max)))

(def param-slider-value (step)
  (if (= param-mode 1)
    (duration-slider-position (nth (param-values) step))
    (nth (param-values) step)))

(def param-haptic-pivot-position ()
  (if (= param-mode 1) 0.5 1))

(def param-haptic-pivot-value ()
  (if (= param-mode 1) 2 (param-max)))

(def param-haptic-exponent ()
  (if (= param-mode 1) 4 1))

(def param-keyword ()
  (seqv-param-keyword param-mode))

(def param-color ()
  (seqv-param-color param-mode))

(def param-name ()
  (seqv-param-name param-mode))

(def param-origin ()
  (seqv-param-origin param-mode))

(def sync-current-label ()
  (nth SEQ.sync-labels (floor (+ 0.5 (nth SEQ.syncs (current-step))))))
