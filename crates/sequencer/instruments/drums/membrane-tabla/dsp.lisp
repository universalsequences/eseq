; Membrane Tabla — hand-struck single-head sibling of drums/membrane-snare-rim.
; One 8x8 FDTD head with a SYAHI: a per-cell mass-loading map over the center
; of the membrane. The syahi paste is what makes a tabla a tabla — center
; loading pulls the drum's inharmonic modes toward HARMONIC ratios (Raman),
; which is why a dayan sings a pitch instead of thudding. Loading = per-cell
; force-to-acceleration divisor (variable mass, uniform tension): stiffness,
; strike force AND viscosity all divide by cell mass — an unscaled viscosity
; over-damps the loaded sur mode up to 25x and mutes the drum's voice.
; MEASURED partials at defaults: 1.00 : 1.99 : 2.74 : 3.03 (the Raman series).
; `syahi` morphs continuously from plain membrane (0) to overloaded (1.5).
;
; Stroke physics (all plockable — this instrument is built to be p-locked):
;   stroke 0..1 morphs TE -> TUN -> NA:
;   - te/ti side (0): closed stroke ON the syahi — strike mask slides to the
;     loaded center, fingertip stays pressed for ~150 ms (peak-held benv
;     damping over the palm mask) — a dry, pitched tap.
;   - tun (0.5): open resonant stroke between syahi and rim; full ring.
;   - na side (1): index finger strikes the kinar ring while another finger
;     RESTS on the sur mode's NODAL LINE (na-ring-mask documents the measured
;     loaded mode shapes). Chokes the fundamental hum ~50 dB under the sur and
;     leaves it singing ~400 ms — the classic "na".
;   press 0..1 is bayan heel pressure: raises per-cell stiffness (pitch gliss,
;   range set by gliss_range) with only light damping under the heel — the
;   gliss stays resonant, unlike a mute. damp 0..1 is the actual mute/choke.
;
; All of membrane-snare's stability machinery is preserved verbatim: no static
; preload, rectified caps, ±3 NaN clamps as safety only, hand-rolled scalar
; filters, per-cell stiffness always <= the scalar leapfrog bound (the syahi
; map only DIVIDES stiffness, so the unloaded-cell bound still dominates).

; ── Host I/O ────────────────────────────────────────────────────────────────
(def gate (in 1 @name gate))
(def host_pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))
(def vel (clip velocity 0 1))
(def vel-gain (sqrt vel))

