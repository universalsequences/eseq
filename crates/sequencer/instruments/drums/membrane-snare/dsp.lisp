; Membrane Snare — full physical model, evolution of drums/membrane-kick.
; Three coupled FDTD subsystems, no noise oscillator in the core:
;   1. Batter head: 8x8 wave-equation membrane, struck by a lumped-mass stick
;      in Hertzian (pen^1.5) penalty contact — contact time shrinks with
;      velocity, so hard hits are brighter for free.
;   2. Resonant head: 8x8 membrane, coupled to the batter by a per-cell
;      relative spring (shell + air column; unconditionally passive).
;   3. Snare wires: 8 detuned 1D strings (rows of an 8x8 tensor, 1D
;      Laplacian) resting a strainer-controlled gap below the resonant head.
;      Contact is a hard unilateral constraint enforced by position
;      PROJECTION (w <= r + gap). The rattle is chaos from that collision
;      nonlinearity, not noise: soft hits stay tonal, hard hits throw the
;      wires clear and they slap back for hundreds of ms.
; Extra realism vs the kick: strike-driven tension modulation (pitch settles
; after hard hits), frequency-dependent damping (highs die faster), and
; off-center readout masks so asymmetric modes are audible. A contact-force
; output tap acts as a bottom "snare mic".
;
; ============================ COMPILER CONSTRAINTS ==========================
; dgen miscompiles several graph shapes at --max-frames > 1 (all verified by
; sample-exact A/B against --max-frames 1 builds; see minimal repros in the
; session that built this):
;   1. `biquad` anywhere in a patch that also has tensor feedback  -> output
;      filters are hand-rolled from scalar make-history (proven exact).
;   2. reading the SAME tensor history more than once              -> each
;      history is read exactly once and the def is shared.
;   3. (write-history h (sum <tensor>)) whose value feeds BACK into tensor
;      dynamics                                                    -> no
;      tensor reduce feeds dynamics; reduces only flow forward to output.
;      (This forced: rigid-plane striker, stick-driven bend, no air mode.)
; The membrane-kick's long-standing "self-oscillates at faithful defaults" is
; miscompilation class 1 (its exciter biquad): the kick patch is near-silent
; and well behaved when compiled at --max-frames 1.
; ============================================================================

; ── Host I/O ────────────────────────────────────────────────────────────────
(def gate (in 1 @name gate))
(def host_pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))

; ── Params ─────────────────────────────────────────────────────────────────
; releases are modal T60 in ms (decay of each linear mode, pitch-independent)
(param release @default 240 @min 20 @max 3000 @unit ms)      ; batter head T60
(param release2 @default 340 @min 20 @max 3000 @unit ms)     ; resonant head T60
; tune offsets host pitch; default maps A4/440 to ~190 Hz batter fundamental
(param tune @default -14.5 @min -48 @max 24 @unit st)
(param pitch2_ratio @default 1.35 @min 0.25 @max 4)          ; reso head vs batter
(param bend @default 0.5 @min 0 @max 4)                      ; strike pitch glide
; freq-dependent damping. SCALE WARNING: the viscous term adds per-sample
; damping ~ tone_damp * mu per mode (mu: 0.24 fundamental .. 7.8 grid), so
; 0.03 gives even the fundamental a ~40 ms T60 and kills the whole drum
; (measured). Keep small: 0.003 leaves the fundamental at the release T60
; while grid-scale modes die in ~20 ms.
(param tone_damp @default 0.003 @min 0 @max 0.02)
; striker (lumped mass, Hertz contact against a rigid strike plane)
(param stick_hard @default 0.004 @min 0.0002 @max 0.05)      ; contact stiffness
(param stick_speed @default 0.02 @min 0.002 @max 0.2)        ; impact speed @ vel=1
(param scrape @default 0.15 @min 0 @max 1)                   ; stick-surface roughness
; head-to-head coupling (relative spring per cell: shell + air column)
(param head_couple @default 0.6 @min 0 @max 2)
; snare wires
(param wire_pitch @default 620 @min 100 @max 2400 @unit Hz)  ; wire fundamental
(param wire_decay @default 420 @min 20 @max 3000 @unit ms)   ; wire T60
(param snare_tension @default 0.85 @min 0 @max 1)            ; strainer tightness
(param rattle @default 120 @min 0 @max 400)                  ; bed restitution
; wire_couple is physically a wire/head MASS RATIO — wires are far lighter
; than the head. Large values turn the rectified contact reaction into a DC
; ratchet that floats the head to the rails (measured at 0.35).
(param wire_couple @default 0.02 @min 0 @max 0.2)            ; wires -> reso head
(param snares @default 0.6 @min 0 @max 4)                    ; contact-mic level
(param bottom_mix @default 0.5 @min 0 @max 2)                ; reso-head mic level
; output body resonators (parallel band-pass peaks, hand-rolled) + level
(param body1_freq @default 190 @min 40 @max 2000 @unit Hz)
(param body1_gain @default 1.2 @min 0 @max 8)
(param body2_freq @default 340 @min 40 @max 4000 @unit Hz)
(param body2_gain @default 0.8 @min 0 @max 8)
(param body3_freq @default 1800 @min 100 @max 8000 @unit Hz)
(param body3_gain @default 0.5 @min 0 @max 8)
(param level @default 0.25 @min 0 @max 2)

