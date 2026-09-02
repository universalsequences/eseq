; Modal Snare — PROTOTYPE modal-synthesis re-voicing of drums/membrane-snare-rim.
; Same instrument idea (batter head + resonant head + snare wires + rim hoop,
; stroke/press expression, Hertz striker), but the heads are MODAL BANKS of
; 2-pole resonators tuned to the circular-membrane eigenmodes (Bessel zeros)
; instead of 6x6 FDTD grids. A modal bank and a FDTD grid compute the same
; linear membrane; the bank just does it per-mode, which buys:
;   - unconditional stability (|pole| < 1, no Verlet CFL bound, no rails)
;   - exact tuning of every partial + a `stretch` knob for air-loading
;     inharmonicity that a grid can't dial in
;   - per-mode decay (`tilt`) instead of a fragile viscosity coefficient
;   - a fraction of the CPU
; What a bank canNOT do natively: (a) the snare-wire COLLISION (nonlinear
; spatial contact) and (b) TWO-WAY head coupling, because the dgen compiler
; kills any feedback of a tensor reduce into its own tensor — even through a
; one-sample scalar history (measured, scratchpad fbtest: the bank goes dead
; in both the fed-back and the zero-gain variant). Solution, extending the
; membrane-hat trick: every (0,n) "volume" mode of BOTH heads is a SCALAR
; 2-pole proxy. The proxies are coupled two-way by the air column (relative
; spring on the volume displacements, same strength law as the grid's kd), so
; a palm on the batter drains the resonant head too — that is what made the
; grid version's muted rimshots decay in ~100 ms with release at 3 s. The
; tensor banks hold only the asymmetric (m>0) modes and never feed anything
; back. 12 detuned wire 2-poles bounce off the reso proxies' displacement
; (rectified impulsive approach force with a strainer gap); the contact train
; flows ONE-WAY into the reso bank (strip mode-shape) and a contact mic.
;
; Mode tables baked by scratchpad gen_tables.py (numpy Bessel integrals):
; modes sorted by frequency, weights = psi(x0)/modal-mass with a 0.06-radius
; stick-tip lowpass; open strike at r=0.40, edge at r=0.93, mics at r=0.55
; and r=0.50; `vol` = int psi dA (air coupling, m=0 modes only); `strip` =
; mean psi along the wire diameter.

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

; ── Params (names match membrane-snare-rim where the meaning is the same) ───
(param release @default 180 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)      ; batter fundamental T60
(param release2 @default 260 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)     ; reso fundamental T60
(param tune @default -14.5 @min -48 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param pitch2_ratio @default 1.35 @min 0.25 @max 4 @mod true @mod-mode additive @mod-depth-min -1.5 @mod-depth-max 1.5)
(param bend @default 0.5 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
; NEW (modal-only): partial spread. 1 = ideal membrane Bessel ratios,
; <1 compresses the overtones toward the fundamental (air-loaded drum head),
; >1 stretches them (stiff/plate-like).
(param stretch @default 0.9 @min 0.4 @max 1.6 @mod true @mod-mode additive @mod-depth-min -0.5 @mod-depth-max 0.5)
; NEW (modal-only): per-mode decay tilt, T60_n = release * ratio_n^-tilt.
; Replaces the FDTD viscosity knob; 0 = every partial rings equally.
(param tilt @default 0.9 @min 0 @max 2.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; striker (lumped mass, Hertz contact — verbatim family machinery)
; stick TIP radius (fraction of head radius): a finite tip low-passes the
; strike over the modes, exp(-(k_n * tip)^2). The grid's strike mask is a
; broad bump spanning half the head, which is why it pumps the fundamental
; so hard (its open hit is 97% below 300 Hz); a point tip spreads the same
; energy over every asymmetric mode and reads as a click.
(param tip @default 0.2 @min 0.03 @max 0.45 @mod true @mod-mode additive @mod-depth-min -0.2 @mod-depth-max 0.2)
(param stick_hard @default 0.004 @min 0.0002 @max 0.05 @mod true @mod-mode additive @mod-depth-min -0.01 @mod-depth-max 0.01)
(param stick_speed @default 0.02 @min 0.002 @max 0.2 @mod true @mod-mode additive @mod-depth-min -0.05 @mod-depth-max 0.05)
(param scrape @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; stroke expression: 0 = ghost, 0.5 = open, 1 = rimshot; press = palm on head
(param stroke @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param press @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; metal hoop (verbatim from the rim version)
(param rim_pitch @default 2200 @min 400 @max 8000 @unit Hz @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit Hz)
(param rim_decay @default 300 @min 20 @max 1500 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)
(param rim_level @default 1.2 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
(param rim_drive @default 0.001 @min 0 @max 0.005)
; batter -> reso air coupling (one-way)
(param head_couple @default 0.6 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; snare wires
(param wire_pitch @default 620 @min 100 @max 2400 @unit Hz @mod true @mod-mode additive @mod-depth-min -800 @mod-depth-max 800 @mod-unit Hz)
(param wire_decay @default 420 @min 20 @max 3000 @unit ms @mod true @mod-mode additive @mod-depth-min -1000 @mod-depth-max 1000 @mod-unit ms)
(param snare_tension @default 0.85 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; bounce hardness (contact impulse gain per unit approach velocity)
(param rattle @default 3 @min 0 @max 8 @mod true @mod-mode additive @mod-depth-min -4 @mod-depth-max 4)
; linear drag of the wires by the reso head (one-way). 0 = wires move only
; when bounced, which measured the densest, most impulsive buzz.
; energy the reso head LOSES while wires touch it (Sekiguchi pseudo-loss, as
; in membrane-hat). Now that contact is sustained overlap this fires every
; sample of contact, so it must be tiny: 0.01 drained the bottom head in
; 40 ms and killed the rattle (the grid's rattle follows the head for 160 ms). The wire kick is one-way, so without this a long-ringing
; head would integrate the bounce train forever (measured at release 3 s,
; tilt 0: the tail GREW for 0.5 s). Also why snares-on drums ring shorter.
(param contact_loss @default 0.0005 @min 0 @max 0.02 @mod true @mod-mode additive @mod-depth-min -0.04 @mod-depth-max 0.04)
(param wire_drive @default 0.0 @min 0 @max 0.02)
(param snares @default 0.6 @min 0 @max 4 @mod true @mod-mode additive @mod-depth-min -2 @mod-depth-max 2)
(param bottom_mix @default 0.5 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; NEW (modal-only): mic radiation tilt, weight_n *= ratio_n^bright. 1 = the
; mic hears head VELOCITY (a close mic); higher = brighter/more attack.
(param bright @default 0 @min 0 @max 2.5 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; bounce restitution + 1: the approach term IS the relative closing velocity,
; so kick 1 = the wire sticks to the head, 2 = perfectly elastic. HARD MAX 2:
; above it a bounce is super-elastic and a ringing head Fermi-pumps the
; wires into a chatter that outlives the drum (measured at kick 5).
(param wire_kick @default 1.8 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; PROTOTYPE debug tap: 0 = audio, 1 = reso proxy z, 2 = wire 1, 3 = raw contact
(param dbg @default 0 @min 0 @max 3)
; palm damping strength (per-sample state shrink at full press on the most
; palm-covered mode). The grid's half-head palm leaves the other half ringing
; as a smaller drum (~150 ms whole-drum T60); fixed modal shapes can't
; relocalize, so this must be far weaker than the grid's local 4 ms T60.
(param palm @default 0.003 @min 0.0005 @max 0.02)
; ── output shaper (replaces the old 3-band body EQ, which barely registered)
; drive: soft-clip saturation (tanh) with level compensation
(param drive @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; tone: tilt around ~1.2 kHz, -1 = dark (highs off), +1 = bright (lows -16 dB, highs +6 dB)
(param tone @default 0 @min -1 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
; punch: transient gain boost on the hit (~15 ms), applied BEFORE the drive
(param punch @default 0.2 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param level @default 0.25 @min 0 @max 2 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

(def release-v (clip (mod release) 20 3000))
(def release2-v (clip (mod release2) 20 3000))
(def tune-v (clip (mod tune) -48 24))
(def pitch2-ratio-v (clip (mod pitch2_ratio) 0.25 4))
(def bend-v (clip (mod bend) 0 4))
(def stretch-v (clip (mod stretch) 0.4 1.6))
(def tilt-v (clip (mod tilt) 0 2.5))
(def tip-v (clip (mod tip) 0.03 0.45))
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
(def rattle-v (clip (mod rattle) 0 8))
(def wire-drive-v (clip wire_drive 0 0.02))
(def contact-loss-v (clip (mod contact_loss) 0 0.02))
(def snares-v (clip (mod snares) 0 4))
(def bottom-mix-v (clip (mod bottom_mix) 0 2))
(def level-v (clip (mod level) 0 2))
(def drive-v (clip (mod drive) 0 1))
(def tone-v (clip (mod tone) -1 1))
(def punch-v (clip (mod punch) 0 1))
(def bright-v (clip (mod bright) 0 2.5))
(def wire-kick-v (clip (mod wire_kick) 0 2))

(def edge-mix (clip (* (- stroke-v 0.5) 2) 0 1))
(def ghost-mix (clip (* (- 0.5 stroke-v) 2) 0 1))

; ── Mode tables (baked; see header) ─────────────────────────────────────────
; batter: modes (m,n) in order: [(0, 1), (1, 1), (2, 1), (0, 2), (3, 1), (1, 2), (4, 1), (2, 2), (0, 3), (5, 1), (3, 2), (6, 1), (1, 3), (4, 2), (7, 1), (2, 3), (0, 4), (5, 2), (3, 3), (1, 4), (6, 2), (4, 3), (2, 4), (7, 2), (0, 5), (5, 3), (3, 4), (1, 5), (6, 3), (4, 4), (2, 5), (0, 6), (7, 3), (5, 4), (3, 5), (1, 6)]
(def bat-ratio (tensor @shape [6 6] @data [
  1.0000 1.5933 2.1355 2.2954 2.6531 2.9173
  3.1555 3.5001 3.5985 3.6475 4.0589 4.1317
  4.2304 4.6010 4.6101 4.8319 4.9033 5.1308
  5.4121 5.5404 5.6508 5.9765 6.1526 6.1631
  6.2087 6.5286 6.7462 6.8490 7.0707 7.3253
  7.4682 7.5145 7.6045 7.8925 8.0710 8.1569
]))
(def bat-open (tensor @shape [6 6] @data [
  0.2855 0.6610 0.5781 0.0824 0.4417 0.7621
  0.3103 1.0000 -0.3902 0.2055 0.9754 0.1300
  -0.2045 0.8173 0.0794 0.3853 -0.2984 0.6212
  0.7458 -0.7702 0.4401 0.8594 -0.4845 0.2953
  0.1509 0.8020 -0.0651 -0.2519 0.6601 0.2723
  -0.5038 0.2583 0.4979 0.4545 -0.4525 0.3257
]))
(def bat-edge (tensor @shape [6 6] @data [
  0.0469 0.1855 0.2807 -0.1474 0.3744 -0.3912
  0.4621 -0.4729 0.2378 0.5404 -0.5378 0.6065
  0.5359 -0.5849 0.6587 0.5730 -0.2845 -0.6142
  0.5891 -0.5770 -0.6265 0.5866 -0.5645 -0.6233
  0.2787 0.5686 -0.5361 0.5207 0.5384 -0.4962
  0.4730 -0.2328 0.4991 -0.4491 0.4192 -0.4051
]))
(def bat-center (tensor @shape [6 6] @data [
  0.7149 0.2202 0.0255 1.4992 0.0026 0.6342
  0.0002 0.0906 1.9516 0.0000 0.0107 0.0000
  1.0742 0.0011 0.0000 0.1844 2.0266 0.0001
  0.0252 1.3767 0.0000 0.0030 0.2750 0.0000
  1.7951 0.0003 0.0423 1.4556 0.0000 0.0055
  0.3305 1.3938 0.0000 0.0006 0.0563 1.3221
]))
(def bat-vol (tensor @shape [6 6] @data [
  1.0000 0.0000 0.0000 -0.2855 0.0000 0.0000
  0.0000 0.0000 0.1453 0.0000 0.0000 0.0000
  0.0000 0.0000 0.0000 0.0000 -0.0913 0.0000
  0.0000 0.0000 0.0000 0.0000 0.0000 0.0000
  0.0641 0.0000 0.0000 0.0000 0.0000 0.0000
  0.0000 -0.0481 0.0000 0.0000 0.0000 0.0000
]))
(def bat-mic (tensor @shape [6 6] @data [
  0.6082 0.4341 0.0815 -0.2720 -0.1958 -0.0082
  -0.2886 0.0297 -0.2524 -0.2228 -0.1457 -0.0896
  -0.2560 -0.3264 0.0259 -0.0509 0.2578 -0.3406
  0.1018 0.0691 -0.1736 0.0775 -0.0129 0.0612
  0.1191 -0.0315 0.0991 0.1820 -0.0647 0.2415
  0.0431 -0.2428 0.0386 0.2439 -0.0966 -0.1071
]))
; reso bank shares the batter's 36-mode tables (same geometry); only its
; mic and wire-strip readouts differ:
; reso36 modes (m,n) in order: [(0, 1), (1, 1), (2, 1), (0, 2), (3, 1), (1, 2), (4, 1), (2, 2), (0, 3), (5, 1), (3, 2), (6, 1), (1, 3), (4, 2), (7, 1), (2, 3)]
; per-wire detune (~±5% pitch spread so the bounces never phase-lock)
; (same six values as membrane-snare's wire rows)

; palm-energy fraction per batter mode (∫psi² over a hand-sized bump on the
; left half / ∫psi², max-normalized): the press damps modes with antinodes
; under the palm, so the fundamental dies while others keep ringing
(def res-mic (tensor @shape [6 6] @data [
  0.6699 -0.3869 -0.0510 -0.1684 0.2785 -0.0894
  -0.2436 -0.0345 -0.3563 0.0869 0.3132 0.0423
  0.2239 -0.3856 -0.0831 0.0226 0.1208 0.1787
  -0.0306 0.0711 0.1077 -0.1060 0.0280 -0.2541
  0.2709 0.1053 -0.2366 -0.1739 0.0948 0.2431
  -0.0158 -0.0990 -0.2947 -0.0771 -0.0115 -0.0606
]))
(def res-strip (tensor @shape [6 6] @data [
  0.4323 -0.0000 -0.8861 0.1971 0.0000 -0.0000
  0.9444 -0.4328 0.3825 -0.0000 0.0000 -1.0000
  -0.0000 0.4598 -0.0000 -0.7634 0.2229 -0.0000
  0.0000 -0.0000 -0.4808 0.7789 -0.4568 -0.0000
  0.3703 -0.0000 0.0000 -0.0000 -0.7996 0.4651
  -0.7439 0.2284 -0.0000 -0.0000 0.0000 -0.0000
]))
(def bat-palm (tensor @shape [6 6] @data [
  0.5506 1.0000 0.7479 0.4623 0.6162 0.7756
  0.5615 0.6241 0.4656 0.5308 0.5715 0.5065
  0.7557 0.5662 0.4851 0.5911 0.4659 0.5688
  0.5381 0.7475 0.5687 0.5371 0.5770 0.5652
  0.4660 0.5470 0.5205 0.7436 0.5557 0.5176
  0.5697 0.4660 0.5616 0.5274 0.5104 0.7414
]))
; asymmetric-mode selectors: the (0,n) entries are handled by the scalar
; proxies below, so the banks must never be excited there
(def bat-asym (tensor @shape [6 6] @data [
  0 1 1 0 1 1
  1 1 0 1 1 1
  1 1 1 1 0 1
  1 1 1 1 1 1
  0 1 1 1 1 1
  1 0 1 1 1 1
]))

; ── Feedback state ─────────────────────────────────────────────────────────
(make-tensor-history bat1 @shape [6 6])   ; batter asymmetric modes (t)
(make-tensor-history bat2 @shape [6 6])   ; (t-1)
(make-tensor-history res1 @shape [6 6])   ; reso asymmetric modes (t)
(make-tensor-history res2 @shape [6 6])
(make-history stickx)
(make-history stickv)
(make-history bendenv)
(make-history rm1y1) (make-history rm1y2)
(make-history rm2y1) (make-history rm2y2)
(make-history rm3y1) (make-history rm3y2)
(make-history dcx1) (make-history dcy1)
(make-history tonelp) (make-history punchenv)
; (0,n) proxies: batter bp*, reso rp*
(make-history bp1a) (make-history bp1b)
(make-history bp2a) (make-history bp2b)
(make-history bp3a) (make-history bp3b)
(make-history rp1a) (make-history rp1b)
(make-history rp2a) (make-history rp2b)
(make-history rp3a) (make-history rp3b)
(make-history cfh)

; ── Read state (each history read EXACTLY once) ─────────────────────────────
(def bat1v (read-tensor-history bat1))
(def bat2v (read-tensor-history bat2))
(def res1v (read-tensor-history res1))
(def res2v (read-tensor-history res2))
(def bp1y1 (read-history bp1a)) (def bp1y2 (read-history bp1b))
(def bp2y1 (read-history bp2a)) (def bp2y2 (read-history bp2b))
(def bp3y1 (read-history bp3a)) (def bp3y2 (read-history bp3b))
(def rp1y1 (read-history rp1a)) (def rp1y2 (read-history rp1b))
(def rp2y1 (read-history rp2a)) (def rp2y2 (read-history rp2b))
(def rp3y1 (read-history rp3a)) (def rp3y2 (read-history rp3b))
(def ind-prev (read-history cfh))

; ── Striker: lumped mass in Hertz contact (verbatim from the rim version) ───
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
; modal injection is a FORCE; a 2-pole integrates it with the physical 1/w
; gain, so this scale is much smaller than the grid version's.
(def strike-force (* stick-f -1 (+ 1 (* scrape-v (noise))) 0.3))

; peak-held stick force: pitch glide + rimshot stick-pivot press (~150 ms)
(def benv (max (* (read-history bendenv) 0.999) (* stick-f 60)))
(write-history bendenv benv)
(def press-total (clip (+ press-v (* edge-mix (min benv 1) 0.9)) 0 1))

; ── Mode frequencies / per-mode decay ───────────────────────────────────────
(def f0 (clip (* host_pitch (exp (/ (* (log 2) tune-v) 12))) 20 2000))
(def bat-r (max (+ 1 (* (- bat-ratio 1) stretch-v)) 0.3))
; stretched (0,2) / (0,3) ratios
(def r2s (+ 1 (* 1.2954 stretch-v)))
(def r3s (+ 1 (* 2.5985 stretch-v)))
; FDTD stiffness x(1+bend*benv) is pitch x sqrt(...); skin pressure raises it
(def bend-mul (* (sqrt (+ 1 (* bend-v benv))) (+ 1 (* press-total 0.04))))
(def f0-res (clip (* f0 pitch2-ratio-v) 20 2000))
(def f-bat (max (* bat-r f0 bend-mul) 20))
(def f-res (max (* bat-r f0-res) 20))
(def w-bat (min (* twopi (/ f-bat samplerate)) 2.83))
(def w-res (min (* twopi (/ f-res samplerate)) 2.83))
(def t60-bat (max (* release-v 0.001 (pow bat-r (* tilt-v -1))) 0.002))
(def t60-res (max (* release2-v 0.001 (pow bat-r (* tilt-v -1))) 0.002))
(def r-bat (exp (/ -6.9077553 (* samplerate t60-bat))))
(def r-res (exp (/ -6.9077553 (* samplerate t60-res))))
; palm damping = per-sample state shrink (dissipative by construction),
; weighted by each mode's energy under the palm. Max coefficient 0.008 puts
; the most-damped mode near 20 ms T60 at full press.
(def dm-bat (max (- 1 (* press-total palm bat-palm)) 0.75))
(def bat1d (* bat1v dm-bat))
(def bat2d (* bat2v dm-bat))

; tip spread relative to the baked 0.06 tip: k_n = 2.405 * ratio_n
(defmacro tipspread (rat)
  (exp (- (* (* 2.405 rat 0.06) (* 2.405 rat 0.06)) (* (* 2.405 rat tip-v) (* 2.405 rat tip-v)))))
(def spread (tipspread bat-r))
(def spread1 (tipspread 1))
(def spread2 (tipspread r2s))
(def spread3 (tipspread r3s))
(def strike-mask-m (* (+ (* bat-open (- 1 edge-mix)) (* bat-edge edge-mix)) spread))

; ── Rim hoop: 3 inharmonic modal resonators (verbatim) ──────────────────────
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
; hoop ring re-injected at the head edge. The grid version injects
; rim_drive*clip(ring) per edge cell against a strike force ~10x ours, so the
; same knob range needs a 0.013 scale here (measured: hoop-only then peaks
; at ~1.5x the batter-only hit, as on the grid). Clipped THEN scaled.
(def rim-inj (* (clip rim-sig -0.5 0.5) rim-drive-v 0.013))

; ── (0,n) proxies: scalar volume modes of both heads, two-way air coupling ──
; A 2-pole with pre-read state (every history is read once, above).
(defmacro pm2 (x w0 rr dmv y1v y2v y1h y2h)
  (def y1d (* y1v dmv))
  (def y (+ (* 2 rr (cos w0) y1d) (* -1 rr rr (* y2v dmv)) x))
  (write-history y2h y1d)
  (write-history y1h y)
  y)
(def wb1 (min (* twopi (/ (* f0 bend-mul) samplerate)) 2.83))
(def wb2 (min (* twopi (/ (* f0 bend-mul r2s) samplerate)) 2.83))
(def wb3 (min (* twopi (/ (* f0 bend-mul r3s) samplerate)) 2.83))
(def wr1 (min (* twopi (/ f0-res samplerate)) 2.83))
(def wr2 (min (* twopi (/ (* f0-res r2s) samplerate)) 2.83))
(def wr3 (min (* twopi (/ (* f0-res r3s) samplerate)) 2.83))
(def rb1 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001) 0.002)))))
(def rb2 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001 (pow r2s (* tilt-v -1))) 0.002)))))
(def rb3 (exp (/ -6.9077553 (* samplerate (max (* release-v 0.001 (pow r3s (* tilt-v -1))) 0.002)))))
(def rr1 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001) 0.002)))))
(def rr2 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001 (pow r2s (* tilt-v -1))) 0.002)))))
(def rr3 (exp (/ -6.9077553 (* samplerate (max (* release2-v 0.001 (pow r3s (* tilt-v -1))) 0.002)))))
; palm on the batter volume modes (palm-energy fractions from the table)
(def dmb1 (- 1 (* press-total palm 0.5506)))
(def dmb2 (- 1 (* press-total palm 0.4623)))
(def dmb3 (- 1 (* press-total palm 0.4851)))
; bounce loss on the reso proxies uses LAST sample's contact indicator
(def dm-p (- 1 (* ind-prev contact-loss-v)))
; air column: relative spring between the two heads' volume displacements,
; from LAST outputs (one-sample delay breaks the algebraic loop). Strength
; law matches the grid's kd = head_couple * 0.5 * (per-cell stiffness): the
; grid's fundamental has mu = 0.395 of that cell stiffness, so kd/k_mode is
; 1.27*head_couple at the fundamental (measured: 0.5 left the reso head
; ringing long after the palm had killed the batter).
(def vol-b (+ bp1y1 (* -0.2855 bp2y1) (* 0.1453 bp3y1)))
(def vol-r (+ rp1y1 (* -0.2855 rp2y1) (* 0.1453 rp3y1)))
(def kc (* head-couple-v 1.27 wr1 wr1))
(def air-b (* kc (- vol-r vol-b)))
(def air-r (* kc (- vol-b vol-r)))
; batter proxies: strike (open/edge morph, (0,n) mask entries) + hoop + air
(def sb1 (+ (* strike-force spread1 (+ (* 0.2855 (- 1 edge-mix)) (* 0.0469 edge-mix))) (* rim-inj 0.0469) air-b))
(def sb2 (+ (* strike-force spread2 (+ (* 0.0824 (- 1 edge-mix)) (* -0.1474 edge-mix))) (* rim-inj -0.1474) (* air-b -0.2855)))
(def sb3 (+ (* strike-force spread3 (+ (* -0.2045 (- 1 edge-mix)) (* 0.2378 edge-mix))) (* rim-inj 0.2378) (* air-b 0.1453)))
(def bq1 (pm2 sb1 wb1 rb1 dmb1 bp1y1 bp1y2 bp1a bp1b))
(def bq2 (pm2 sb2 wb2 rb2 dmb2 bp2y1 bp2y2 bp2a bp2b))
(def bq3 (pm2 sb3 wb3 rb3 dmb3 bp3y1 bp3y2 bp3a bp3b))
; reso proxies: air + a little shell bleed of the strike
(def sr1 (+ air-r (* strike-force 0.0517 0.03)))
(def sr2 (+ (* air-r -0.2855) (* strike-force -0.1623 0.03)))
(def sr3 (+ (* air-r 0.1453) (* strike-force 0.2619 0.03)))
(def zp1 (pm2 sr1 wr1 rr1 dm-p rp1y1 rp1y2 rp1a rp1b))
(def zp2 (pm2 sr2 wr2 rr2 dm-p rp2y1 rp2y2 rp2a rp2b))
(def zp3 (pm2 sr3 wr3 rr3 dm-p rp3y1 rp3y2 rp3a rp3b))
; reso head displacement at the strip center (J0(0) = 1 for every (0,n))
(def z-res (+ zp1 zp2 zp3))

