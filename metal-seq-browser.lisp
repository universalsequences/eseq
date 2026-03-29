;; metal-seq-browser.lisp — Sample browser mode for Metal Sequencer
;; C-x s to open, type to filter, Enter to audition, +/= to add track, q to quit
;; Uses tree widget inside scroll container for hierarchical browsing.

;; ── State ──
(def sbrowser-filter (state ""))
(def sbrowser-source-buffer "")

;; ── SDF widgets ──

(defwidget browser-panel-bg
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect (* width 1) (* height 1) 0.02)
              (material :color (rgba 0.16 0.16 0.17 1)))))

(defwidget mag-glass
  :width 2 :height 2
  :paint-margin 0.4
  :shader
  (let (
      (__cx -0.05) (__cy 0.08) (__r 0.5)
      (__lens (- (sqrt (+ (* (- x __cx) (- x __cx))
                          (* (- y __cy) (- y __cy)))) __r))
      (__ring (- (abs __lens) 0.07))
      (__px (- x 0.35)) (__py (- y 0.385))
      (__cos 0.866) (__sin 0.5)
      (__rx (+ (* __cos __px) (* __sin __py)))
      (__ry (- (* __cos __py) (* __sin __px)))
      (__hx (- __rx (clamp __rx 0.0 0.4)))
      (__handle (- (sqrt (+ (* __hx __hx) (* __ry __ry))) 0.08))
      (__shape (min __ring __handle)))
    (sdf/layer
      (sdf/fill __shape
        (material
          :color (rgba 0.45 0.47 0.50 1.0))))))

;; ── Actions ──

(def sbrowser-audition (item)
  (let ((path (get item :path)))
    (if path
      (do
        (host-command "audition-sample" (dict :path path))
        (status (str "Audition: " (get item :label))))
      (status (str (get item :label))))))

(def sbrowser-add-track (item)
  (let ((path (get item :path)))
    (if path
      (do
        (host-command "add-track-sample" (dict :path path))
        (status (str "Add track: " (get item :label))))
      (status "Select a sample file, not a folder"))))

;; ── Filter logic: keep tree structure but only branches with matching leaves ──

(def sbrowser-filter-tree (tree query)
  (if (= query "")
    tree
    (reduce
      |acc item|
      (let ((label (get item :label))
            (children (get item :children)))
        (if children
          (let ((filtered (sbrowser-filter-tree children query)))
            (if (> (len filtered) 0)
              (append acc (list (dict :label label :children filtered)))
              acc))
          (if (string-contains? (string-downcase label) (string-downcase query))
            (append acc (list item))
            acc)))
      '()
      tree)))

;; ── Search bar widget ──

(def sbrowser-header ()
  (box :padding 0.25
    (text-input
      :value sbrowser-filter
      :placeholder "Search samples..."
      :on-change (lambda (v) (set! sbrowser-filter v))
      :width 50
      :height 1.5
      :font-size 12
      (mag-glass))))

;; ── Build widgets ──

(def sbrowser-build-widgets ()
  (let ((header (sbrowser-header)))
    (list header
      (box :background "browser-panel-bg" :padding 0 :flex 1
        (scroll :flex 1
          (tree
            :row-bg-even  '(0.16 0.16 0.17)
            :row-bg-odd   '(0.19 0.19 0.20)
            :selected-bg  '(0.00 0.35 0.82)
            :folder-color '(0.88 0.88 0.89)
            :file-color   '(0.62 0.62 0.65)
            :chevron-color '(0.50 0.50 0.53)
            :items (sbrowser-filter-tree (seq-sample-tree) sbrowser-filter)
            :expand-all (not (= sbrowser-filter ""))
            :on-select (lambda (item) (sbrowser-audition item))
            :on-activate (lambda (item) (sbrowser-add-track item))))))))

;; ── Reactive rendering (like metal-seq-grid.lisp) ──

(effect-buffer "*samples*"
  (v-stack :gap 0.5 :padding 1 (sbrowser-build-widgets)))

;; ── Entry point: just switch to the buffer ──

(def sample-browser-here ()
  (set! sbrowser-source-buffer (current-buffer-name))
  (set! sbrowser-filter "")
  (switch-to-buffer "*samples*"))

(bind-key "C-x s" "sample-browser-here")