; ── Feedback state ─────────────────────────────────────────────────────────
(make-tensor-history ph1 @shape [8 8])   ; batter head (t)
(make-tensor-history ph2 @shape [8 8])   ; batter head (t-1)
(make-tensor-history rh1 @shape [8 8])   ; resonant head (t)
(make-tensor-history rh2 @shape [8 8])   ; resonant head (t-1)
(make-tensor-history wh1 @shape [8 8])   ; wire bed (t)   — row = one wire
(make-tensor-history wh2 @shape [8 8])   ; wire bed (t-1)
(make-history stickx)                    ; striker position
(make-history stickv)                    ; striker velocity
(make-history bendenv)                   ; strike-force envelope (pitch glide)
; output-filter states (hand-rolled biquads; compiler constraint 1)
(make-history dcx1)
(make-history dcy1)
(make-history b1x1) (make-history b1x2) (make-history b1y1) (make-history b1y2)
(make-history b2x1) (make-history b2x2) (make-history b2y1) (make-history b2y2)
(make-history b3x1) (make-history b3x2) (make-history b3y1) (make-history b3y2)

; ── Kernels / masks ─────────────────────────────────────────────────────────
(def laplacian (tensor @shape [3 3] @data [0 1 0  1 -4 1  0 1 0]))
; 1D Laplacian along each row: wires are independent strings with fixed ends
; (zero-padded same conv supplies the fixed boundary)
(def wire-lap (tensor @shape [3 3] @data [0 0 0  1 -2 1  0 0 0]))

; 8x8 zero-padded membrane: mu_pq = 4 - 2cos(p pi/9) - 2cos(q pi/9)
(def mem-cos1 (cos (/ pi 9)))
(def mem-mu1 (- 4 (* 4 mem-cos1)))
(def mem-mu-max (+ 4 (* 4 mem-cos1)))
(def mem-max-pitch (* samplerate 0.05))
; 8-point string: mu_p = 2 - 2cos(p pi/9)
(def wire-mu1 (- 2 (* 2 mem-cos1)))
(def wire-mu-max (+ 2 (* 2 mem-cos1)))
(def wire-max-pitch (* samplerate 0.05))

; strike mask: smooth off-center bump (~row 2.5, col 4.5) — real hits are
; off-center, which excites the asymmetric modes that give a snare its bark
(def strike-mask (tensor @shape [8 8] @data [
  0.00 0.01 0.02 0.04 0.04 0.02 0.01 0.00
  0.01 0.04 0.12 0.22 0.22 0.12 0.04 0.01
  0.02 0.10 0.30 0.60 0.60 0.30 0.10 0.02
  0.02 0.12 0.36 0.72 0.72 0.36 0.12 0.02
  0.01 0.07 0.20 0.40 0.40 0.20 0.07 0.01
  0.01 0.03 0.08 0.16 0.16 0.08 0.03 0.01
  0.00 0.01 0.03 0.06 0.06 0.03 0.01 0.00
  0.00 0.00 0.01 0.02 0.02 0.01 0.00 0.00]))
