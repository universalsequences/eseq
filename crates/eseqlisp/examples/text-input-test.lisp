;; Text input + tree filtering demo

(load "../sequencer/ui/themes/mac-osx-dark.lisp")

;; ── SDF widgets for search bar styling ──

(defwidget panel-bg
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect (* width 1) (* height 1) 0.02)
              (material :color (rgba 0.16 0.16 0.17 1)))))

(defwidget mag-glass
  :width 1.6 :height 1.6
  :paint-margin 0.4
  :shader
  (let (
      (__cx -0.05) (__cy 0.08) (__r 0.45)
      (__lens (- (sqrt (+ (* (- x __cx) (- x __cx))
                          (* (- y __cy) (- y __cy)))) __r))
      (__ring (- (abs __lens) 0.12))
      (__px (- x 0.32)) (__py (- y 0.35))
      (__cos 0.866) (__sin 0.5)
      (__rx (+ (* __cos __px) (* __sin __py)))
      (__ry (- (* __cos __py) (* __sin __px)))
      (__hx (- __rx (clamp __rx 0.0 0.35)))
      (__handle (- (sqrt (+ (* __hx __hx) (* __ry __ry))) 0.12))
      (__shape (min __ring __handle)))
    (sdf/layer
      (sdf/fill __shape
        (material
          :color (rgba 0.50 0.52 0.55 1.0))))))

;; ── State ──

(def search-text (state ""))
(def selected-item (state ""))

;; ── Sample tree data ──

(def sample-tree '(
  (:label "drums" :children (
      (:label "acoustic" :children (
          (:label "kick_tight.wav")
          (:label "kick_room.wav")
          (:label "snare_crack.wav")
          (:label "snare_buzz.wav")))
      (:label "electronic" :children (
          (:label "808_kick.wav")
          (:label "808_snare.wav")
          (:label "909_hat_closed.wav")
          (:label "909_hat_open.wav")))))
  (:label "synths" :children (
      (:label "pads" :children (
          (:label "warm_pad.wav")
          (:label "glass_pad.wav")))
      (:label "leads" :children (
          (:label "saw_lead.wav")
          (:label "square_lead.wav")))
      (:label "bass" :children (
          (:label "sub_bass.wav")
          (:label "reese_bass.wav")))))
  (:label "fx" :children (
      (:label "risers" :children (
          (:label "white_noise_rise.wav")
          (:label "tonal_rise.wav")))
      (:label "impacts" :children (
          (:label "boom.wav")
          (:label "crash_cymbal.wav")))))))

;; ── Filter logic: keep tree structure but only branches with matching leaves ──

(def filter-tree (tree query)
  (if (= query "")
    tree
    (reduce
      |acc item|
      (let ((label (get item :label))
            (children (get item :children)))
        (if children
          ;; Folder: recurse into children
          (let ((filtered (filter-tree children query)))
            (if (> (len filtered) 0)
              (append acc (list (dict :label label :children filtered)))
              acc))
          ;; Leaf: check if label matches
          (if (string-contains? (string-downcase label) (string-downcase query))
            (append acc (list item))
            acc)))
      '()
      tree)))

;; ── UI ──

(effect (v-stack :padding 1 :gap 1
    (label (str "Selected: " selected-item) :color :white :bg :transparent :font-size 14)

    ;; Search bar with text-input widget
    (text-input
      :value search-text
      :placeholder "Search samples..."
      :on-change (lambda (v) (set! search-text v))
      :width 50
      :height 1.5
      :font-size 13
      (mag-glass))

    ;; Tree view — filtered when searching, expand-all to show matches
    (box :background "panel-bg" :padding 0 :flex 1
      (scroll :flex 1
        (tree
          :row-bg-even  '(0.16 0.16 0.17)
          :row-bg-odd   '(0.19 0.19 0.20)
          :selected-bg  '(0.00 0.35 0.82)
          :folder-color '(0.88 0.88 0.89)
          :file-color   '(0.62 0.62 0.65)
          :chevron-color '(0.50 0.50 0.53)
          :items (filter-tree sample-tree search-text)
          :expand-all (not (= search-text ""))
          :on-select (lambda (item) (set! selected-item (get item :label)))
          :on-activate (lambda (item) (status (str "Activate: " (get item :label)))))))))