; ── Snare wires: 12 detuned scalar strings bouncing off the reso head ───────
; Wires are thrown by the rectified IMPULSIVE approach force (never a
; sustained penalty on the wires); the reso head and the mic see the
; sustained overlap (grid semantics). kick x approach is a velocity-exchange
; contact (kick = 1 + restitution). Wires never feed the proxies.
; gap scaled to the reso PROXY displacement (~0.01-0.07 peak; the grid's
; 0.0002+0.02 was against ~0.5). Measured: a gap the proxy falls under by
; 60 ms cuts the rattle off there, while the grid's keeps chattering to
; 160 ms with the head.
(def wire-gap (+ 0.00002 (* (- 1 snare-tension-v) 0.001)))
(def wire-t60 (* wire-decay-v 0.001))
(def wdrive (* z-res wire-drive-v))
(defmacro pmode (x f t60s dmv y1h y2h)
  (def w0 (min (* twopi (/ (max f 20) samplerate)) 2.83))
  (def rr (exp (/ -6.9077553 (* samplerate (max t60s 0.002)))))
  (def y1v (* (read-history y1h) dmv))
  (def y2v (* (read-history y2h) dmv))
  (def y (+ (* 2 rr (cos w0) y1v) (* -1 rr rr y2v) x))
  (write-history y2h y1v)
  (write-history y1h y)
  y)