; readout "mic positions": off-center smooth bumps, different spots per head
(def read-mask-p (tensor @shape [8 8] @data [
  0.00 0.00 0.00 0.00 0.00 0.00 0.00 0.00
  0.00 0.00 0.00 0.00 0.02 0.05 0.03 0.00
  0.00 0.00 0.00 0.03 0.10 0.22 0.12 0.02
  0.00 0.00 0.02 0.08 0.28 0.55 0.30 0.05
  0.00 0.00 0.03 0.10 0.35 0.70 0.38 0.06
  0.00 0.00 0.02 0.06 0.20 0.40 0.22 0.03
  0.00 0.00 0.00 0.02 0.07 0.14 0.08 0.01
  0.00 0.00 0.00 0.00 0.02 0.04 0.02 0.00]))
(def read-mask-r (tensor @shape [8 8] @data [
  0.00 0.02 0.05 0.03 0.00 0.00 0.00 0.00
  0.03 0.12 0.28 0.15 0.03 0.00 0.00 0.00
  0.06 0.30 0.65 0.35 0.07 0.00 0.00 0.00
  0.05 0.24 0.50 0.27 0.05 0.00 0.00 0.00
  0.02 0.10 0.20 0.11 0.02 0.00 0.00 0.00
  0.00 0.03 0.07 0.04 0.00 0.00 0.00 0.00
  0.00 0.00 0.02 0.01 0.00 0.00 0.00 0.00
  0.00 0.00 0.00 0.00 0.00 0.00 0.00 0.00]))
; per-wire stiffness detune (row-constant, ~±5% pitch spread => ~±10%
; stiffness). Identical wires phase-lock and sound metallic; the spread makes
; the rattle non-periodic.
(def wire-detune (tensor @shape [8 8] @data [
  0.906 0.906 0.906 0.906 0.906 0.906 0.906 0.906
  1.062 1.062 1.062 1.062 1.062 1.062 1.062 1.062
  0.951 0.951 0.951 0.951 0.951 0.951 0.951 0.951
  1.114 1.114 1.114 1.114 1.114 1.114 1.114 1.114
  0.874 0.874 0.874 0.874 0.874 0.874 0.874 0.874
  1.033 1.033 1.033 1.033 1.033 1.033 1.033 1.033
  0.982 0.982 0.982 0.982 0.982 0.982 0.982 0.982
  1.147 1.147 1.147 1.147 1.147 1.147 1.147 1.147]))

; ── Damping / stiffness maps ────────────────────────────────────────────────
(def p-damp (- 1 (exp (/ -13.8155106 (* samplerate (* (clip release 20 3000) 0.001))))))
(def r-damp (- 1 (exp (/ -13.8155106 (* samplerate (* (clip release2 20 3000) 0.001))))))
(def w-damp (- 1 (exp (/ -13.8155106 (* samplerate (* (clip wire_decay 20 3000) 0.001))))))

(def p-pitch-hz
  (clip (* host_pitch (exp (/ (* (log 2) tune) 12))) 1 mem-max-pitch))
(def p-sin (sin (* pi (/ p-pitch-hz samplerate))))
(def p-stiff-base (/ (* 4 p-sin p-sin) mem-mu1))
(def p-stiff-max (* 0.995 (/ (- 4 (* 2 p-damp)) mem-mu-max)))

(def r-pitch-hz (clip (* p-pitch-hz pitch2_ratio) 1 mem-max-pitch))
(def r-sin (sin (* pi (/ r-pitch-hz samplerate))))
(def r-stiff-base (/ (* 4 r-sin r-sin) mem-mu1))

(def w-pitch-hz (clip wire_pitch 1 wire-max-pitch))
(def w-sin (sin (* pi (/ w-pitch-hz samplerate))))
(def w-stiff-base (/ (* 4 w-sin w-sin) wire-mu1))
(def w-stiff-max (* 0.995 (/ (- 2 (* 2 w-damp)) wire-mu-max)))

