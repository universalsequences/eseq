; Step 4 of the modal protocol: add the connection forces to the free state.
; push is the summed force on the modes (G F phi per connection; scalar for a
; lumped object). The result is clamped at +-50 as a last-resort guard; a
; healthy patch never reaches it. The patch stores it: the old s1 becomes s2,
; the result becomes s1.
(defmacro modal-advance (free push)
  (max (min (+ free push) 50) -50))
