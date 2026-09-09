; Shared voice building blocks. Amplitude envelopes use milliseconds, matching
; the published DGen ADSR primitive; frequency depths use octaves.
(defmacro ff-smooth (target reset)
  (make-history previous)
  (def old (read-history previous))
  (def coefficient (- 1 (exp (/ -1 (* 0.002 samplerate)))))
  (def value (gswitch reset target (+ old (* coefficient (- target old)))))
  (write-history previous value)
  value)

(defmacro ff-hz (target reset)
  (exp (ff-smooth (log (max target 0.001)) reset)))

(defmacro ff-phase (frequency reset)
  (make-history previous)
  (def value (gswitch reset 0 (wrap (+ (read-history previous) (/ frequency samplerate)) 0 1)))
  (write-history previous value)
  (* twopi value))

; Wichmann-Hill combined generator. Every integer product is below 2^24,
; so the recurrence remains exact in float32. Distinct explicit seeds give
; independent band streams without depending on compiler-global RNG state.
(defmacro ff-noise (seed reset)
  (make-history x)
  (make-history y)
  (make-history z)
  (def sx (gswitch reset (+ 1 (wrap (* seed 13) 0 30268)) (read-history x)))
  (def sy (gswitch reset (+ 1 (wrap (* seed 17) 0 30306)) (read-history y)))
  (def sz (gswitch reset (+ 1 (wrap (* seed 19) 0 30322)) (read-history z)))
  (def nx (wrap (* 171 sx) 0 30269))
  (def ny (wrap (* 172 sy) 0 30307))
  (def nz (wrap (* 170 sz) 0 30323))
  (write-history x nx)
  (write-history y ny)
  (write-history z nz)
  (- (* 2 (wrap (+ (/ nx 30269) (/ ny 30307) (/ nz 30323)) 0 1)) 1))

; A complex resonator driven by white noise. Pole radius is defined by
; bandwidth in Hz; matched gain bounds its integrated power as width changes.
; This exposes decay bandwidth, not a claim of exact digital -6 dB width.
(defmacro ff-noise-band (source center bandwidth)
  (def radius (exp (/ (* -1 pi bandwidth) samplerate)))
  (def angle (/ (* twopi center) samplerate))
  (def c (* radius (cos angle)))
  (def s (* radius (sin angle)))
  (make-history real)
  (make-history imag)
  (def r (read-history real))
  (def i (read-history imag))
  (def nr (+ (* c r) (* -1 s i) (* (sqrt (- 1 (* radius radius))) source)))
  (def ni (+ (* s r) (* c i)))
  (write-history real nr)
  (write-history imag ni)
  nr)

(defmacro ff-dc (x)
  (make-history previous_x)
  (make-history previous_y)
  (def y (+ (- x (read-history previous_x))
            (* (exp (/ (* -1 twopi 5) samplerate)) (read-history previous_y))))
  (write-history previous_x x)
  (write-history previous_y y)
  y)

(defmacro ff-pan (signal pan)
  (tuple (* signal (cos (* (+ pan 1) 0.25 pi)))
         (* signal (sin (* (+ pan 1) 0.25 pi)))))
