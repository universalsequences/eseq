; Membrane Snare Rim — stroke-expressive sibling of drums/membrane-snare.
; Same physical core (two 6x6 FDTD heads, lumped-mass Hertz striker, 6
; collision-modeled wires under the reso head), plus stroke physics:
;   stroke 0..1 morphs GHOST -> OPEN -> RIMSHOT with real injection physics:
;   - ghost side: lower stick height / relaxed grip (slower, softer impact,
;     so the Hertz contact time lengthens and the hit darkens naturally)
;   - rimshot side: the strike mask slides to the head EDGE (weak fundamental,
;     strong high asymmetric modes), the stick hardens (shorter contact =
;     crack), the metal HOOP rings (3 inharmonic modal resonators struck by
;     the same stick impulse), and the stick pivots pressed into the head for
;     ~150 ms (peak-held damping envelope) — crack, not boom.
;   press 0..1 is a hand laid on the head: per-cell damping bump (palm mask)
;   plus a small skin-pressure pitch raise. Wire choke falls out for free
;   (damped head passes less energy to the reso head, so less buzz).
;   Cross-stick = stroke 1 + press high + head mics low (see presets).
;
; All of membrane-snare's stability machinery is preserved verbatim: wire
; contact by position PROJECTION (never penalty), no static preload, rectified
; contact-reaction caps, ±3 NaN clamps as safety only, hand-rolled scalar
; filters. See that file's header for the measured failure modes behind each.

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
; Velocity participates in the physical strike and the final loudness. The
; sqrt curve keeps mid velocities playable while velocity 0 injects no state
; energy.
(def vel (clip velocity 0 1))
(def vel-gain (sqrt vel))

