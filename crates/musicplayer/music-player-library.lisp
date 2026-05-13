(defwidget player-browser-bg
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect (* width 1) (* height 1) 0.006)
              (material :color (rgba 0.12 0.12 0.13 1)))))

(defwidget player-search-icon
  :width 1.6 :height 1.6
  :paint-margin 0.35
  :shader
  (let ((__cx -0.08) (__cy 0.08) (__r 0.42)
        (__lens (- (sqrt (+ (* (- x __cx) (- x __cx))
                            (* (- y __cy) (- y __cy)))) __r))
        (__ring (- (abs __lens) 0.09))
        (__px (- x 0.28)) (__py (- y 0.32))
        (__cos 0.866) (__sin 0.5)
        (__rx (+ (* __cos __px) (* __sin __py)))
        (__ry (- (* __cos __py) (* __sin __px)))
        (__hx (- __rx (clamp __rx 0.0 0.34)))
        (__handle (- (sqrt (+ (* __hx __hx) (* __ry __ry))) 0.09))
        (__shape (min __ring __handle)))
    (sdf/layer
      (sdf/fill __shape
        (material :color (rgba 0.48 0.50 0.54 1.0))))))

(def play-tree-item (item)
  (let ((idx (get item :index)))
    (mp-play-track idx)))

(def library-search (state ""))

(def filter-albums (albums query)
  (if (= query "")
    albums
    (reduce
      |acc album|
      (let ((label (get album :label)))
        (if (string-contains? (string-downcase label) (string-downcase query))
          (append acc (list album))
          acc))
      '()
      albums)))

(def select-album (album)
  (let ((idx (get album :index)))
    (mp-play-album idx)))

(def truncate-album-label (label)
  (if (> (len label) 24)
    (str (substring label 0 22) "...")
    label))

(def album-card (album)
  (let ((selected (= (get album :path) MP.current_album_path))
        (cover (get album :cover_path)))
    (box
      :key (get album :path)
      :width :fill
      :height :fill
      :padding 0.35
      :corner-radius 14
      :border-width 2
      :border-color (if selected :accent '(0 0 0 0))
      :background-color (if selected '(0.10 0.13 0.17 1) '(0.11 0.11 0.12 1))
      :on-click (lambda (evt) (select-album album))
      (v-stack :gap 0.28 :width :fill
        (if (= cover "")
          (box :width :fill :aspect 1 :corner-radius 10 :background-color '(0.16 0.16 0.17 1)
            (label (get album :label) :font-size 10 :color :dim :bg :transparent))
          (image :src cover :width :fill :aspect 1 :fit :cover :radius 10))
        (label (truncate-album-label (get album :label))
          :width :fill
          :height 1.4
          :font-size 10
          :color :white
          :bg :transparent)))))

(def album-cards (albums)
  (reduce
    |acc album|
    (append acc
      (list
        (subtree :key (str "album-card-" (get album :path))
          (album-card album))))
    '()
    albums))

(effect-buffer "*library*"
  (v-stack :gap 0.5 :width :fill :flex 1 :padding 1
    (text-input
      :width :fill
      :value library-search
      :placeholder "Search library..."
      :on-change (lambda (v) (set! library-search v))
      :height 1.5
      :font-size 12
      (player-search-icon))
    (box :width :fill :background "player-browser-bg" :padding 0 :flex 1
      (subtree :key "music-player-library-scroll"
        (scroll :key "music-player-library-scroll" :width :fill :flex 1
          (responsive-grid
            :levels '(1 2 3)
            :min-item-width 13.5
            :gap 1.0
            :row-aspect 0.52
            (album-cards (filter-albums MP.albums library-search))))))))