(defmacro wire (det y1h y2h ovh kh)
  (def kick (read-history kh))
  (def w (pmode (+ wdrive (* kick wire-kick-v)) (* wire-pitch-v det) wire-t60 1 y1h y2h))
  (def ov (max (- (- z-res w) wire-gap) 0))
  (def ovp (read-history ovh))
  (write-history ovh ov)
  (def ap (min (max (- ov ovp) 0) 0.02))
  (write-history kh ap)
  ; the wire is thrown by the APPROACH impulse (above), but what the head
  ; and the mic feel is the sustained OVERLAP, as on the grid: it pulses
  ; once per wire cycle, so the rattle carries the wire pitch instead of
  ; reading as white clicks. Capped like the grid's per-cell contact.
  (min ov 0.02))
(make-history w1a) (make-history w1b) (make-history w1o) (make-history w1k)
(make-history w2a) (make-history w2b) (make-history w2o) (make-history w2k)
(make-history w3a) (make-history w3b) (make-history w3o) (make-history w3k)
(make-history w4a) (make-history w4b) (make-history w4o) (make-history w4k)
(make-history w5a) (make-history w5b) (make-history w5o) (make-history w5k)
(make-history w6a) (make-history w6b) (make-history w6o) (make-history w6k)
(make-history w7a) (make-history w7b) (make-history w7o) (make-history w7k)
(make-history w8a) (make-history w8b) (make-history w8o) (make-history w8k)
(make-history w9a) (make-history w9b) (make-history w9o) (make-history w9k)
(make-history w10a) (make-history w10b) (make-history w10o) (make-history w10k)
(make-history w11a) (make-history w11b) (make-history w11o) (make-history w11k)
(make-history w12a) (make-history w12b) (make-history w12o) (make-history w12k)
(def wire1 (wire 0.906 w1a w1b w1o w1k))
(def approach (+ wire1
                 (wire 1.062 w2a w2b w2o w2k)
                 (wire 0.951 w3a w3b w3o w3k)
                 (wire 1.114 w4a w4b w4o w4k)
                 (wire 0.874 w5a w5b w5o w5k)
                 (wire 1.147 w6a w6b w6o w6k)
                 (wire 0.932 w7a w7b w7o w7k)
                 (wire 1.088 w8a w8b w8o w8k)
                 (wire 0.983 w9a w9b w9o w9k)
                 (wire 1.131 w10a w10b w10o w10k)
                 (wire 0.891 w11a w11b w11o w11k)
                 (wire 1.037 w12a w12b w12o w12k)))