; ── Read state (each history read EXACTLY once — compiler constraint 2) ─────
(def p-state (read-tensor-history ph1))
(def p-prev (read-tensor-history ph2))
(def r-state (read-tensor-history rh1))
(def r-prev (read-tensor-history rh2))
(def w-state (read-tensor-history wh1))
(def w-prev (read-tensor-history wh2))

; ── Striker: lumped mass in Hertz contact with a rigid strike plane ─────────
; position resets just above the plane at trigger, velocity resets downward
; with note velocity. The pen^1.5 contact decelerates the stick, so contact
; time (and brightness) falls naturally as velocity rises. The plane is rigid
; (head displacement does not feed back into pen): sensing the head requires
; a tensor reduce into same-frame dynamics, which is compiler constraint 3.
(def stick-x (gswitch trigger 0.0001 (read-history stickx)))
(def stick-v (gswitch trigger (* -1 (* stick_speed (+ 0.25 (* 0.75 velocity))))
                      (read-history stickv)))
(def stick-pen (max (* stick-x -1) 0))
(def stick-f (min (* stick_hard stick-pen (sqrt stick-pen)) 0.01))
(def stick-v-next (+ stick-v stick-f))
(def stick-x-next (+ stick-x stick-v-next))
(write-history stickx stick-x-next)
(write-history stickv stick-v-next)
; head sees the reaction, roughened by a little contact-noise while touching.
; gain staging is load-bearing: cell displacements must stay well under the
; ±3 NaN-clamps — a hard clamp on a Verlet scheme is an energy-GENERATING
; one-sided constraint and a railed head pumps a permanent limit cycle.
(def strike-force (* stick-f -1 (+ 1 (* scrape (noise))) 3))

; strike-driven tension envelope (pitch glide): peak-held stick force,
; ~150 ms settle. Scalar-only chain, so no tensor reduce feeds dynamics.
(def benv (max (* (read-history bendenv) 0.999) (* stick-f 60)))
(write-history bendenv benv)

; ── Head-to-head coupling: per-cell relative spring ─────────────────────────
; unconditionally passive; carries strike energy into the resonant head so
; the wires have something to rattle against. Scaled by the reso head's
; stiffness so coupling strength tracks tuning.
(def kd (* head_couple r-stiff-base 0.5))
(def couple-pr (* kd (- r-state p-state)))   ; force on batter
(def couple-rp (* kd (- p-state r-state)))   ; force on reso head

; ── Wire update + contact: position PROJECTION, not penalty forces ──────────
; wires rest a small GAP below the resonant head, anchored by their own
; pinned ends; the strainer knob shrinks the gap. Deliberately NO static
; preload force: a constant bias is a ratchet (bias feeds the wires, wires
; push the head, the stack climbs to the NaN rails — measured). With a gap
; the model is exactly silent at rest.
;
; Contact is the hard unilateral constraint w <= r + gap, enforced by
; projecting the wire update. Penalty contact failed in BOTH directions
; here (measured): stiff springs blow past the Verlet stability bound (the
; integrator generates energy), soft clamped springs act as a constant force
; that chase-accelerates the wires to ~10x head velocity so they tunnel
; units-deep THROUGH the head and slingshot into checkerboard rail states.
; Projection cannot tunnel and is dissipative by construction (impact is
; implicitly inelastic); `rattle` maps to restitution, and the wires' own
; elasticity supplies the rest of the bounce.
(def wire-gap (+ 0.0002 (* (- 1 snare_tension) 0.02)))
(def wire-restitution (min (* rattle 0.002) 0.85))
; cap the scalar before the detune multiply (largest detune row is 1.147):
; tensor-vs-signal min is unsupported, and this keeps every wire stable anyway
(def w-stiff (* (min w-stiff-base (/ w-stiff-max 1.147)) wire-detune))
(def w-lap (conv2d w-state wire-lap @padding same))
; viscosity: per-cell projection is non-smooth and pumps grid-scale
; (checkerboard) modes into the bed; velocity-Laplacian damping kills exactly
; those without touching the wires' musical low modes
; 0.015: checkerboard T60 ~12 ms, wire fundamental keeps most of wire_decay
(def w-visc (* 0.015 (conv2d (- w-state w-prev) wire-lap @padding same)))
(def w-next-free (+ (- (* (- 2 w-damp) w-state) (* (- 1 w-damp) w-prev))
                    (* w-lap w-stiff)
                    w-visc))
