;; Jaki figure builder — the defscene proving ground.
;;
;; The sequencer definition is published once. `jb-figures` remains a named
;; scene-slot read in its shipped tick body, so changing scenes selects another
;; body without republishing or recompiling the definition. Every editor action
;; is a plain set! of the pattern-scoped value; defscene supplies persistence,
;; targeted repaint, scheduler publication, and undo.

(import alez.jaki.surface)

(def jb-name "jaki-builder")
(def script-buffer-name "*jaki-builder*")
(def script-tab-label "Jaki")
(def script-sequencer-name jb-name)

(def jb-shape-options (list ". . . ." ". . -" "- . . ." ". - . -"))
(def jb-mod-options (list "none" "fast 2" "every 4 swap" "stac"))

(def jb-default-figure () '(fig (. . . .)))

;; The slot is the scheduler-ready Jaki body, including its route. Keeping the
;; portable data in executable shape is what lets the tick call `run` directly
;; without shipping any builder-only helper functions.
(defscene jb-figures (append (list (jb-default-figure)) '(-> 0)))

(def jb-shape-from-label (label)
  (match label
    ". . -" '(. . -)
    "- . . ." '(- . . .)
    ". - . -" '(. - . -)
    _ '(. . . .)))

(def jb-shape-label (shape)
  (let ((rendered (source shape)))
    (if (= rendered "(. . -)") ". . -"
      (if (= rendered "(- . . .)") "- . . ."
        (if (= rendered "(. - . -)") ". - . -" ". . . .")))))

(def jb-mod-form (label)
  (match label
    "fast 2" '(* 2)
    "every 4 swap" '(every 4 swap)
    "stac" '(stac)
    _ nil))

(def jb-figure-shape (figure) (nth figure 1))
(def jb-figure-mod (figure)
  (let ((mod (nth figure 2)))
    (if (= mod nil) "none"
      (let ((rendered (source mod)))
        (if (= rendered "(* 2)") "fast 2"
          (if (= rendered "(every 4 swap)") "every 4 swap"
            (if (= rendered "(stac)") "stac" "none")))))))

(def jb-with-shape (figure shape)
  (cons 'fig (cons shape (rest (rest figure)))))

(def jb-with-mod (figure label)
  (let ((mod (jb-mod-form label)))
    (cons 'fig (cons (jb-figure-shape figure)
      (if (= mod nil) (list) (list mod))))))

;; This is the sole sequencer publication. The register macro deliberately
;; accepts a body expression rather than quoting body syntax.
(alez.jaki.surface/register jb-name :16 jb-figures)

(def jb-update-nth (items index update)
  (map (lambda (i)
         (if (= i index) (update (nth items i)) (nth items i)))
       (range 0 (len items))))

(def jb-set-shape (index label)
  (set! jb-figures
    (jb-update-nth jb-figures index
      (lambda (figure) (jb-with-shape figure (jb-shape-from-label label))))))

(def jb-set-mod (index label)
  (set! jb-figures
    (jb-update-nth jb-figures index
      (lambda (figure) (jb-with-mod figure label)))))

(def jb-figure-count () (- (len jb-figures) 2))
(def jb-prefix (items count)
  (if (or (<= count 0) (empty? items))
    (list)
    (cons (first items) (jb-prefix (rest items) (- count 1)))))

(def jb-add-figure ()
  (set! jb-figures
    (append (jb-prefix jb-figures (jb-figure-count))
            (list (jb-default-figure))
            '(-> 0))))

(def jb-remove-figure (index)
  (if (> (jb-figure-count) 1)
    (set! jb-figures
      (append
        (reduce (lambda (kept i)
                  (if (= i index) kept (append kept (list (nth jb-figures i)))))
                (list)
                (range 0 (jb-figure-count)))
        '(-> 0)))
    nil))

(defstate jb-baked-code "")

(def jb-code ()
  (source (append (list 'jak jb-name :16) jb-figures)))

(def jb-bake ()
  (set! jb-baked-code (jb-code)))

(def jb-figure-row (index figure)
  (h-stack :key (str "jaki-builder-row-" index) :gap 0.5 :align :center
    (label (str (+ index 1)) :width 1.4 :height 1.2 :font-size 9
      :h-align :center :color :dim :bg :transparent)
    (dropdown :key (str "jaki-builder-shape-" index)
      :value (jb-shape-label (jb-figure-shape figure))
      :options jb-shape-options
      :width 9 :height 1.2 :font-size 9
      :on-change (lambda (value) (jb-set-shape index value)))
    (dropdown :key (str "jaki-builder-mod-" index)
      :value (jb-figure-mod figure)
      :options jb-mod-options
      :width 12 :height 1.2 :font-size 9
      :on-change (lambda (value) (jb-set-mod index value)))
    (button "-" :key (str "jaki-builder-remove-" index)
      :width 2 :height 1.2 :font-size 9
      :on-click (lambda (event) (jb-remove-figure index)))))

(def jb-panel (figures pattern)
  (box :key "jaki-builder-panel" :width 31
    :height (+ 5 (* (- (len figures) 2) 1.7)) :padding 0.8
    :background-color :mixer-strip-bg :border-color :mixer-strip-border
    :corner-radius 12
    (v-stack :width :fill :gap 0.55
      (h-stack :width :fill :gap 0.5 :align :center
        (label (str "scene " (+ pattern 1)) :width 8 :height 1.2
          :font-size 9 :color :dim :bg :transparent)
        (button "+ figure" :key "jaki-builder-add"
          :width 7 :height 1.2 :font-size 9
          :on-click (lambda (event) (jb-add-figure)))
        (button "bake to code" :key "jaki-builder-bake"
          :width 10 :height 1.2 :font-size 9
          :on-click (lambda (event) (jb-bake))))
      (each (range 0 (- (len figures) 2)) |index|
        (jb-figure-row index (nth figures index)))
      (box :key "jaki-builder-code" :width 29.4 :height 1.4 :padding 0.2
        :background-color :bg :corner-radius 4
        (label (if (= jb-baked-code "")
                 "Bake the current scene to a (jak ...) form"
                 jb-baked-code)
          :width 29 :height 1 :font-size 8 :color :dim :bg :transparent)))))

(effect-buffer "*jaki-builder*"
  (box :width 32 :height (+ 6 (* (- (len jb-figures) 2) 1.7))
    (jb-panel jb-figures SEQ.current-pattern)))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab
  script-tab-label script-buffer-name script-sequencer-name "")