; ── Params ─────────────────────────────────────────────────────────────────
(param release @default 700 @min 20 @max 4000 @unit ms @mod true @mod-mode additive @mod-depth-min -1000 @mod-depth-max 1000 @mod-unit ms)
; tune offsets host pitch; default calibrated so the host note lands on the
; drum's SUNG pitch (the harmonic 2nd partial — the sur) at syahi 1
(param tune @default 5 @min -48 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param bend @default 0.2 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)          ; strike pitch glide
; freq-dependent damping. SCALE WARNING: see membrane-snare — keep small.
(param tone_damp @default 0.002 @min 0 @max 0.02)
; syahi loading: 0 = bare membrane (inharmonic thud), 1 = full paste (sings)
(param syahi @default 1 @min 0 @max 1.5 @mod true @mod-mode additive @mod-depth-min -1.5 @mod-depth-max 1.5)
; finger (lumped mass, Hertz contact — softer + longer than a stick tip)
(param finger_hard @default 0.004 @min 0.0002 @max 0.05 @mod true @mod-mode additive @mod-depth-min -0.01 @mod-depth-max 0.01)
(param finger_speed @default 0.02 @min 0.002 @max 0.2 @mod true @mod-mode additive @mod-depth-min -0.05 @mod-depth-max 0.05)
(param scrape @default 0.05 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; ── stroke expression ──
; 0 = te (closed, on syahi), 0.5 = tun (open), 1 = na (edge, ringing overtone)
(param stroke @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; bayan heel pressure: pitch gliss, stays resonant
(param press @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; how far full press bends: stiffness multiplier 1+gliss_range (0.8 ~ +5 st)
(param gliss_range @default 0.4 @min 0 @max 0.8 @mod true @mod-mode additive @mod-depth-min -0.8 @mod-depth-max 0.8)
; palm mute / choke (separate from press so glisses don't choke)
(param damp @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; mic position: 0 = over the syahi (full, dark), 1 = at the kinar (airy)
(param mic_blend @default 0.35 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; kettle resonators (parallel band-pass peaks, hand-rolled) + level
(param body1_freq @default 250 @min 40 @max 2000 @unit Hz @mod true @mod-mode additive @mod-depth-min -1000 @mod-depth-max 1000 @mod-unit Hz)
(param body1_gain @default 1.0 @min 0 @max 8 @mod true @mod-mode additive @mod-depth-min -4 @mod-depth-max 4)
(param body2_freq @default 520 @min 40 @max 4000 @unit Hz @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit Hz)
(param body2_gain @default 0.6 @min 0 @max 8 @mod true @mod-mode additive @mod-depth-min -4 @mod-depth-max 4)
(param body3_freq @default 1400 @min 100 @max 8000 @unit Hz @mod true @mod-mode additive @mod-depth-min -4000 @mod-depth-max 4000 @mod-unit Hz)
(param body3_gain @default 0.3 @min 0 @max 8 @mod true @mod-mode additive @mod-depth-min -4 @mod-depth-max 4)
(param level @default 0.3 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

; Clipped modulation targets — feedback-path params range-limited at use sites
(def release-v (clip (mod release) 20 4000))
(def tune-v (clip (mod tune) -48 24))
(def bend-v (clip (mod bend) 0 4))
(def syahi-v (clip (mod syahi) 0 1.5))
(def finger-hard-v (clip (mod finger_hard) 0.0002 0.05))
(def finger-speed-v (clip (mod finger_speed) 0.002 0.2))
(def scrape-v (clip (mod scrape) 0 1))
(def stroke-v (clip (mod stroke) 0 1))
(def press-v (clip (mod press) 0 1))
(def gliss-range-v (clip (mod gliss_range) 0 0.8))
(def damp-v (clip (mod damp) 0 1))
(def mic-blend-v (clip (mod mic_blend) 0 1))
(def body1-freq-v (clip (mod body1_freq) 40 2000))
(def body1-gain-v (clip (mod body1_gain) 0 8))
(def body2-freq-v (clip (mod body2_freq) 40 4000))
(def body2-gain-v (clip (mod body2_gain) 0 8))
(def body3-freq-v (clip (mod body3_freq) 100 8000))
(def body3-gain-v (clip (mod body3_gain) 0 8))
(def level-v (clip (mod level) 0 2))

; stroke morph legs: below 0.5 fades in the closed te stroke, above the na.
(def na-mix (clip (* (- stroke-v 0.5) 2) 0 1))
(def te-mix (clip (* (- 0.5 stroke-v) 2) 0 1))

; ── Feedback state ─────────────────────────────────────────────────────────
(make-tensor-history ph1 @shape [8 8])   ; head (t)
(make-tensor-history ph2 @shape [8 8])   ; head (t-1)
(make-history fingx)                     ; finger position
(make-history fingv)                     ; finger velocity
(make-history bendenv)                   ; strike-force envelope (glide + te hold)
; output-filter states (hand-rolled biquads; see membrane-snare header)
(make-history dcx1)
(make-history dcy1)
(make-history b1x1) (make-history b1x2) (make-history b1y1) (make-history b1y2)
(make-history b2x1) (make-history b2x2) (make-history b2y1) (make-history b2y2)
(make-history b3x1) (make-history b3x2) (make-history b3y1) (make-history b3y2)

; ── Kernels / masks ─────────────────────────────────────────────────────────
(def laplacian (tensor @shape [3 3] @data [0 1 0  1 -4 1  0 1 0]))

; 8x8 zero-padded membrane: mu_pq = 4 - 2cos(p pi/9) - 2cos(q pi/9)
; (8x8 instead of the rim's 6x6: the syahi needs grid resolution — on 6x6 the
; loaded 2nd-partial ratio saturated at ~1.77, measurably short of harmonic)
(def mem-cos1 (cos (/ pi 9)))
(def mem-mu1 (- 4 (* 4 mem-cos1)))
(def mem-mu-max (+ 4 (* 4 mem-cos1)))
(def mem-max-pitch (* samplerate 0.05))

; syahi loading map: per-cell stiffness CUT at syahi = 1 (full paste),
; precomputed as 1 - 1/m for a radial mass bump (center ~12x skin mass,
; tapering) covering ~40% of the head. Runtime tensor DIVISION miscompiles
; inside the fused FDTD block — measured frame-0-only evaluation — so the
; reciprocal is baked in here. syahi > 1 overloads linearly; the floor clamp
; at the use site keeps every cell inside the stability bound regardless.
(def syahi-delta (tensor @shape [8 8] @data [
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.400 0.400 0.000 0.000 0.000
  0.000 0.000 0.660 0.875 0.875 0.660 0.000 0.000
  0.000 0.400 0.875 0.940 0.940 0.875 0.400 0.000
  0.000 0.400 0.875 0.940 0.940 0.875 0.400 0.000
  0.000 0.000 0.660 0.875 0.875 0.660 0.000 0.000
  0.000 0.000 0.000 0.400 0.400 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000]))

; open strike position (tun): between syahi border and kinar. Compact — a
; broad strike bump projects mostly onto the fundamental and buries the
; overtone family that carries the tabla's voice (measured on 6x6).
(def strike-mask
  (tensor-param @shape [8 8] @name strike_mask @default-file "strike-mask.json"))
; te strike position: on the syahi. Sum-normalized to the open mask (~5.7).
(def center-mask (tensor @shape [8 8] @data [
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.180 0.180 0.000 0.000 0.000
  0.000 0.000 0.180 1.050 1.050 0.180 0.000 0.000
  0.000 0.000 0.180 1.050 1.050 0.180 0.000 0.000
  0.000 0.000 0.000 0.180 0.180 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000]))
; na strike position: the kinar ring at ~80% radius (NOT flush against the
; pinned boundary — cells there can't excite the sur mode at all; measured
; only >1.2 kHz clatter). Weak fundamental, strong sur + brightness. Sum ~5.7.
(def edge-mask (tensor @shape [8 8] @data [
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.054 0.135 0.045
  0.000 0.000 0.000 0.000 0.045 0.315 0.630 0.135
  0.000 0.000 0.000 0.000 0.081 0.540 0.945 0.198
  0.000 0.000 0.000 0.000 0.063 0.450 0.810 0.162
  0.000 0.000 0.000 0.000 0.027 0.225 0.450 0.090
  0.000 0.000 0.000 0.000 0.000 0.072 0.180 0.054
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000]))
; na damping mask: the resting finger sits on the SUR MODE'S NODAL LINE.
; MEASURED loaded mode shapes (per-cell FFT of ph1, default strike position):
; the fundamental peaks at center; the sur ROCKS THE PLUG (center amplitude
; 0.66-0.93 — center damping kills both, tried and failed) but is near-zero
; along the tilted node path (1,4)-(2,4)-...-(5,3)-(6,3), where the f0/sur
; amplitude ratio is 7-22x. Damping there chokes the hum, spares the sur.
; NOTE: the sur's orientation follows the strike azimuth — this mask matches
; the default right-side strike masks; a heavily repainted strike_mask can
; rotate the sur off this node line and weaken the na choke.
(def na-ring-mask (tensor @shape [8 8] @data [
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.450 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.700 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.700 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.450 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000]))
; palm mute mask (damp/choke + the held te fingertip): broad, syahi-centered
(def mute-mask (tensor @shape [8 8] @data [
  0.010 0.020 0.040 0.060 0.060 0.040 0.020 0.010
  0.020 0.060 0.150 0.220 0.220 0.150 0.060 0.020
  0.040 0.150 0.350 0.500 0.500 0.350 0.150 0.040
  0.060 0.220 0.500 0.700 0.700 0.500 0.220 0.060
  0.060 0.220 0.500 0.700 0.700 0.500 0.220 0.060
  0.040 0.150 0.350 0.500 0.500 0.350 0.150 0.040
  0.020 0.060 0.150 0.220 0.220 0.150 0.060 0.020
  0.010 0.020 0.040 0.060 0.060 0.040 0.020 0.010]))
; heel-of-hand mask for press: bump on the left side — light damping only,
; the gliss itself is the stiffness raise
(def heel-mask (tensor @shape [8 8] @data [
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.050 0.150 0.050 0.000 0.000 0.000 0.000 0.000
  0.150 0.400 0.150 0.000 0.000 0.000 0.000 0.000
  0.250 0.600 0.250 0.000 0.000 0.000 0.000 0.000
  0.250 0.600 0.250 0.000 0.000 0.000 0.000 0.000
  0.150 0.400 0.150 0.000 0.000 0.000 0.000 0.000
  0.050 0.150 0.050 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000]))
; mic positions. COMPACT on purpose: a broad read bump is a spatial lowpass
; that projects almost entirely onto the fundamental (measured -57 dB
; overtones with the membrane-snare-style masks). A near-point pickup keeps
; the (2,1)/(2,2) family audible — that family carries the tabla's voice.
; sited on the sur-mode ANTINODE (rows 3-4, cols 1-2): the (2,1) overtone
; reads at full strength there while the fundamental is at ~0.64 — a mic over
; the head center sits on the sur's node and buries it (measured -36 dB).
(def read-mask-c (tensor @shape [8 8] @data [
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.050 0.040 0.000 0.000 0.000 0.000 0.000
  0.000 0.450 0.300 0.040 0.000 0.000 0.000 0.000
  0.000 0.450 0.300 0.040 0.000 0.000 0.000 0.000
  0.000 0.050 0.040 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000]))
(def read-mask-e (tensor @shape [8 8] @data [
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.050 0.180
  0.000 0.000 0.000 0.000 0.000 0.000 0.120 0.500
  0.000 0.000 0.000 0.000 0.000 0.000 0.100 0.400
  0.000 0.000 0.000 0.000 0.000 0.000 0.030 0.120
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000
  0.000 0.000 0.000 0.000 0.000 0.000 0.000 0.000]))

; ── Damping / stiffness ─────────────────────────────────────────────────────
(def p-damp (- 1 (exp (/ -13.8155106 (* samplerate (* release-v 0.001))))))

(def p-pitch-hz
  (clip (* host_pitch (exp (/ (* (log 2) tune-v) 12))) 1 mem-max-pitch))
(def p-sin (sin (* pi (/ p-pitch-hz samplerate))))
(def p-stiff-base (/ (* 4 p-sin p-sin) mem-mu1))
; stiffness bound uses the SCALAR base p-damp: masks only ADD damping on top,
; and more damping only widens the true stability region — conservative.
(def p-stiff-max (* 0.995 (/ (- 4 (* 2 p-damp)) mem-mu-max)))

; ── Read state (each history read EXACTLY once) ─────────────────────────────
(def p-state (read-tensor-history ph1))
(def p-prev (read-tensor-history ph2))

; ── Finger: lumped mass in Hertz contact with a rigid strike plane ──────────
; stroke reshapes the finger BEFORE the physics: te and na are crisp
; fingertip strokes (stiffer contact = brighter snap), tun is the fuller open
; stroke. Contact time, brightness, level all fall out of the Hertz model.
(def finger-hard-eff
  (clip (* finger-hard-v (+ 1 (* te-mix 1.5) (* na-mix 1.5))) 0.0002 0.05))
(def finger-x (gswitch trigger 0.0001 (read-history fingx)))
(def finger-v (gswitch trigger (* -1 (* finger-speed-v vel-gain))
                       (read-history fingv)))
(def finger-pen (max (* finger-x -1) 0))
(def finger-f (min (* finger-hard-eff finger-pen (sqrt finger-pen)) 0.01))
(def finger-v-next (+ finger-v finger-f))
(def finger-x-next (+ finger-x finger-v-next))
(write-history fingx finger-x-next)
(write-history fingv finger-v-next)
; head sees the reaction, roughened by skin-contact noise while touching.
; gain staging is load-bearing: cell displacements stay well under ±3.
(def strike-force (* finger-f -1 (+ 1 (* scrape-v (noise))) 3))
; three-position strike morph: te (syahi) <- tun (open) -> na (kinar)
(def strike-mask-m (+ (* strike-mask (- 1 te-mix na-mix))
                      (* center-mask te-mix)
                      (* edge-mask na-mix)))

; strike-driven tension envelope: peak-held finger force, ~150 ms settle.
; Doubles as the te fingertip-hold damping envelope below. Scalar-only chain.
(def benv (max (* (read-history bendenv) 0.999) (* finger-f 60)))
(write-history bendenv benv)

; ── Per-cell damping: mute + te hold + na ring + heel touch ────────────────
; te fingertip stays on the syahi ~150 ms after the stroke (benv peak-hold);
; damp is the user choke; na ring is stationary while stroke sits at na.
(def mute-total (clip (+ damp-v (* te-mix (min benv 1) 0.9)) 0 1))
(def p-damp-t (min (+ p-damp
                      (* mute-total mute-mask 0.12)
                      (* na-mix na-ring-mask 0.1)
                      (* press-v heel-mask 0.015))
                   0.5))

; ── Head update: syahi-loaded FDTD ─────────────────────────────────────────
; press raises pitch (bayan gliss) on top of the strike glide; the scalar
; stiffness is bounded FIRST, then divided by per-cell syahi mass (<= 1), so
; every cell stays inside the leapfrog bound for all param/mod values.
(def p-stiff-scalar (min (* p-stiff-base
                            (+ 1 (* bend-v benv))
                            (+ 1 (* press-v gliss-range-v)))
                         p-stiff-max))
; NOTE: tensor products in the feedback block are kept strictly BINARY —
; (* scalar tensor tensor) in one call miscompiled to zero inside the fused
; FDTD cluster (measured; standalone taps of the same expression were fine).
(def syahi-scale (max (- 1 (* syahi-v syahi-delta)) 0.04))
(def p-stiff-t (* p-stiff-scalar syahi-scale))
(def inject-mask (* strike-mask-m syahi-scale))
(def p-lap (conv2d p-state laplacian @padding same))
; viscosity is a FORCE — it must divide by cell mass like every other force.
; Unscaled, it runs up to 25x too strong inside the syahi and murders the sur
; overtone (measured -35 dB with, -3 dB without).
(def p-visc (* (* tone_damp syahi-scale)
               (conv2d (- p-state p-prev) laplacian @padding same)))
(def p-next (+ (- (* (- 2 p-damp-t) p-state) (* (- 1 p-damp-t) p-prev))
               (* p-lap p-stiff-t)
               p-visc
               (* strike-force inject-mask)))
(def p-nextc (max (min p-next 3) -3))   ; NaN-safety clamp only

; ── Output taps (forward-only reduces; computed before the history writes) ──
(def mic-c (sum (* p-nextc read-mask-c)))
(def mic-e (sum (* p-nextc read-mask-e)))
; x8 makeup: the near-point mic masks pick up ~10x less than broad bumps
(def mixdown (* (+ (* mic-c (- 1 mic-blend-v)) (* mic-e mic-blend-v)) 8))

; ── Write feedback ──────────────────────────────────────────────────────────
(write-tensor-history ph2 p-state)
(write-tensor-history ph1 p-nextc)

; ── Kettle EQ + DC block (hand-rolled scalar-history filters) ───────────────
(defmacro bpf (x f q x1h x2h y1h y2h)
  (def w0 (* twopi (/ (clip f 20 20000) samplerate)))
  (def alpha (/ (sin w0) (* 2 q)))
  (def a0 (+ 1 alpha))
  (def x1v (read-history x1h))
  (def x2v (read-history x2h))
  (def y1v (read-history y1h))
  (def y2v (read-history y2h))
  (def y (/ (+ (* alpha x) (* -1 alpha x2v)
               (* 2 (cos w0) y1v) (* -1 (- 1 alpha) y2v))
            a0))
  (write-history x2h x1v)
  (write-history x1h x)
  (write-history y2h y1v)
  (write-history y1h y)
  y)

(def bq1 (bpf mixdown body1-freq-v 4 b1x1 b1x2 b1y1 b1y2))
(def bq2 (bpf mixdown body2-freq-v 3 b2x1 b2x2 b2y1 b2y2))
(def bq3 (bpf mixdown body3-freq-v 2 b3x1 b3x2 b3y1 b3y2))
(def bodied (+ mixdown (* bq1 body1-gain-v) (* bq2 body2-gain-v) (* bq3 body3-gain-v)))
; one-pole-zero DC blocker
(def dcin bodied)
(def dcy (+ (- dcin (read-history dcx1)) (* 0.998 (read-history dcy1))))
(write-history dcx1 dcin)
(write-history dcy1 dcy)
(out (* dcy level-v vel-gain) 1 @name audio)
