;; metal-seq-browser.lisp — Sample browser mode for Metal Sequencer
;; C-x s to open, type to filter, Enter to audition, +/= to add track, q to quit
;; Uses tree widget inside scroll container for hierarchical browsing.

;; ── State ──
(def sbrowser-filter "")
(def sbrowser-source-buffer "")

;; ── Mode (for keyboard input) ──

(define-mode "sample-browser-mode" :read-only true :on-key "sbrowser-handle-key")
(mode-bind-key "sample-browser-mode" "q" "sbrowser-quit")

;; ── SDF widgets ──

(defwidget browser-panel-bg
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect (* width 0.98) (* height 0.98) 0.02)
              (material :color (rgba 0.16 0.16 0.17 1)))))

(defwidget search-bar
  :paint-margin 0.5
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect (* aspect 1) 1 1)
      (material
        :color
        (let ((__d d)
              (__inner (mix (rgba 0.24 0.25 0.26 1.0)
                            (rgba 0.20 0.21 0.23 1.0)
                            (smoothstep 0.43 0.13 (* y d))))
              (__border (mix (rgba 0.15 0.17 0.12 1.0)
                             (rgba 0.18 0.19 0.22 1.0)
                             (smoothstep 0 0.001 d))))
          (mix __border __inner (smoothstep 0.0 0.1 (- (abs d) 0.003))))))))

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

;; ── Search bar widget ──

(def sbrowser-header ()
  (box :padding 0.25
    (box :background "search-bar" :height 1.5 :width 50
      :align :left :padding 0.25
      (h-stack :gap 0.5 :align :center
        (mag-glass)
        (label (if (= sbrowser-filter "") "Search samples..." sbrowser-filter)
          :font-size 12
          :color (if (= sbrowser-filter "")
            '(rgba 0.4 0.4 0.45 1)
            '(rgba 0.85 0.85 0.85 1))
          :bg :transparent)))))

;; ── Search results (flat list with focusable labels) ──

(def sbrowser-search-result-widget (entry)
  (box :padding 0.1
    (label (str "  " (get entry :parent) "/" (get entry :name))
      :font-size 13 :width 80
      :color '(rgba 0.8 0.8 0.8 1)
      :focusable true
      :on-enter (lambda ()
        (host-command "audition-sample" (dict :path (get entry :path)))
        (status (str "Audition: " (get entry :name))))
      :on-focus-key (lambda (key text)
        (if (or (= text "+") (= text "="))
          (do
            (host-command "add-track-sample" (dict :path (get entry :path)))
            (status (str "Add track: " (get entry :name)))
            true)
          false)))))

;; ── Build widgets ──

(def sbrowser-build-widgets ()
  (let ((header (sbrowser-header)))
    (if (= sbrowser-filter "")
      ;; Browse mode: tree widget
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
              :items (seq-sample-tree)
              :on-select (lambda (item) (sbrowser-audition item))
              :on-activate (lambda (item) (sbrowser-add-track item))))))
      ;; Search mode: flat filtered results
      (let ((entries (seq-search-samples sbrowser-filter)))
        (cons header
          (cons (label (fmt "  {} results" (len entries))
                  :font-size 13 :color :gray :width 80)
            (map sbrowser-search-result-widget entries)))))))

;; ── Refresh ──

(def sbrowser-refresh ()
  (render-widget (v-stack :gap 0.5 :padding 1 (sbrowser-build-widgets)))
  (status (if (= sbrowser-filter "") "samples" (fmt "search: {}" sbrowser-filter))))

;; ── Key handling (type to filter) ──

(def sbrowser-handle-key (key text)
  (if text
    (do
      (set! sbrowser-filter (str sbrowser-filter text))
      (sbrowser-refresh)
      true)
    (if (= key "BS")
      (do
        (if (> (len sbrowser-filter) 0)
          (set! sbrowser-filter (substring sbrowser-filter 0 (- (len sbrowser-filter) 1)))
          nil)
        (sbrowser-refresh)
        true)
      (if (= key "ESC")
        (do
          (set! sbrowser-filter "")
          (sbrowser-refresh)
          true)
        false))))

;; ── Quit ──

(def sbrowser-quit ()
  (render-widget nil)
  (if (not (= sbrowser-source-buffer ""))
    (switch-to-buffer sbrowser-source-buffer)
    (let ((bufs (buffer-list)))
      (if (> (len bufs) 1)
        (switch-to-buffer (nth bufs 0))
        (status "No other buffer")))))

;; ── Entry point ──

(def sample-browser-here ()
  (set! sbrowser-source-buffer (current-buffer-name))
  (set! sbrowser-filter "")
  (switch-or-create-buffer "*samples*")
  (set-buffer-mode "sample-browser-mode")
  (sbrowser-refresh))

(bind-key "C-x s" "sample-browser-here")

;; Auto-refresh browser if it's already open (enables live-editing this file)
(if (and (not (= sbrowser-filter nil))
         (reduce |acc b| (if (= b "*samples*") true acc) false (buffer-list)))
  (let ((tree (v-stack :padding 1 :gap 0.5 (sbrowser-build-widgets))))
    (render-widget-to-buffer "*samples*" tree)
    (status "browser refreshed"))
  nil)