(def w-limit (+ r-state wire-gap))
(def w-overshoot (max (- w-next-free w-limit) 0))   ; constraint violation
; reflect about the head plane with restitution e: w -> limit - e*overshoot
(def w-next (- w-next-free (* w-overshoot (+ 1 wire-restitution))))
(def w-nextc (max (min w-next 3) -3))
; reaction on the head: the projection correction is the impulse the head
; absorbed, scaled by the wire/head mass ratio. The cap is load-bearing, not
; NaN-safety: during the strike "scoop" (head carrying the bed down for
; hundreds of samples) the rectified reaction is a DC ratchet, and
; cap*wire_couple must stay below the head's restoring force at small
; displacement or the head floats up to the rails.
(def contact-f (min (* w-overshoot (+ 1 wire-restitution)) 0.005))

; ── Batter head update ──────────────────────────────────────────────────────
(def p-stiff (min (* p-stiff-base (+ 1 (* bend benv))) p-stiff-max))
(def p-lap (conv2d p-state laplacian @padding same))
(def p-visc (* tone_damp (conv2d (- p-state p-prev) laplacian @padding same)))
(def p-next (+ (- (* (- 2 p-damp) p-state) (* (- 1 p-damp) p-prev))
               (* p-lap p-stiff)
               p-visc
               (* strike-force strike-mask)
               couple-pr))
(def p-nextc (max (min p-next 3) -3))   ; NaN-safety clamp only

; ── Resonant head update ────────────────────────────────────────────────────
(def r-lap (conv2d r-state laplacian @padding same))
(def r-visc (* (* tone_damp 0.5) (conv2d (- r-state r-prev) laplacian @padding same)))
(def r-next (+ (- (* (- 2 r-damp) r-state) (* (- 1 r-damp) r-prev))
               (* r-lap r-stiff-base)
               r-visc
               couple-rp
               (* contact-f wire_couple)))   ; wires slap the head back
(def r-nextc (max (min r-next 3) -3))

; ── Output taps (forward-only reduces; computed before the history writes) ──
(def mic-top (sum (* p-nextc read-mask-p)))
(def mic-bot (sum (* r-nextc read-mask-r)))
(def mic-snap (* (sum contact-f) 15))
(def mixdown (+ mic-top (* mic-bot bottom_mix) (* mic-snap snares)))

; ── Write feedback ──────────────────────────────────────────────────────────
(write-tensor-history ph2 p-state)
(write-tensor-history ph1 p-nextc)
(write-tensor-history rh2 r-state)
(write-tensor-history rh1 r-nextc)
(write-tensor-history wh2 w-state)
(write-tensor-history wh1 w-nextc)

; ── Body EQ + DC block (hand-rolled scalar-history filters) ─────────────────
; RBJ constant-skirt band-pass resonators in parallel with the dry path.
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

(def bq1 (bpf mixdown body1_freq 4 b1x1 b1x2 b1y1 b1y2))
(def bq2 (bpf mixdown body2_freq 3 b2x1 b2x2 b2y1 b2y2))
(def bq3 (bpf mixdown body3_freq 2 b3x1 b3x2 b3y1 b3y2))
(def bodied (+ mixdown (* bq1 body1_gain) (* bq2 body2_gain) (* bq3 body3_gain)))
; one-pole-zero DC blocker
(def dcin bodied)
(def dcy (+ (- dcin (read-history dcx1)) (* 0.998 (read-history dcy1))))
(write-history dcx1 dcin)
(write-history dcy1 dcy)
(out (* dcy level) 1 @name audio)
