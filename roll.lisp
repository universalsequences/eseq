



















(defstate last-action nil)
(defstate items-text "")
(defstate drag-kind nil)
(defstate drag-base-items '())
(defstate view-start 0)
(defstate view-duration 16)
(defstate lane-scroll 0)
(defstate next-item-id 100)

(defstate lanes
  (list
    (dict :id 0 :label "kick")
    (dict :id 1 :label "snare")
    (dict :id 2 :label "hat")))

(defstate items
  (list
    (dict :id 10 :lane 0 :start 1 :end 4 :selected true)
    (dict :id 11 :lane 1 :start 6 :end 9 :selected false)
    (dict :id 12 :lane 2 :start 10 :end 14 :selected false)))

(def contains? (needle xs)
  (if (= (len xs) 0)
    false
    (if (= needle (first xs))
      true
      (contains? needle (rest xs)))))

(def item-by-id (xs target-id)
  (if (= (len xs) 0)
    nil
    (let ((item (first xs)))
      (let (((id) item))
        (if (= id target-id)
          item
          (item-by-id (rest xs) target-id))))))

(def clear-selection (xs)
  (->> xs
    (map |item| (merge item :selected false))))

(def apply-select (xs ids)
  (->> xs
    (map |item|
      (let (((id) item))
        (merge item :selected (contains? id ids))))))

(def marquee-hit? (item event)
  (let (((lane start end) item)
        ((lane-a lane-b time-a time-b) event))
    (if (>= lane lane-a)
      (if (<= lane lane-b)
        (if (< start time-b)
          (> end time-a)
          false)
        false)
      false)))

(def apply-marquee (xs event)
  (->> xs
    (map |item| (merge item :selected (marquee-hit? item event)))))

(def reset-drag ()
  (do
    (set! drag-kind nil)
    (set! drag-base-items '())))

(def drag-base-for (kind)
  (if (= drag-kind kind)
    drag-base-items
    (do
      (set! drag-kind kind)
      (set! drag-base-items items)
      items)))

(def clamp-lane (lane)
  (if (< lane 0) 0 lane))

(def clamp-view-start (start)
  (if (< start 0) 0 start))

(def clamp-view-duration (duration)
  (min 128 (max 1 duration)))

(def apply-move-items-absolute (base event)
  (let (((ids anchor-id start lane) event))
    (let ((anchor (item-by-id base anchor-id)))
      (if anchor
        (let ((delta-time (- start anchor.start))
              (delta-lane (- lane anchor.lane)))
          (map |item|
            (if (contains? item.id ids)
              (let ((new-start (+ item.start delta-time)))
                (merge item
                  :lane (clamp-lane (+ item.lane delta-lane))
                  :start new-start
                  :end (+ new-start (- item.end item.start))
                  :selected true))
              (merge item :selected false))
            base))
        base))))

(def apply-resize-item-absolute (base event)
  (map |item|
    (if (= item.id event.id)
      (let ((base-item (item-by-id base event.id)))
        (if base-item
          (if (= event.edge :start)
            (merge base-item
              :start (if (< event.time base-item.end) event.time base-item.start)
              :selected true)
            (if (= event.edge :end)
              (merge base-item
                :end (if (> event.time base-item.start) event.time base-item.end)
                :selected true)
              (merge base-item :selected true)))
          item))
      (merge item :selected false))
    base))

(def without-preview (xs)
  (->> xs
    (filter |item|
      (let (((id) item))
        (not (= id 999))))))

(def apply-create-item (xs event)
  (let (((lane start end) event))
    (append
      (without-preview xs)
      (list
        (dict
          :id 999
          :lane lane
          :start start
          :end end
          :selected true)))))

(def finish-create-item (xs event)
  (let (((lane start end) event))
    (append
      (without-preview xs)
      (list
        (dict
          :id next-item-id
          :lane lane
          :start start
          :end end
          :selected true)))))

(def apply-delete-items (xs ids)
  (->> xs
    (filter |item|
      (let (((id) item))
        (not (contains? id ids))))))

(def apply-nudge-selection (xs event)
  (let (((ids delta-time delta-lane) event))
    (map |item|
      (let (((id lane start end) item))
        (if (contains? id ids)
          (merge item
            :lane (clamp-lane (+ lane delta-lane))
            :start (+ start delta-time)
            :end (+ end delta-time)
            :selected true)
          item))
      xs)))

(def apply-scroll-view (event)
  (do
    (set! view-start (clamp-view-start (+ view-start event.delta-time)))
    (set! lane-scroll (max 0 (+ lane-scroll event.delta-lanes)))))

(def apply-zoom-view (event)
  (let ((anchor-ratio (/ (- event.anchor-time view-start) view-duration))
        (next-duration (clamp-view-duration (/ view-duration event.factor))))
    (let ((next-start
            (if (and (= view-start 0) (< event.factor 1))
              0
              (clamp-view-start
                (- event.anchor-time (* anchor-ratio next-duration))))))
      (set! view-duration next-duration)
      (set! view-start next-start))))

(def handle-timeline-action (event)
  (set! last-action event)
  (match event.type
    :select
    (do
      (reset-drag)
      (set! items (apply-select items event.ids)))
    :clear-selection
    (do
      (reset-drag)
      (set! items (clear-selection items)))
    :marquee-select
    (do
      (reset-drag)
      (set! items (apply-marquee items event)))
    :move-items-absolute
    (set! items (apply-move-items-absolute (drag-base-for :move-items-absolute) event))
    :resize-item-absolute
    (set! items (apply-resize-item-absolute (drag-base-for :resize-item-absolute) event))
    :create-item
    (set! items (apply-create-item (drag-base-for :create-item) event))
    :finish-create-item
    (do
      (set! items (finish-create-item (drag-base-for :create-item) event))
      (set! next-item-id (+ next-item-id 1))
      (reset-drag))
    :delete-items
    (do
      (reset-drag)
      (set! items (apply-delete-items items event.ids)))
    :nudge-selection
    (set! items (apply-nudge-selection items event))
    :scroll-view
    (apply-scroll-view event)
    :zoom-view
    (apply-zoom-view event)
    _
    nil)
  (set! items-text (str items)))

(defstate toolstate true)
(def tool (derived (if toolstate :draw :pointer)))
(effect
  (v-stack
    
    (label (fmt "action: {}" last-action))
    (label (fmt "items: {}" items-text))
    (h-stack (label "draw: ") (toggle :bind toolstate))
    
    (timeline
      :height 12
      :focusable true
      :sidebar-width 8
      :tool tool
      :lanes lanes
      :items items
      :view-start view-start
      :view-duration view-duration
      :lane-scroll lane-scroll
      :snap 1
      :on-action |event| (handle-timeline-action event))))
