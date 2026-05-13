; Bending Metal - compact tensor plate resonator for tinyseq.
; Smaller 6x6 variant of Examples/BendingMetal/main.swift.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))

(param gain @default 0.72 @min 0 @max 1)
(param strike @default 0.90 @min 0 @max 2)
(param tune @default 1.0 @min 0.25 @max 2.0)
(param base_tension @default 0.10 @min 0.02 @max 0.20)
(param damping @default 0.00005 @min 0.000001 @max 0.002)
(param tension_coupling @default 0.00025 @min 0 @max 0.0015)
(param bend_depth @default 0.024 @min 0 @max 0.06)
(param drive @default 1.15 @min 0.25 @max 8.0)

(make-tensor-history state @shape [6 6])
(make-tensor-history prev @shape [6 6])
(make-tensor-history tension @shape [6 6] @data [
  0.10 0.10 0.10 0.10 0.10 0.10
  0.10 0.10 0.10 0.10 0.10 0.10
  0.10 0.10 0.10 0.10 0.10 0.10
  0.10 0.10 0.10 0.10 0.10 0.10
  0.10 0.10 0.10 0.10 0.10 0.10
  0.10 0.10 0.10 0.10 0.10 0.10])

(def excitation-pattern (tensor @shape [6 6] @data [
  0 0 0 0 0 0
  0 0 0.8 0.5 0 0
  0 0 0.5 0.3 0 0
  0 0 0 0 0 0
  0 0 0 0 0 0
  0 0 0 0 0 0]))

(def horiz-grad (tensor @shape [6 6] @data [
  -1 -1 -1 -1 -1 -1
  -0.6 -0.6 -0.6 -0.6 -0.6 -0.6
  -0.2 -0.2 -0.2 -0.2 -0.2 -0.2
  0.2 0.2 0.2 0.2 0.2 0.2
  0.6 0.6 0.6 0.6 0.6 0.6
  1 1 1 1 1 1]))

(def diag-grad (tensor @shape [6 6] @data [
  -1 -0.8 -0.6 -0.4 -0.2 0
  -0.8 -0.6 -0.4 -0.2 0 0.2
  -0.6 -0.4 -0.2 0 0.2 0.4
  -0.4 -0.2 0 0.2 0.4 0.6
  -0.2 0 0.2 0.4 0.6 0.8
  0 0.2 0.4 0.6 0.8 1]))

(def pitch-ratio (clip (/ pitch 220) 0.25 2.4))
(def pitch-tension (clip (* base_tension tune pitch-ratio) 0.01 0.22))
(def gated-excite (* excitation-pattern trigger strike velocity))

(def bend-mod-1 (sin (* (phasor 0.31) twopi)))
(def bend-mod-2 (sin (* (phasor 0.19) twopi)))
(def bend-field
  (+ (* (* horiz-grad bend-mod-1) bend_depth)
     (* (* diag-grad bend-mod-2) bend_depth 0.6)))

(def state-t-raw (read-tensor-history state))
(def state-t-1 (read-tensor-history prev))
(def tension-t (read-tensor-history tension))
(def state-t (+ state-t-raw gated-excite))

(def laplacian-kernel (tensor @shape [3 3] @data [0 1 0 1 -4 1 0 1 0]))
(def laplacian (conv2d (pad state-t @padding [1:1 1:1]) laplacian-kernel))

(def two-minus-d (- 2 damping))
(def one-minus-d (- 1 damping))
(def effective-tension (max (min (* tension-t pitch-ratio tune) 0.24) 0.01))
(def state-next
  (+ (- (* state-t two-minus-d) (* state-t-1 one-minus-d))
     (* laplacian effective-tension)))

(def velocity-field (- state-t state-t-1))
(def local-energy (* velocity-field velocity-field))
(def relaxed (+ (* tension-t 0.9994) (* pitch-tension 0.0006)))
(def tension-unclamped
  (+ (+ relaxed (* local-energy tension_coupling)) bend-field))
(def tension-next (max (min tension-unclamped 0.24) 0.01))

(write-tensor-history prev state-t)
(write-tensor-history state state-next)
(write-tensor-history tension tension-next)

(def pickup-mask (tensor @shape [6 6] @data [
  0 0 0 0 0 0
  0 0 1 0 0 0
  0 0 0 0 0.8 0
  0 0 0 0 0 0
  0 0 0.7 0 0 0
  0 0 0 0 0 0]))

(def raw-out (sum (* state-next pickup-mask)))
(def saturated (tanh (* raw-out drive)))
(out (* saturated gain) 1 @name audio)
