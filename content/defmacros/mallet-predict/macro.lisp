; A mallet: a free mass thrown at an object (Modalys bi-two-mass used as a
; striker). Not a modal object, but it speaks the same protocol. x / v are its
; position and velocity last sample (from the patch's histories). On hit it is
; placed at `drop` above the rest line moving down at `speed` (per sample);
; otherwise it coasts. Returns (x, v): the free position and velocity this
; sample. Its compliance is G / mass in a modal object's units.
(defmacro mallet-predict (x v hit drop speed)
  (def v-free (gswitch hit (* -1 speed) v))
  (def x-start (gswitch hit drop x))
  (tuple (+ x-start v-free) v-free))
