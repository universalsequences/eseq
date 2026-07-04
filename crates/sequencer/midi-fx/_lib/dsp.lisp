(def fx-min (a b)
  (if (< a b) a b))

(def fx-max (a b)
  (if (> a b) a b))

(def fx-clamp (value low high)
  (fx-min (fx-max value low) high))

(def fx-positive-mod (value divisor)
  (mod (+ (mod value divisor) divisor) divisor))

(def fx-positive-int (value)
  (fx-max 1 (round value)))

(def fx-wrap-transpose-into-range (note low high)
  (let ((lo (fx-min low high))
        (hi (fx-max low high)))
    (if (< note lo)
      (fx-clamp (+ lo (fx-positive-mod (- note lo) 12)) lo hi)
      (if (> note hi)
        (fx-clamp (- hi (fx-positive-mod (- hi note) 12)) lo hi)
        note))))

(def fx-note-shift (note semitones)
  (merge note :note (+ (get note :note) semitones)))

(def fx-note-octaves (note octaves)
  (map |oct|
    (fx-note-shift note (* 12 oct))
    (range 0 (fx-positive-int octaves))))

(def fx-notes-octaves (notes octaves)
  (reduce |acc oct|
    (append acc
      (map |note| (fx-note-shift note (* 12 oct)) notes))
    (list)
    (range 0 (fx-positive-int octaves))))

(def fx-note-active-at? (note beat)
  (and (>= beat (get note :start))
       (< beat (get note :end))))

(def fx-notes-active-at (notes beat)
  (filter |note| (fx-note-active-at? note beat) notes))

(def fx-notes-end (notes)
  (reduce |end note|
    (fx-max end (get note :end))
    0
    notes))

(def fx-directed-index (tick count direction)
  (if (<= count 1)
    0
    (if (= direction 1)
      (- (- count 1) (mod tick count))
      (if (= direction 2)
        (let ((period (- (* count 2) 2))
              (pos (mod tick (- (* count 2) 2))))
          (if (< pos count) pos (- period pos)))
        (if (= direction 3)
          (mod (+ (* tick 1103515245) 12345) count)
          (mod tick count))))))

(def fx-arp-count-for (notes rate)
  (let ((rate-beats (fx-time rate)))
    (if (<= rate-beats 0)
      0
      (ceil (/ (fx-notes-end notes) rate-beats)))))

(def fx-arp-note-from (notes rate tick direction)
  (let ((beat (fx-time rate tick))
        (active (fx-notes-active-at notes (fx-time rate tick))))
    (if (= (len active) 0)
      false
      (nth active
        (fx-directed-index
          (+ tick (fx-phase-tick rate))
          (len active)
          direction)))))

(def fx-arp-emit-from (notes rate tick direction gate velocity)
  (let ((note (fx-arp-note-from notes rate tick direction)))
    (if note
      (fx-emit :beats (fx-time rate tick)
        :note (get note :note)
        :vel velocity
        :dur (* gate (/ (fx-time rate) (fx-source-time))))
      false)))
