; filterbank-sc — the filterbank with an FM sidechain: fm_in replaces the
; driven input as the audio-rate cutoff FM source (like the builtin's FM
; sidechain track). Cutoff shift = tanh(fm_in) * fm * 4 octaves, so feed it
; roughly +-1 signals; scale quiet ones up. Other inputs as in filterbank.
(use-defmacro filterbank)

(defmacro filterbank-sc (x fm_in f1 f2 mode1 mode2 res drive_db crunch fm corr par harm mix)
  (def post (filterbank-drive x drive_db))
  (def wet (filterbank-core post fm_in f1 f2 harm mode1 mode2 res crunch fm corr par))
  (def theta (* (clip mix 0 1) 1.570796327))
  (+ (* x (cos theta)) (* wet (sin theta))))