(def contact-f (* approach rattle-v))
(def contact-ind (min (* approach 50) 1))
(write-history cfh contact-ind)
(defmacro hp1 (x hist)
  (def lpv (read-history hist))
  (def lpn (+ lpv (* 0.1 (- x lpv))))
  (write-history hist lpn)
  (- x lpn))
(make-history cfl1) (make-history cfl2)
(def contact-hp (hp1 (hp1 contact-f cfl1) cfl2))

; ── Bank updates (asymmetric modes only; y = 2r cos(w) y1 - r² y2 + x) ──────
; TWO-WAY per-mode head coupling, elementwise between the two banks from
; last-sample states — exactly the grid's kd*(r - p) per cell, which the
; compiler handles fine (only REDUCES break feedback). Same kc as the
; proxies. This is what carries the strike's asymmetric-mode energy into the
; reso head and lets a palm on the batter drain the whole drum.
(def couple-b (* kc (- res1v bat1v)))
(def couple-r (* kc (- bat1v res1v)))
(def x-bat (* (+ (* strike-force strike-mask-m) (* rim-inj bat-edge) couple-b) bat-asym))
(def bat-next (+ (* 2 r-bat (cos w-bat) bat1d) (* -1 r-bat r-bat bat2d) x-bat))
(def bat-nextc (max (min bat-next 3) -3))
(def dm-res (- 1 (* contact-ind contact-loss-v)))
(def res1d (* res1v dm-res))
(def res2d (* res2v dm-res))
; wire slap on the reso head: the grid's cap*wire_couple is ~0.3% of its
; strike force; 0.002 here keeps a similar proportion (0.5 kicked the bank
; ~500x harder than the stick and it rang for the full release)
(def x-res (* (+ (* contact-hp res-strip 0.002) (* strike-force bat-edge 0.03) couple-r) bat-asym))
(def res-next (+ (* 2 r-res (cos w-res) res1d) (* -1 r-res r-res res2d) x-res))
(def res-nextc (max (min res-next 3) -3))

