; Apply a connection force to a mallet from mallet-predict. The mallet is the
; upper end of a contact, so it takes -F (felt-contact's convention); C is its
; compliance. Once it is back above `drop` moving up it parks there, out of
; the way, until the next hit. Returns (x, v) for the patch to store.
(defmacro mallet-advance (x v F C drop)
  (def x-next (- x (* C F)))
  (def v-next (- v (* C F)))
  (def parked (* (gt x-next drop) (gt v-next 0)))
  (tuple (gswitch parked drop x-next) (gswitch parked 0 v-next)))
