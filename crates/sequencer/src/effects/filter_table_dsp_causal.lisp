; Causal engine tail — minimum-phase 256-tap FIR.
;
; The hop-rate magnitude response is converted to a causal minimum-phase
; impulse response via the real cepstrum (log magnitude -> IDFT -> causal
; fold -> DFT -> complex exp -> IDFT), truncated to TAPS taps with a raised-
; cosine fade over the last 64, and applied as a per-sample time-domain FIR.
; Zero latency: the effect reports no PDC in this mode, and the dry path is
; the raw input.
;
; Two dgen scheduling constraints shape this code (probed in
; filter_table_causal.rs):
;   1. The kernel chain runs hop-gated. It must cross to frame rate through
;      `latch` (frame-rate cond) BEFORE any gather — a per-frame gather of a
;      hop-gated tensor reads clobbered scratch between hops, and a multiply
;      against the un-latched kernel silently gates the conv to hop frames.
;   2. ifft returns the real part only; every IDFT in this chain has a real
;      result (log spectrum is even, folded cepstrum is causal-real).

(def TAPS 256)

(def logm (log (max response-mag 0.00001)))
(def ceps (ifft logm (* logm 0) @N 2048 @backend accelerated))

; Causal fold: keep quefrency 0 and N/2, double 1..N/2-1, zero the rest.
(def cep-idx (iota 2048))
(def fold-w (+ (+ (eq cep-idx 0) (eq cep-idx 1024))
               (* 2 (* (gte cep-idx 1) (lte cep-idx 1023)))))
(def folded (* ceps fold-w))

(def (mp-re mp-im) (fft folded @N 2048 @backend accelerated))
(def hmag (exp mp-re))
(def h-re (* hmag (cos mp-im)))
(def h-im (* hmag (sin mp-im)))
(def ir-mp (ifft h-re h-im @N 2048 @backend accelerated))

; Capture the IR into the latch ONLY on hop frames. The ifft writes its
; output cells on the frame the hop fires; between hops those scratch cells
; can be clobbered by other tensor work, so a latch with an always-true cond
; re-captures garbage mid-hop (audible as huge transients under modulation).
; A hand-rolled frame counter stays frame-rate (so the conv below is not
; hop-gated) while firing in phase with the hop machinery's own counters,
; which also start at zero on the first processed frame.
(make-history hop-frame-ctr)
(def hop-phase-next
  (write-history hop-frame-ctr (% (+ (read-history hop-frame-ctr) 1) HOP)))
(def ir-held (latch ir-mp (eq hop-phase-next 1)))

; Kernel is stored time-reversed: the sliding window puts the newest sample
; at index TAPS-1, so kernel[i] multiplies x[n-(TAPS-1-i)] and must hold
; ir[TAPS-1-i]. The fade windows impulse-response time (rev-idx), not kernel
; index, so truncation ripple stays bounded for steep responses.
(def rev-idx (- (- TAPS 1) (iota 256)))
(def tap-fade-ramp (clip (/ (- rev-idx 192) 64) 0 1))
(def tap-fade (* 0.5 (+ 1 (cos (* PI tap-fade-ramp)))))
(def kernel-target (* (gather ir-held rev-idx) tap-fade))

; Per-tap one-pole slew at frame rate. The kernel is rebuilt once per hop and
; would otherwise switch instantly at hop boundaries — a step change in a
; convolution kernel injects an audible click, most obvious when `frame`
; morphs between dissimilar table rows. The spectral engine crossfades
; responses implicitly through its 4x overlap-add; this slew is the causal
; equivalent, turning each hop's kernel step into a ~KERNEL-SLEW-MS
; exponential glide. The chain must stay frame-rate throughout, which it is:
; kernel-target descends from the latched ir-held.
(def KERNEL-SLEW-MS 8)
(make-history kern-slew @shape [256])
(make-history kern-slew-seed)
(def slew-alpha (min 1 (/ 1.0 (max 1 (* samplerate (* KERNEL-SLEW-MS 0.001))))))
; Seed to the target on the very first sample (smooth-control convention) so
; a fresh instance has no fade-in glide; every later change slews.
(def kern-seeded (read-history kern-slew-seed))
(write-history kern-slew-seed 1)
(def kern-prev (read-history kern-slew))
(def kernel
  (write-history kern-slew
    (+ (* (- 1 kern-seeded) kernel-target)
       (* kern-seeded (+ kern-prev (* slew-alpha (- kernel-target kern-prev)))))))

(def win-l (reshape (buffer in-l 256) @shape [256]))
(def win-r (reshape (buffer in-r 256) @shape [256]))
(def wet-l (sum (* win-l kernel)))
(def wet-r (sum (* win-r kernel)))

; Equal-power dry/wet law. Both branches are zero-latency here, so the dry
; path is the raw input. Mix remains sample-rate modulatable.
(def mix-mod (clip (mod mix) 0 1))
(def dry-gain (sqrt (- 1 mix-mod)))
(def wet-gain (sqrt mix-mod))
(out (* output (+ (* in-l dry-gain) (* wet-l wet-gain))) 1 @name left)
(out (* output (+ (* in-r dry-gain) (* wet-r wet-gain))) 2 @name right)