; ── Output taps ─────────────────────────────────────────────────────────────
; radiation tilt (velocity-like readout): a displacement bank's 1/w gain
; buries everything under the fundamental otherwise. Proxies use the (0,n)
; entries of the same mic tables.
(def mic-top (* 14 (+ (sum (* bat-nextc bat-mic (pow bat-r bright-v)))
                      (* bq1 0.6082)
                      (* bq2 -0.2720 (pow r2s bright-v))
                      (* bq3 -0.2524 (pow r3s bright-v)))))
(def mic-bot (* 14 (+ (sum (* res-nextc res-mic (pow bat-r bright-v)))
                      (* zp1 0.6699)
                      (* zp2 -0.1684 (pow r2s bright-v))
                      (* zp3 -0.3563 (pow r3s bright-v)))))
; contact mic: 25 (was 200 = five times the rest of the drum on a muted hit)
(def mic-snap (* contact-hp 25))
(def mixdown (+ mic-top (* mic-bot bottom-mix-v) (* mic-snap snares-v)
                (* rim-sig rim-level-v 0.1)))

; ── Write feedback ──────────────────────────────────────────────────────────
(write-tensor-history bat2 bat1d)
(write-tensor-history bat1 bat-nextc)
(write-tensor-history res2 res1d)
(write-tensor-history res1 res-nextc)

