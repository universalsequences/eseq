;; Inline code widgets end-to-end demo.
;;
;; In metal_seq:
;;   1. Open the Scripts sidebar and load `inline-code-widgets-demo.lisp`.
;;   2. Select the "Inline Code Widgets" source tab.
;;   3. Press C-x C-b once in this source buffer to evaluate it here. This is
;;      important: inline anchors belong to the source buffer being evaluated.
;;   4. Put notes on track 0 and start transport if you want to see/hear the
;;      process and live scope. The widgets themselves work while stopped.
;;
;; What to try:
;;   - Drag the three free controls below. Their numeric literals rewrite in
;;     place; one drag is one undo step, and re-evaluation happens on release.
;;     Each compact control occupies visual cells immediately before its value,
;;     so changing the value's digit count never moves the control itself.
;;   - Edit text above a control. Its anchor follows the edit. Edit inside its
;;     form and it dims until C-x C-b refreshes the source snapshot.
;;   - The process call-site knob omits :min/:max; it inherits 0..24 and integer
;;     stepping from the `limit` inlet declaration.
;;   - The process call-site toggle writes through to the current pattern's
;;     attached process while its source literal remains the saved value.
;;   - The body slider under `:run` appears without the process firing. Changing
;;     it rewrites and hot-reloads the process definition.
;;   - The orange lane band previews the literal lane. The teal scope band reads
;;     track 0's post-track audio and is flat until that track produces audio.
;;
;; Re-evaluating this file is idempotent for the current pattern: `processes`
;; replaces track 0's process chain rather than appending another instance.

(seq-register-script-source-tab "Inline Code Widgets")

;; Bare literal sites: the buffer text is the entire state model.
(def inline-demo-free-level
  (~slider 0.35 :min 0 :max 1 :step 0.01))

(def inline-demo-free-shape
  (~knob 0.5 :min 0 :max 1 :step 0.01))

(def inline-demo-free-enabled
  (~toggle 1))

;; A normal authored process. `limit` and `enabled` demonstrate call-site
;; metadata inference; `amount` demonstrates a lane-backed inlet.
(def-process inline-demo-transpose
  :doc "Inline widget demo: add a clipped lane value to step transpose."
  :target (step-param :transpose)
  :in ((limit :int 0 24 :default 7)
       (amount :float -12 12 :default 0 :lane true)
       (enabled :int 0 1 :default 1))
  :run (if (> (in :enabled) 0)
         (target-add!
           (clip (in :amount)
                 (- 0 (~slider 12 :min 0 :max 24 :step 1))
                 (in :limit)))
         nil))

;; Instance call sites: the knob and toggle write through to this handle. The
;; lane form remains the plain tagged lane value consumed by the process model.
(def inline-demo-process-h
  (inline-demo-transpose
    :limit (~knob 7)
    :amount (~lane (lane 0 2 0 0 -4 0 7 0) :height 4)
    :enabled (~toggle 1)))

(def inline-demo-chain
  (processes :track 0 inline-demo-process-h))

;; Full-width live band. Use :track 0 for post-track audio or omit :track for
;; the master output.
(~scope :track 0 :height 6)