; ── Params ─────────────────────────────────────────────────────────────────
; releases are modal T60 in ms (decay of each linear mode, pitch-independent)
(param release @default 240 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)      ; batter head T60
(param release2 @default 340 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)     ; resonant head T60
; tune offsets host pitch; default maps A4/440 to ~190 Hz batter fundamental
(param tune @default -14.5 @min -48 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param pitch2_ratio @default 1.35 @min 0.25 @max 4 @mod true @mod-mode additive @mod-depth-min -1.5 @mod-depth-max 1.5)          ; reso head vs batter
(param bend @default 0.5 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)                      ; strike pitch glide
; freq-dependent damping. SCALE WARNING: see membrane-snare — keep small.
(param tone_damp @default 0.003 @min 0 @max 0.02)
; striker (lumped mass, Hertz contact against a rigid strike plane)
(param stick_hard @default 0.004 @min 0.0002 @max 0.05 @mod true @mod-mode additive @mod-depth-min -0.01 @mod-depth-max 0.01)      ; contact stiffness
(param stick_speed @default 0.02 @min 0.002 @max 0.2 @mod true @mod-mode additive @mod-depth-min -0.05 @mod-depth-max 0.05)        ; impact speed @ vel=1
(param scrape @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)                   ; stick-surface roughness
; ── stroke expression ──
; 0 = ghost note, 0.5 = open hit (matches membrane-snare), 1 = rimshot
(param stroke @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; hand laid on the head: localized damping + slight skin-pressure pitch raise
(param press @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; metal hoop: 3 inharmonic partials (ring flexural ratios 1 : 2.76 : 5.40)
(param rim_pitch @default 2200 @min 400 @max 8000 @unit Hz @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit Hz)
(param rim_decay @default 300 @min 20 @max 1500 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)
(param rim_level @default 1.2 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
; hoop ring re-injected into the head at the edge. Cap is load-bearing: the
; hoop is one-way driven (head does not feed it back) so this is a bounded
; decaying force, but it must stay far below the head's restoring force.
(param rim_drive @default 0.001 @min 0 @max 0.005)
; head-to-head coupling (relative spring per cell: shell + air column)
(param head_couple @default 0.6 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; snare wires
(param wire_pitch @default 620 @min 100 @max 2400 @unit Hz @mod true @mod-mode additive @mod-depth-min -800 @mod-depth-max 800 @mod-unit Hz)  ; wire fundamental
(param wire_decay @default 420 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -1000 @mod-depth-max 1000 @mod-unit ms)   ; wire T60
(param snare_tension @default 0.85 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)            ; strainer tightness
(param rattle @default 120 @min 0 @max 400 @mod true @mod-mode additive @mod-depth-min -200 @mod-depth-max 200)                  ; bed restitution
; wire_couple is physically a wire/head MASS RATIO — see membrane-snare.
(param wire_couple @default 0.02 @min 0 @max 0.2)            ; wires -> reso head
(param snares @default 0.6 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)                    ; contact-mic level
(param bottom_mix @default 0.5 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)                ; reso-head mic level
; output body resonators (parallel band-pass peaks, hand-rolled) + level
(param body1_freq @default 190 @min 40 @max 2000 @unit Hz @mod true @mod-mode additive @mod-depth-min -1000 @mod-depth-max 1000 @mod-unit Hz)
(param body1_gain @default 1.2 @min 0 @max 8 @mod true @mod-mode additive @mod-depth-min -4 @mod-depth-max 4)
(param body2_freq @default 340 @min 40 @max 4000 @unit Hz @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit Hz)
(param body2_gain @default 0.8 @min 0 @max 8 @mod true @mod-mode additive @mod-depth-min -4 @mod-depth-max 4)
(param body3_freq @default 1800 @min 100 @max 8000 @unit Hz @mod true @mod-mode additive @mod-depth-min -4000 @mod-depth-max 4000 @mod-unit Hz)
(param body3_gain @default 0.5 @min 0 @max 8 @mod true @mod-mode additive @mod-depth-min -4 @mod-depth-max 4)
(param level @default 0.25 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

; Clipped modulation targets. All feedback-path params are range-limited at
; their exact use sites so active modulation cannot exceed the tested bounds.
(def release-v (clip (mod release) 20 3000))
(def release2-v (clip (mod release2) 20 3000))
(def tune-v (clip (mod tune) -48 24))
(def pitch2-ratio-v (clip (mod pitch2_ratio) 0.25 4))
(def bend-v (clip (mod bend) 0 4))
(def stick-hard-v (clip (mod stick_hard) 0.0002 0.05))
(def stick-speed-v (clip (mod stick_speed) 0.002 0.2))
(def scrape-v (clip (mod scrape) 0 1))
(def stroke-v (clip (mod stroke) 0 1))
(def press-v (clip (mod press) 0 1))
(def rim-pitch-v (clip (mod rim_pitch) 400 8000))
(def rim-decay-v (clip (mod rim_decay) 20 1500))
(def rim-level-v (clip (mod rim_level) 0 4))
(def rim-drive-v (clip rim_drive 0 0.005))
(def head-couple-v (clip (mod head_couple) 0 2))
(def wire-pitch-v (clip (mod wire_pitch) 100 2400))
(def wire-decay-v (clip (mod wire_decay) 20 3000))
(def snare-tension-v (clip (mod snare_tension) 0 1))
(def rattle-v (clip (mod rattle) 0 400))
(def snares-v (clip (mod snares) 0 4))
(def bottom-mix-v (clip (mod bottom_mix) 0 2))
(def body1-freq-v (clip (mod body1_freq) 40 2000))
(def body1-gain-v (clip (mod body1_gain) 0 8))
(def body2-freq-v (clip (mod body2_freq) 40 4000))
(def body2-gain-v (clip (mod body2_gain) 0 8))
(def body3-freq-v (clip (mod body3_freq) 100 8000))
(def body3-gain-v (clip (mod body3_gain) 0 8))
(def level-v (clip (mod level) 0 2))

; stroke morph legs: below 0.5 fades in the ghost stroke, above fades in the
; rimshot. At exactly 0.5 both are zero and the drum IS membrane-snare.
(def edge-mix (clip (* (- stroke-v 0.5) 2) 0 1))
(def ghost-mix (clip (* (- 0.5 stroke-v) 2) 0 1))

; ── Feedback state ─────────────────────────────────────────────────────────
(make-tensor-history ph1 @shape [6 6])   ; batter head (t)
(make-tensor-history ph2 @shape [6 6])   ; batter head (t-1)
(make-tensor-history rh1 @shape [6 6])   ; resonant head (t)
(make-tensor-history rh2 @shape [6 6])   ; resonant head (t-1)
(make-tensor-history wh1 @shape [6 6])   ; wire bed (t)   — row = one wire
(make-tensor-history wh2 @shape [6 6])   ; wire bed (t-1)
(make-history stickx)                    ; striker position
(make-history stickv)                    ; striker velocity
(make-history bendenv)                   ; strike-force envelope (pitch glide + rimshot press)
; rim hoop modal states (2 per partial)
(make-history rm1y1) (make-history rm1y2)
(make-history rm2y1) (make-history rm2y2)
(make-history rm3y1) (make-history rm3y2)
; output-filter states (hand-rolled biquads; see membrane-snare header)
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

; 6x6 zero-padded membrane: mu_pq = 4 - 2cos(p pi/7) - 2cos(q pi/7)
(def mem-cos1 (cos (/ pi 7)))
(def mem-mu1 (- 4 (* 4 mem-cos1)))
(def mem-mu-max (+ 4 (* 4 mem-cos1)))
(def mem-max-pitch (* samplerate 0.05))
; 6-point string: mu_p = 2 - 2cos(p pi/7)
(def wire-mu1 (- 2 (* 2 mem-cos1)))
(def wire-mu-max (+ 2 (* 2 mem-cos1)))
(def wire-max-pitch (* samplerate 0.05))

; strike mask: smooth off-center bump — the OPEN strike position
(def strike-mask
  (tensor-param @shape [6 6] @name strike_mask @default-file "strike-mask.json"))
; RIM strike position: hugging the boundary (right edge). Zero-padded edge
; cells are heavily pinned, so this projects weakly onto the fundamental and
; strongly onto the high asymmetric modes — the rimshot "bark". Sum-normalized
; to the default open mask (~5.7) so loudness doesn't jump across the morph.
(def edge-mask (tensor @shape [6 6] @data [
  0.000 0.000 0.000 0.000 0.091 0.228
  0.000 0.000 0.000 0.046 0.319 0.820
  0.000 0.000 0.000 0.073 0.501 1.184
  0.000 0.000 0.000 0.055 0.410 0.957
  0.000 0.000 0.000 0.018 0.182 0.501
  0.000 0.000 0.000 0.000 0.073 0.228]))
; palm mask for `press`: smooth bump on the opposite side of the head from
; the strike/rim area, like a hand laid across the left half
(def press-mask (tensor @shape [6 6] @data [
  0.000 0.000 0.000 0.000 0.000 0.000
  0.020 0.100 0.060 0.000 0.000 0.000
  0.100 0.450 0.300 0.040 0.000 0.000
  0.140 0.600 0.400 0.060 0.000 0.000
  0.080 0.350 0.220 0.030 0.000 0.000
  0.010 0.060 0.040 0.000 0.000 0.000]))
; readout "mic positions": off-center smooth bumps, different spots per head
(def read-mask-p (tensor @shape [6 6] @data [
  0.000 0.000 0.000 0.009 0.013 0.002
  0.000 0.000 0.015 0.105 0.140 0.035
  0.000 0.010 0.073 0.391 0.482 0.128
  0.000 0.018 0.099 0.520 0.643 0.166
  0.000 0.007 0.044 0.225 0.280 0.070
  0.000 0.000 0.005 0.054 0.067 0.014]))
(def read-mask-r (tensor @shape [6 6] @data [
  0.022 0.102 0.096 0.007 0.000 0.000
  0.115 0.450 0.405 0.050 0.000 0.000
  0.137 0.528 0.470 0.055 0.000 0.000
  0.043 0.167 0.149 0.015 0.000 0.000
  0.005 0.040 0.040 0.000 0.000 0.000
  0.000 0.003 0.005 0.000 0.000 0.000]))
; per-wire stiffness detune (row-constant, ~±5% pitch spread => ~±10%
; stiffness). Identical wires phase-lock and sound metallic; the spread makes
; the rattle non-periodic.
(def wire-detune (tensor @shape [6 6] @data [
  0.906 0.906 0.906 0.906 0.906 0.906
  1.062 1.062 1.062 1.062 1.062 1.062
  0.951 0.951 0.951 0.951 0.951 0.951
  1.114 1.114 1.114 1.114 1.114 1.114
  0.874 0.874 0.874 0.874 0.874 0.874
  1.147 1.147 1.147 1.147 1.147 1.147]))

; ── Damping / stiffness maps ────────────────────────────────────────────────
(def p-damp (- 1 (exp (/ -13.8155106 (* samplerate (* release-v 0.001))))))
(def r-damp (- 1 (exp (/ -13.8155106 (* samplerate (* release2-v 0.001))))))
(def w-damp (- 1 (exp (/ -13.8155106 (* samplerate (* wire-decay-v 0.001))))))

(def p-pitch-hz
  (clip (* host_pitch (exp (/ (* (log 2) tune-v) 12))) 1 mem-max-pitch))
(def p-sin (sin (* pi (/ p-pitch-hz samplerate))))
(def p-stiff-base (/ (* 4 p-sin p-sin) mem-mu1))
; stiffness bound uses the SCALAR base p-damp: press adds per-cell damping on
; top, and more damping only widens the true stability region — conservative.
(def p-stiff-max (* 0.995 (/ (- 4 (* 2 p-damp)) mem-mu-max)))

(def r-pitch-hz (clip (* p-pitch-hz pitch2-ratio-v) 1 mem-max-pitch))
(def r-sin (sin (* pi (/ r-pitch-hz samplerate))))
(def r-stiff-base (/ (* 4 r-sin r-sin) mem-mu1))

(def w-pitch-hz (clip wire-pitch-v 1 wire-max-pitch))
(def w-sin (sin (* pi (/ w-pitch-hz samplerate))))
(def w-stiff-base (/ (* 4 w-sin w-sin) wire-mu1))
(def w-stiff-max (* 0.995 (/ (- 2 (* 2 w-damp)) wire-mu-max)))

; ── Read state (each history read EXACTLY once) ─────────────────────────────
(def p-state (read-tensor-history ph1))
(def p-prev (read-tensor-history ph2))
(def r-state (read-tensor-history rh1))
(def r-prev (read-tensor-history rh2))
(def w-state (read-tensor-history wh1))
(def w-prev (read-tensor-history wh2))

; ── Striker: lumped mass in Hertz contact with a rigid strike plane ─────────
; stroke reshapes the stick BEFORE the physics: ghost = lower stick height
; (slower impact) with a relaxed grip (softer effective tip), rimshot =
; harder effective tip (the stick lands shouldered on the metal rim, so the
; contact stiffens and shortens = crack). Everything downstream — contact
; time, brightness, level — then falls out of the same Hertz model.
(def stick-hard-eff
  (clip (* stick-hard-v (- 1 (* ghost-mix 0.5)) (+ 1 (* edge-mix 2))) 0.0002 0.05))
(def stick-speed-eff (* stick-speed-v (- 1 (* ghost-mix 0.65))))
(def stick-x (gswitch trigger 0.0001 (read-history stickx)))
(def stick-v (gswitch trigger (* -1 (* stick-speed-eff vel-gain))
                      (read-history stickv)))
(def stick-pen (max (* stick-x -1) 0))
(def stick-f (min (* stick-hard-eff stick-pen (sqrt stick-pen)) 0.01))
(def stick-v-next (+ stick-v stick-f))
(def stick-x-next (+ stick-x stick-v-next))
(write-history stickx stick-x-next)
(write-history stickv stick-v-next)
; head sees the reaction, roughened by a little contact-noise while touching.
; gain staging is load-bearing: cell displacements must stay well under the
; ±3 NaN-clamps (see membrane-snare header).
(def strike-force (* stick-f -1 (+ 1 (* scrape-v (noise))) 3))
; strike-position morph: open bump -> pinned edge cells
(def strike-mask-m (+ (* strike-mask (- 1 edge-mix)) (* edge-mask edge-mix)))

; strike-driven tension envelope (pitch glide): peak-held stick force,
; ~150 ms settle. Scalar-only chain, so no tensor reduce feeds dynamics.
; Doubles as the rimshot stick-pivot press envelope below.
(def benv (max (* (read-history bendenv) 0.999) (* stick-f 60)))
(write-history bendenv benv)

; ── Press: hand (and pivoting rimshot stick) laid on the batter head ────────
; press-total = user hand + the stick pressed through the rim for ~150 ms
; after a rimshot (benv is peak-held stick force, max ~0.6 at full velocity)
(def press-total (clip (+ press-v (* edge-mix (min benv 1) 0.9)) 0 1))
; per-cell damping bump under the palm. Coefficient 0.12 with palm-mask peak
; 0.6 puts palm-cell T60 ~4 ms at full press — a real muffle; the rest of the
; head keeps its release and loses energy only through the palm region.
(def p-damp-t (min (+ p-damp (* press-total press-mask 0.12)) 0.5))

; ── Head-to-head coupling: per-cell relative spring ─────────────────────────
(def kd (* head-couple-v r-stiff-base 0.5))
(def couple-pr (* kd (- r-state p-state)))   ; force on batter
(def couple-rp (* kd (- p-state r-state)))   ; force on reso head

; ── Rim hoop: 3 inharmonic modal resonators struck by the same stick ────────
; A metal hoop's flexural modes sit near ratios 1 : 2.76 : 5.40. Each partial
; is a hand-rolled 2-pole modal filter y = 2r·cos(w)·y1 - r²·y2 + x with r set
; from a per-partial T60 (higher partials die faster). The hoop is driven by
; the SAME stick impulse as the head — one stick, two contact points, which
; is exactly what makes a rimshot a rimshot. One-way coupling: the head never
; feeds the hoop, so the ring is a bounded decaying signal by construction.
; The hoop is driven by the trigger impulse, not the Hertz skin force: the
; stick-on-metal contact is orders of magnitude stiffer than tip-on-skin, so
; its pulse is near-impulsive (the ~1-2 ms skin pulse rolls off before the
; upper partials — measured 1000x less energy at the 2nd partial).
(defmacro rimmode (x f t60s y1h y2h)
  (def w0 (* twopi (/ (clip f 40 18000) samplerate)))
  (def rr (exp (/ -6.9077553 (* samplerate (max t60s 0.001)))))
  (def y1v (read-history y1h))
  (def y2v (read-history y2h))
  (def y (+ (* 2 rr (cos w0) y1v) (* -1 rr rr y2v) x))
  (write-history y2h y1v)
  (write-history y1h y)
  y)
(def rim-t60 (* rim-decay-v 0.001))
(def rim-in (* trigger edge-mix vel-gain 0.2))
(def rim-m1 (rimmode rim-in rim-pitch-v rim-t60 rm1y1 rm1y2))
(def rim-m2 (rimmode rim-in (* rim-pitch-v 2.76) (* rim-t60 0.6) rm2y1 rm2y2))
(def rim-m3 (rimmode rim-in (* rim-pitch-v 5.40) (* rim-t60 0.35) rm3y1 rm3y2))
(def rim-sig (+ rim-m1 (* rim-m2 0.6) (* rim-m3 0.35)))
; hoop ring re-injected at the head edge (shoulder of the stick buzzing the
; skin against the hoop). Clipped THEN scaled: worst case 0.5 * 0.005 per
; sample on edge cells, far under the head's restoring force.
(def rim-inj (* (clip rim-sig -0.5 0.5) rim-drive-v))

; ── Wire update + contact: position PROJECTION, not penalty forces ──────────
; (all contact machinery verbatim from membrane-snare — see its header for
; the measured failure modes that mandate projection, gap, and caps)
(def wire-gap (+ 0.0002 (* (- 1 snare-tension-v) 0.02)))
(def wire-restitution (min (* rattle-v 0.002) 0.85))
(def w-stiff (* (min w-stiff-base (/ w-stiff-max 1.147)) wire-detune))
(def w-lap (conv2d w-state wire-lap @padding same))
(def w-visc (* 0.015 (conv2d (- w-state w-prev) wire-lap @padding same)))
(def w-next-free (+ (- (* (- 2 w-damp) w-state) (* (- 1 w-damp) w-prev))
                    (* w-lap w-stiff)
                    w-visc))
(def w-limit (+ r-state wire-gap))
(def w-overshoot (max (- w-next-free w-limit) 0))   ; constraint violation
(def w-next (- w-next-free (* w-overshoot (+ 1 wire-restitution))))
(def w-nextc (max (min w-next 3) -3))
(def contact-f (min (* w-overshoot (+ 1 wire-restitution)) 0.005))

; ── Batter head update ──────────────────────────────────────────────────────
; skin pressure raises pitch slightly on top of the strike glide
(def p-stiff (min (* p-stiff-base
                     (+ 1 (* bend-v benv))
                     (+ 1 (* press-total 0.08)))
                  p-stiff-max))
(def p-lap (conv2d p-state laplacian @padding same))
(def p-visc (* tone_damp (conv2d (- p-state p-prev) laplacian @padding same)))
(def p-next (+ (- (* (- 2 p-damp-t) p-state) (* (- 1 p-damp-t) p-prev))
               (* p-lap p-stiff)
               p-visc
               (* strike-force strike-mask-m)
               (* rim-inj edge-mask)
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
(def mixdown (+ mic-top (* mic-bot bottom-mix-v) (* mic-snap snares-v)
                (* rim-sig rim-level-v)))

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
