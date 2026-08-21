(defmacro freq-to-delay (freq sample_rate)
  (def max-0 (max freq 0.001))
  (def return (/ sample_rate max-0))
  return)
