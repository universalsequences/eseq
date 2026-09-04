; Membrane Snare MK2 — total fork of drums/membrane-snare-rim that keeps the
; finite-difference (FDTD) physical core verbatim and replaces the voicing /
; control surface with what was learned building drums/modal-snare.
;
; Kept from membrane-snare-rim, unchanged:
;   two 6x6 FDTD heads, lumped-mass Hertz striker, 6 collision-modeled wires
;   under the reso head (position PROJECTION, never penalty forces), the metal
;   hoop, and the stroke/press expression morph. All of that machinery — the
;   gap, the rectified contact caps, the +-3 NaN clamps, the hand-rolled
;   scalar filters — is load-bearing stability work; see membrane-snare's
;   header for the measured failure modes behind each.
;
; Changed, borrowing modal-snare's voicing:
;   - the 3-band BODY EQ is GONE. It barely registered and its six controls
;     ate a third of the panel. In its place is modal-snare's output shaper:
;     LOWCUT -> TONE tilt -> PUNCH -> DRIVE, which is what actually colours
;     the drum.
;   - LOWCUT (new here, not in modal-snare): a 12 dB/oct high-pass swept
;     20 Hz .. 1.2 kHz, placed FIRST so the boom is gone before the saturator
;     sees it. At 20 Hz it is inaudible (full-range); past ~200 Hz the drum
;     turns into a crack with no body, which is the point.
;   - BRIGHT: mic radiation tilt. A displacement readout buries everything
;     under the fundamental; blending in the head VELOCITY (p_next - p_now,
;     already on hand, no extra state) tilts +6 dB/oct like a close mic.
;     Normalised by (1 + bright) so the fundamental holds its level and the
;     overtones come up, rather than the whole drum getting louder.
;   - TILT: the old `tone_damp` on modal-snare's scale and name. It is the
;     frequency-dependent viscosity, i.e. exactly the per-mode decay tilt the
;     modal bank spells out explicitly. SCALE WARNING from membrane-snare
;     still applies, so the user-facing 0..2.5 is scaled down internally.
;   - STRETCH (SPREAD): partial inharmonicity, the one modal-snare control a
;     grid cannot get for free. Implemented honestly, as the physics that
;     causes it in a real head: a bi-harmonic (bending-stiffness) term
;     -S*lap(lap(p)) alongside the membrane tension term. Mode mu then sits at
;     sqrt(K*mu + S*mu^2) instead of sqrt(K*mu), so the upper partials stretch
;     sharp while the fundamental barely moves. S is bounded twice — a
;     musical fraction of K, and the remaining von Neumann budget — so it can
;     never cross the stability line.
;   - CONTACT LOSS: energy the reso head sheds while the wires are touching
;     it (Sekiguchi pseudo-loss, as in membrane-hat and modal-snare). The
;     wire kick is one-way, so without it a long-ringing head integrates the
;     bounce train and the tail grows.

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
(param release @default 1205 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)      ; batter head T60
(param release2 @default 2207 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)     ; resonant head T60
; tune offsets host pitch; default maps A4/440 to ~190 Hz batter fundamental
(param tune @default -12.5 @min -48 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param pitch2_ratio @default 1.21 @min 0.25 @max 4 @mod true @mod-mode additive @mod-depth-min -1.5 @mod-depth-max 1.5)          ; reso head vs batter
(param bend @default 0.84 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)                      ; strike pitch glide
; per-mode decay tilt = frequency-dependent viscous damping. Modal-snare's
; TILT scale (0..2.5, 0.9 nominal); internally this is membrane-snare's
; `tone_damp`, whose usable range is ~0..0.008, hence the 0.0033 factor.
; SCALE WARNING: raising that factor destabilises the grid at high tune.
(param tilt @default 0 @min 0 @max 2.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; bending stiffness -> partial stretch. 0 = ideal membrane (the rim version's
; exact tuning), 1 = maximum musical inharmonicity (top partial ~2x sharp).
(param stretch @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; where on the head the stick lands, 0..1 across the grid in each axis.
; Default is the centroid of membrane-snare-rim's painted default mask.
(param strike_x @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param strike_y @default 0.41 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; striker (lumped mass, Hertz contact against a rigid strike plane)
(param stick_hard @default 0.026 @min 0.0002 @max 0.05 @mod true @mod-mode additive @mod-depth-min -0.01 @mod-depth-max 0.01)      ; contact stiffness
(param stick_speed @default 0.2 @min 0.002 @max 0.2 @mod true @mod-mode additive @mod-depth-min -0.05 @mod-depth-max 0.05)        ; impact speed @ vel=1
(param scrape @default 0.59 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)                   ; stick-surface roughness
; ── stroke expression ──
; 0 = ghost note, 0.5 = open hit (matches membrane-snare), 1 = rimshot
(param stroke @default 0.44 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; hand laid on the head: localized damping + slight skin-pressure pitch raise
(param press @default 0.05 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; metal hoop: 3 inharmonic partials (ring flexural ratios 1 : 2.76 : 5.40)
(param rim_pitch @default 760 @min 400 @max 8000 @unit Hz @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit Hz)
(param rim_decay @default 447 @min 20 @max 1500 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)
(param rim_level @default 1.2 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
; hoop ring re-injected into the head at the edge. Cap is load-bearing: the
; hoop is one-way driven (head does not feed it back) so this is a bounded
; decaying force, but it must stay far below the head's restoring force.
(param rim_drive @default 0.001 @min 0 @max 0.005)
; head-to-head coupling (relative spring per cell: shell + air column)
(param head_couple @default 1.74 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; snare wires
(param wire_pitch @default 1623 @min 100 @max 2400 @unit Hz @mod true @mod-mode additive @mod-depth-min -800 @mod-depth-max 800 @mod-unit Hz)  ; wire fundamental
(param wire_decay @default 1525 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -1000 @mod-depth-max 1000 @mod-unit ms)   ; wire T60
(param snare_tension @default 0.44 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)            ; strainer tightness
(param rattle @default 315 @min 0 @max 400 @mod true @mod-mode additive @mod-depth-min -200 @mod-depth-max 200)                  ; bed restitution
; wire_couple is physically a wire/head MASS RATIO — see membrane-snare.
(param wire_couple @default 0.002 @min 0 @max 0.2 @mod true @mod-mode additive @mod-depth-min -0.1 @mod-depth-max 0.1)            ; wires -> reso head
; energy the reso head loses while the wires are riding on it. One-way kick
; without a loss term makes a long tail GROW.
;
; This reads 0..1, not physical units. The underlying coefficient is useful
; only across about 0 .. 0.026 and does nothing measurable above ~0.05, so a
; dial in raw units spends 90% of its travel doing nothing. Squaring a 0..1
; knob into that window puts the fine control at the bottom, where the
; difference between a buzz that rides the head and one that chokes actually
; lives: 0.5 on the dial is 0.0125, 0.72 is 0.026, 1.0 is 0.05.
(param contact_loss @default 0.62 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param snares @default 2.29 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)                    ; contact-mic level
(param bottom_mix @default 0.5 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)                ; reso-head mic level
; mic radiation tilt: 0 = displacement mic (dark), 2.5 = velocity mic (+6 dB/oct)
(param bright @default 0.74 @min 0 @max 2.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; ── output shaper (replaces the old 3-band body EQ) ──
; lowcut: 12 dB/oct high-pass, FIRST in the chain so the drive never sees the boom
(param lowcut @default 20 @min 20 @max 1200 @unit Hz @mod true @mod-mode additive @mod-depth-min -600 @mod-depth-max 600 @mod-unit Hz)
; tone: tilt around ~1.2 kHz, -1 = dark (highs off), +1 = bright (lows -16 dB)
(param tone @default 0 @min -1 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; punch: transient gain boost on the hit (~15 ms), applied BEFORE the drive
(param punch @default 0.2 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; drive: soft-clip saturation (tanh) with level compensation
(param drive @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param level @default 0.25 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

; Clipped modulation targets. All feedback-path params are range-limited at
; their exact use sites so active modulation cannot exceed the tested bounds.
(def release-v (clip (mod release) 20 3000))
(def release2-v (clip (mod release2) 20 3000))
(def tune-v (clip (mod tune) -48 24))
(def pitch2-ratio-v (clip (mod pitch2_ratio) 0.25 4))
(def bend-v (clip (mod bend) 0 4))
(def tilt-v (clip (mod tilt) 0 2.5))
(def tone-damp-v (* tilt-v 0.0033))
(def stretch-v (clip (mod stretch) 0 1))
(def strike-x-v (clip (mod strike_x) 0 1))
(def strike-y-v (clip (mod strike_y) 0 1))
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
(def wire-couple-v (clip (mod wire_couple) 0 0.2))
(def contact-loss-amt (clip (mod contact_loss) 0 1))
(def contact-loss-v (* 0.05 contact-loss-amt contact-loss-amt))
(def snares-v (clip (mod snares) 0 4))
(def bottom-mix-v (clip (mod bottom_mix) 0 2))
(def bright-v (clip (mod bright) 0 2.5))
(def lowcut-v (clip (mod lowcut) 20 1200))
(def tone-v (clip (mod tone) -1 1))
(def punch-v (clip (mod punch) 0 1))
(def drive-v (clip (mod drive) 0 1))
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
; output shaper states (hand-rolled; see membrane-snare header)
(make-history dcx1)
(make-history dcy1)
(make-history hpx1) (make-history hpx2) (make-history hpy1) (make-history hpy2)
(make-history tonelp)
(make-history punchenv)

; ── Kernels / masks ─────────────────────────────────────────────────────────
(def laplacian (tensor @shape [3 3] @data [0 1 0  1 -4 1  0 1 0]))
; STRETCH's bi-harmonic (plate bending) term is L(L(p)), built by chaining
; this same Laplacian with itself down in the head updates rather than by a
; kernel of its own. Writing L(L(p)) as the 13-point 5x5 stencil instead
; LOOKS cheaper and is a trap: zero-padded on a grid this small that stencil
; is NOT the square of the zero-padded Laplacian, and Gershgorin puts its
; spectrum on [-24, 64]. A negative eigenvalue there is an ANTI-restoring
; force, so the drum runs away however small S is — measured, as a flat
; non-decaying tail at high tune. Chaining L with itself gives L^T L, whose
; eigenvalues are mu^2 and therefore never negative, so the ordinary von
; Neumann bound applies again.
; 1D Laplacian along each row: wires are independent strings with fixed ends
; (zero-padded same conv supplies the fixed boundary)
(def wire-lap (tensor @shape [3 3] @data [0 0 0  1 -2 1  0 0 0]))

; 6x6 zero-padded membrane: mu_pq = 4 - 2cos(p pi/7) - 2cos(q pi/7)
(def mem-cos1 (cos (/ pi 7)))
(def mem-mu1 (- 4 (* 4 mem-cos1)))
(def mem-mu-max (+ 4 (* 4 mem-cos1)))
(def mem-mu-max2 (* mem-mu-max mem-mu-max))
; Fraction of the REMAINING von Neumann headroom that STRETCH is allowed to
; claim. Running the bending term right up to the theoretical limit is not
; safe here: the strike envelope makes the coefficients time-varying, and a
; Verlet mode parked at the marginal-stability boundary gets parametrically
; pumped instead of decaying — measured as a tail that sat at 0.48 forever
; with release at 400 ms. At normal tunings the tension term uses only a few
; percent of the budget, so this clamp never binds and never touches the
; sound; it only engages at high tune, where it trades some spread for a
; drum that actually stops.
(def stretch-margin 0.35)
(def mem-max-pitch (* samplerate 0.05))
; 6-point string: mu_p = 2 - 2cos(p pi/7)
(def wire-mu1 (- 2 (* 2 mem-cos1)))
(def wire-mu-max (+ 2 (* 2 mem-cos1)))
(def wire-max-pitch (* samplerate 0.05))

; STRIKE POSITION. membrane-snare-rim exposed this as a painted 6x6
; tensor-param, which is the raw state of the model rather than a thing a
; drummer does. Here it is two numbers — where on the head the stick lands —
; and the mask is generated from them as a Gaussian bump, so the control can
; be a pad you drag a stick position around in.
;
; The bump is sum-normalised to the same total the painted default mask had
; (5.686), because cells outside the grid are pinned: without it, sliding the
; strike toward a rim would quietly lose a third of the injected energy and
; read as a volume drop rather than a position change.
(def cell-x (tensor @shape [6 6] @data [
  0 1 2 3 4 5   0 1 2 3 4 5   0 1 2 3 4 5
  0 1 2 3 4 5   0 1 2 3 4 5   0 1 2 3 4 5]))
(def cell-y (tensor @shape [6 6] @data [
  0 0 0 0 0 0   1 1 1 1 1 1   2 2 2 2 2 2
  3 3 3 3 3 3   4 4 4 4 4 4   5 5 5 5 5 5]))
; contact width in cells. Matches the painted default mask's spread; a wider
; stick tip would be a natural knob here, but it is deliberately not one yet.
(def strike-sigma 1.05)
(def strike-dx (- cell-x (* strike-x-v 5)))
(def strike-dy (- cell-y (* strike-y-v 5)))
(def strike-bump
  (exp (* (+ (* strike-dx strike-dx) (* strike-dy strike-dy))
          (/ -1 (* 2 strike-sigma strike-sigma)))))
(def strike-mask (* strike-bump (/ 5.686 (max (sum strike-bump) 0.001))))
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
; Wire loss is split into a flat part and a frequency-dependent part, and the
; split is tied to the DECAY dial rather than being a fixed coefficient.
;
; membrane-snare hard-codes the wire viscosity at 0.015, which at the string's
; lowest eigenvalue (mu1 = 0.198) is 0.00297 of loss per sample no matter what
; DECAY says. That swamps the dial everywhere above ~97 ms: measured, the
; snare mic decayed in 85 ms at EVERY setting from 20 ms to 3000 ms, because
; the dial contributes 0.000189/sample at 1525 ms — 16x less than the fixed
; viscosity, and 31x less at 3000 ms. The DECAY control did nothing.
;
; Making the viscosity a multiple of the flat loss keeps the string's
; high-modes-die-first character (mode mu loses (1 + tone*mu) times the
; fundamental's rate, so the top of the string dies ~8x faster) while letting
; DECAY set the absolute timescale. Dividing the flat part by (1 + tone*mu1)
; makes the dial read true: the wire fundamental's T60 IS wire_decay.
(def wire-tone 2.0)
(def w-damp-b (/ w-damp (+ 1 (* wire-tone wire-mu1))))
(def w-visc-k (* w-damp-b wire-tone))

(def p-pitch-hz
  (clip (* host_pitch (exp (/ (* (log 2) tune-v) 12))) 1 mem-max-pitch))
(def p-sin (sin (* pi (/ p-pitch-hz samplerate))))
(def p-stiff-base (/ (* 4 p-sin p-sin) mem-mu1))
; stiffness bound uses the SCALAR base p-damp: press adds per-cell damping on
; top, and more damping only widens the true stability region — conservative.
(def p-stiff-max (* 0.995 (/ (- 4 (* 2 p-damp)) mem-mu-max)))
(def p-stiff-budget (* p-stiff-max mem-mu-max))

(def r-pitch-hz (clip (* p-pitch-hz pitch2-ratio-v) 1 mem-max-pitch))
(def r-sin (sin (* pi (/ r-pitch-hz samplerate))))
(def r-stiff-base (/ (* 4 r-sin r-sin) mem-mu1))
(def r-stiff-max (* 0.995 (/ (- 4 (* 2 r-damp)) mem-mu-max)))
(def r-stiff-budget (* r-stiff-max mem-mu-max))

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
(def w-visc (* w-visc-k (conv2d (- w-state w-prev) wire-lap @padding same)))
(def w-next-free (+ (- (* (- 2 w-damp-b) w-state) (* (- 1 w-damp-b) w-prev))
                    (* w-lap w-stiff)
                    w-visc))
(def w-limit (+ r-state wire-gap))
(def w-overshoot (max (- w-next-free w-limit) 0))   ; constraint violation
(def w-next (- w-next-free (* w-overshoot (+ 1 wire-restitution))))
(def w-nextc (max (min w-next 3) -3))
(def contact-f (min (* w-overshoot (+ 1 wire-restitution)) 0.005))
; 0..1 "wires are riding here" indicator, for the pseudo-loss term below.
; contact-f is capped at 0.005, so 2000x saturates on real contact and stays
; proportional for the light grazes that make the tail of a ghost note.
(def contact-ind (min (* contact-f 2000) 1))

; ── Batter head update ──────────────────────────────────────────────────────
; skin pressure raises pitch slightly on top of the strike glide
(def p-stiff-raw (* p-stiff-base
                   (+ 1 (* bend-v benv))
                   (+ 1 (* press-total 0.08))))
; bending stiffness (STRETCH), PITCH-COMPENSATED. Raw bending sharpens every
; mode including the fundamental, so the knob would read as a tune control
; with a timbre side effect. Subtracting S*mu1 from the tension term puts
; mode 1 back exactly where it was (its stiffness is K*mu1 + S*mu1^2, so
; K = K0 - S*mu1 restores it), leaving a pure spread: the fundamental
; holds still and the modes above it stretch progressively sharp.
(def p-bend-s0 (* stretch-v 1.35 p-stiff-raw))
(def p-tens (max (- p-stiff-raw (* p-bend-s0 mem-mu1)) (* p-stiff-raw 0.1)))
(def p-stiff (min p-tens p-stiff-max))
; stability: the budget term guarantees K*mu_max + S*mu_max^2 stays under the
; von Neumann limit even when p-stiff is already at its ceiling (at which
; point S falls to 0 and pitch wins — safe, not silent).
; the viscosity term tightens the bound as well (the true limit is
; 4 - 2*damp - 2*td*mu, not 4 - 2*damp), so charge it against the budget too
(def p-stretch-room (- p-stiff-budget
                     (* p-stiff mem-mu-max)
                     (* 2 tone-damp-v mem-mu-max)))
(def p-stretch-k (max (min p-bend-s0
                           (* stretch-margin (/ p-stretch-room mem-mu-max2)))
                      0))
; tension + viscosity + bending in ONE outer convolution. The Laplacian is
; linear and every coefficient here is a scalar, so
;   L(K*p + td*(p - p_prev) - S*L(p)) == K*L(p) + td*L(p - p_prev) - S*L(L(p))
; which is the whole right-hand side for the price of two conv2ds.
(def p-l1 (conv2d p-state laplacian @padding same))
(def p-field (+ (* p-state p-stiff)
                (* (- p-state p-prev) tone-damp-v)
                (* p-l1 (* -1 p-stretch-k))))
(def p-lap (conv2d p-field laplacian @padding same))
(def p-next (+ (- (* (- 2 p-damp-t) p-state) (* (- 1 p-damp-t) p-prev))
               p-lap
               (* strike-force strike-mask-m)
               (* rim-inj edge-mask)
               couple-pr))
(def p-nextc (max (min p-next 3) -3))   ; NaN-safety clamp only

; ── Resonant head update ────────────────────────────────────────────────────
(def r-bend-s0 (* stretch-v 1.35 r-stiff-base))
(def r-stiff (min (max (- r-stiff-base (* r-bend-s0 mem-mu1)) (* r-stiff-base 0.1))
                  r-stiff-max))
; the viscosity term tightens the bound as well (the true limit is
; 4 - 2*damp - 2*td*mu, not 4 - 2*damp), so charge it against the budget too
(def r-stretch-room (- r-stiff-budget
                     (* r-stiff mem-mu-max)
                     (* 2 tone-damp-v mem-mu-max)))
(def r-stretch-k (max (min r-bend-s0
                           (* stretch-margin (/ r-stretch-room mem-mu-max2)))
                      0))
(def r-l1 (conv2d r-state laplacian @padding same))
(def r-field (+ (* r-state r-stiff)
                (* (- r-state r-prev) (* tone-damp-v 0.5))
                (* r-l1 (* -1 r-stretch-k))))
(def r-lap (conv2d r-field laplacian @padding same))
; pseudo-loss: the head sheds velocity-proportional energy under a riding
; wire. The kick below is one-way, so without this the tail integrates the
; bounce train and GROWS at long releases.
(def r-loss (* contact-loss-v contact-ind (- r-state r-prev)))
(def r-next (+ (- (* (- 2 r-damp) r-state) (* (- 1 r-damp) r-prev))
               r-lap
               (* -1 r-loss)
               couple-rp
               (* contact-f wire-couple-v)))   ; wires slap the head back
(def r-nextc (max (min r-next 3) -3))

; ── Output taps (forward-only reduces; computed before the history writes) ──
; BRIGHT = radiation tilt. A displacement mic buries the overtones under the
; fundamental; head VELOCITY (p_next - p_now, already on hand) is a +6 dB/oct
; tilt. Blend, scaled so velocity has unit gain at the fundamental, then
; normalise by (1 + bright) so the fundamental keeps its level and only the
; overtone balance moves. The 60 cap stops a sub-100 Hz tuning from turning
; the scale factor into an amplifier for the top of the band.
; Weights: 0 .. 1.25 crossfades displacement -> velocity (+6 dB/oct),
; 1.25 .. 2.5 crossfades velocity -> acceleration (+12 dB/oct). Velocity
; alone tops out around 5 dB of tilt across this drum's mode span (measured),
; which is not a character control; acceleration doubles the slope and gets
; the upper modes genuinely forward. The three weights sum to 1 and each term
; is scaled to unit gain at its head's fundamental, so the fundamental holds
; its level at every setting and only the balance above it moves.
(def br-a (clip (/ bright-v 1.25) 0 1))
(def br-b (clip (/ (- bright-v 1.25) 1.25) 0 1))
(def br-w0 (- 1 br-a))
(def br-w1 (* br-a (- 1 br-b)))
(def br-w2 (* br-a br-b))
(def p-vel-k (clip (/ samplerate (* twopi p-pitch-hz)) 1 60))
(def r-vel-k (clip (/ samplerate (* twopi r-pitch-hz)) 1 60))
(def mic-top-n (sum (* p-nextc read-mask-p)))
(def mic-top-c (sum (* p-state read-mask-p)))
(def mic-top-o (sum (* p-prev read-mask-p)))
(def mic-top (+ (* br-w0 mic-top-n)
                (* br-w1 p-vel-k (- mic-top-n mic-top-c))
                (* br-w2 p-vel-k p-vel-k (+ (- mic-top-n (* 2 mic-top-c)) mic-top-o))))
(def mic-bot-n (sum (* r-nextc read-mask-r)))
(def mic-bot-c (sum (* r-state read-mask-r)))
(def mic-bot-o (sum (* r-prev read-mask-r)))
(def mic-bot (+ (* br-w0 mic-bot-n)
                (* br-w1 r-vel-k (- mic-bot-n mic-bot-c))
                (* br-w2 r-vel-k r-vel-k (+ (- mic-bot-n (* 2 mic-bot-c)) mic-bot-o))))
; The snare mic hears the CONTACT train only, never the wire bed's own
; vibration. That is deliberate, and it is also why WIRE DECAY is a quiet
; control — see the note on w-damp-b. A wire mic was tried here and removed:
; the wires spend the whole tail resting against the head being chattered by
; the constraint, so radiating them directly adds an erratic rattle that never
; decays (measured: the 50 ms peak envelope stopped falling after 150 ms and
; wandered around 0.02-0.05 forever) rather than a metallic ring.
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

; ── Shaper: lowcut -> tone tilt -> punch -> drive -> DC block ───────────────
; The chain is level-neutral at drive 0 / punch 0 / tone 0: xs scales the mix
; into a +-1 working range for the tanh and the final gain scales it back.
(def xs (* mixdown 0.25))
; lowcut: RBJ 2-pole high-pass, q = 0.707. First in the chain so the boom is
; gone before the saturator can turn it into low-mid mud.
(def hp-w0 (* twopi (/ lowcut-v samplerate)))
(def hp-cw (cos hp-w0))
(def hp-alpha (/ (sin hp-w0) 1.41421356))
(def hp-a0 (+ 1 hp-alpha))
(def hp-b0 (/ (+ 1 hp-cw) 2))
(def hp-x1 (read-history hpx1))
(def hp-x2 (read-history hpx2))
(def hp-y1 (read-history hpy1))
(def hp-y2 (read-history hpy2))
(def hp-y (/ (+ (* hp-b0 xs) (* -2 hp-b0 hp-x1) (* hp-b0 hp-x2)
                (* 2 hp-cw hp-y1) (* -1 (- 1 hp-alpha) hp-y2))
             hp-a0))
(write-history hpx2 hp-x1)
(write-history hpx1 xs)
(write-history hpy2 hp-y1)
(write-history hpy1 hp-y)
(def cut (clip hp-y -8 8))
; tone: one-pole split at ~1.15 kHz (coef 0.15), tilt the two halves
(def tlp-prev (read-history tonelp))
(def tlp (+ tlp-prev (* 0.15 (- cut tlp-prev))))
(write-history tonelp tlp)
(def toned (+ (* tlp (- 1 (* (max tone-v 0) 0.85)))
              (* (- cut tlp) (+ 1 tone-v))))
; punch: trigger-fired envelope, ~14 ms e-fold, up to +12 dB on the transient
(def penv (max (* (read-history punchenv) 0.9985) trigger))
(write-history punchenv penv)
(def punched (* toned (+ 1 (* punch-v 3 penv))))
; drive: tanh soft clip, normalized so a full-scale hit stays full-scale
(def pre (+ 1 (* drive-v 12)))
(def driven (/ (tanh (* punched pre)) (tanh pre)))
(def bodied (* driven 4))
; one-pole-zero DC blocker
(def dcin bodied)
(def dcy (+ (- dcin (read-history dcx1)) (* 0.998 (read-history dcy1))))
(write-history dcx1 dcin)
(write-history dcy1 dcy)
(out (* dcy level-v vel-gain) 1 @name audio)
