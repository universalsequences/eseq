(defmacro pitch-transpose (hz semitones)
  (def factor (pow 2.0 (/ semitones 12.0)))
  (* hz factor))
