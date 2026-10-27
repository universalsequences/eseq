;; A fretted string: a finger presses the plucked string onto the
;; fingerboard at 'fret (a fifth up, 1/3 of the way along), which clamps it
;; there while the key is held; the board just under the string lets it buzz.
;; Move fret's point to slide.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-fretted"
  :objects (list (m/finger 'fin :depth 0.12)
                 (m/string 'str :made-of mat/steel :length 0.65 :radius-mm 0.45 :modes 70)
                 (m/plate 'body :made-of mat/spruce :mass 20))
  :connections (list (m/fingerboard 'fret 'str 0.333 :board -0.0012)
                     (m/pluck 'pick 'fin 'str 0.8)
                     (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4)))
  :listens (list (m/listen 'str 0.9 :gain 0.6) (m/listen 'body (list 0.3 0.65) :gain 0.6)))