; ── Shaper: tone tilt -> punch -> drive -> DC block ──────────────────────────
; mixdown peaks ~4 at full velocity; the shaper works on a +-1 normalized copy
(def xs (* mixdown 0.25))
; tone: one-pole split at ~1.15 kHz (coef 0.15), tilt the two halves
(def tlp-prev (read-history tonelp))
(def tlp (+ tlp-prev (* 0.15 (- xs tlp-prev))))
(write-history tonelp tlp)
(def toned (+ (* tlp (- 1 (* (max tone-v 0) 0.85)))
              (* (- xs tlp) (+ 1 tone-v))))
; punch: trigger-fired envelope, ~14 ms e-fold, up to +12 dB on the transient
(def penv (max (* (read-history punchenv) 0.9985) trigger))
(write-history punchenv penv)
(def punched (* toned (+ 1 (* punch-v 3 penv))))
; drive: tanh soft clip, normalized so a full-scale hit stays full-scale
(def pre (+ 1 (* drive-v 12)))
(def driven (/ (tanh (* punched pre)) (tanh pre)))
(def bodied (* driven 4))
(def dcin bodied)
(def dcy (+ (- dcin (read-history dcx1)) (* 0.998 (read-history dcy1))))
(write-history dcx1 dcin)
(write-history dcy1 dcy)
; prototype debug tap (see `dbg`)
(def sel1 (clip (- 1 (abs (- dbg 1))) 0 1))
(def sel2 (clip (- 1 (abs (- dbg 2))) 0 1))
(def sel3 (clip (- 1 (abs (- dbg 3))) 0 1))
(def sel0 (clip (- 1 (+ sel1 sel2 sel3)) 0 1))
(out (+ (* dcy level-v vel-gain sel0) (* z-res sel1) (* (read-history w1a) sel2) (* contact-f sel3)) 1 @name audio)

